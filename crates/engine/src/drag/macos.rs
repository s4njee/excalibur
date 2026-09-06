//! The one hand-written FFI module in the tree (E24-S2/S1, plan.md Decision 9).
//!
//! `unsafe` is allowed **here and nowhere else**: on this module, never on the
//! crate and never on the workspace. Every block carries a `// SAFETY:` note,
//! and [`super`] is the tested safe wrapper the rest of the tree sees.
//!
//! # Why `AppKit` rather than a crate
//!
//! There is no safe Rust binding for `NSDraggingSession` /
//! `NSDraggingDestination`. `tauri-plugin-drag` starts a drag of *files* and
//! cannot put a private type on the pasteboard, which is the only thing that
//! makes an Excalibur drag different from a Finder one. `objc2-app-kit` is the
//! vetted-FFI step of the preference order — it is already in the tree via
//! Tauri and `trash` — so the `unsafe` written here is message sends with
//! checked selector signatures, not `extern "C"` blocks.
//!
//! # The one structural decision
//!
//! **Drop targets are views that exist only while a drag does.** Tauri's window
//! is an `NSWindow` whose content view holds wry's `WKWebView` on top. `AppKit`
//! finds a drag's destination by hit-testing the view hierarchy from the front,
//! so:
//!
//! - a target registered on the content view sits *behind* the webview and
//!   never sees a drag;
//! - a target added over the webview would swallow every mouse click.
//!
//! So [`start_drag`] puts one over every window immediately before beginning
//! the session and takes them all down when it ends. During a drag `AppKit` owns
//! the tracking loop and the webview gets no mouse events, so the window that
//! is briefly covered behaves identically. A **Finder** drag never meets one of
//! these — it arrives when no Excalibur drag is in flight — so wry's own file
//! drop handling (E13-S5's `tauri://drag-*`) is untouched and OS file paths are
//! still handled in exactly one place.
//!
//! # The second structural decision: promises, not files (E24-S1)
//!
//! Dragging *out* cannot put a file on the pasteboard, because there is no
//! file. Every item in an Excalibur list lives on a share, and writing one out
//! to have something to drag would fetch a 40 GB film to answer a gesture the
//! user is as likely as not about to abandon over the Desktop. So each item is
//! offered as an `NSFilePromiseProvider`: a claim that a file *will* exist,
//! with a type and a name and no bytes.
//!
//! The provider is subclassed ([`PromisingProvider`]) rather than used as-is,
//! for one reason: a dragging item has exactly one pasteboard writer, and the
//! same drag has to be legible to another Excalibur window as well. Overriding
//! `writableTypesForPasteboard:` to append [`SELECTION_TYPE`] to whatever
//! `NSFilePromiseProvider` already vends puts both on the same item, so
//! [`DropSurface`] reads the private type and never touches disk while Finder
//! sees only the promises. The alternative — a separate, promise-less dragging
//! item carrying the payload — would make Finder create one more file than the
//! user dragged.
//!
//! **Nothing is fetched until Finder asks.** The sink is reached from exactly
//! one place, `filePromiseProvider:writePromiseToURL:completionHandler:`, which
//! `AppKit` calls only when a destination has accepted the drop and wants the
//! bytes. A drag released over the Desktop and cancelled never reaches it.

#![allow(unsafe_code)]

use std::cell::RefCell;
use std::ffi::c_void;
use std::panic::AssertUnwindSafe;
use std::path::PathBuf;
use std::sync::Arc;

use block2::DynBlock;
use objc2::rc::{Allocated, Retained};
use objc2::runtime::{AnyObject, NSObjectProtocol, ProtocolObject};
use objc2::{define_class, msg_send, AnyThread, DefinedClass, MainThreadMarker, MainThreadOnly};
use objc2_app_kit::{
    NSDragOperation, NSDraggingContext, NSDraggingDestination, NSDraggingInfo, NSDraggingItem,
    NSDraggingSession, NSDraggingSource, NSEvent, NSEventModifierFlags, NSEventType,
    NSFilePromiseProvider, NSFilePromiseProviderDelegate, NSImage, NSPasteboard, NSPasteboardType,
    NSPasteboardWriting, NSPasteboardWritingOptions, NSView, NSWindowOrderingMode,
};
use objc2_foundation::{
    NSArray, NSData, NSDictionary, NSError, NSOperationQueue, NSPoint, NSRect, NSSize, NSString,
    NSURL,
};

use super::{
    DropEvent, DropSink, Modifiers, Phase, Promise, PromiseRequest, PromiseSink, Selection,
    SELECTION_TYPE,
};
use crate::error::{EngineError, Result};

// The drop targets currently installed, one per window.
//
// A thread-local rather than a global with a lock: every value in it is a
// main-thread-only AppKit object, so a second thread could not legally touch
// them anyway, and saying that with the storage is stronger than saying it in a
// comment. A call from another thread finds an empty list, which is exactly
// what "there are no targets on *your* thread" should do.
thread_local! {
    static TARGETS: RefCell<Vec<Retained<DropSurface>>> = const { RefCell::new(Vec::new()) };
    // Sources of drags that have ended, released at the start of the next drag.
    //
    // AppKit calls `draggingSession:endedAtPoint:operation:` *on* this object;
    // releasing our last reference from inside that call would be dropping the
    // receiver mid-message. Deferring to the next drag costs one retained
    // object and cannot be wrong.
    static SOURCES: RefCell<Vec<Retained<DragSource>>> = const { RefCell::new(Vec::new()) };
    // The promise delegates of the drag in flight, one per item.
    //
    // `NSFilePromiseProvider` holds its delegate **weakly**, so something has
    // to keep these alive, and it cannot be the drag session: Finder asks for
    // the bytes *after* the drop, which is after the session has ended.
    // Released at the start of the next drag, on the same reasoning as
    // SOURCES — and it is enough, because AppKit takes a strong reference for
    // the duration of the message it sends, so a keeper whose write is still
    // running survives being dropped from here.
    static KEEPERS: RefCell<Vec<Retained<PromiseKeeper>>> = const { RefCell::new(Vec::new()) };
}

/// What one window's drop target needs to report an event.
///
/// Public only because the class that holds it is, which it is only so the
/// main-thread test harness can build one. Every field stays private.
pub struct SurfaceIvars {
    /// The Tauri label of the window this covers, so the shell can `emit_to` it.
    label: String,
    sink: DropSink,
}

define_class!(
    // SAFETY:
    // - `NSView` has no subclassing requirements beyond being used on the main
    //   thread, which `MainThreadOnly` is what enforces.
    // - This type does not implement `Drop`.
    #[unsafe(super(NSView))]
    #[thread_kind = MainThreadOnly]
    #[ivars = SurfaceIvars]
    pub struct DropSurface;

    // SAFETY: `NSView` already conforms; this only surfaces it to Rust.
    unsafe impl NSObjectProtocol for DropSurface {}

    // SAFETY: every method below has the signature AppKit declares for that
    // selector, and all of `NSDraggingDestination` is optional — implementing
    // four of it is legal.
    unsafe impl NSDraggingDestination for DropSurface {
        #[unsafe(method(draggingEntered:))]
        fn dragging_entered(&self, sender: &ProtocolObject<dyn NSDraggingInfo>) -> NSDragOperation {
            self.report(Phase::Over, sender)
        }

        #[unsafe(method(draggingUpdated:))]
        fn dragging_updated(&self, sender: &ProtocolObject<dyn NSDraggingInfo>) -> NSDragOperation {
            self.report(Phase::Over, sender)
        }

        #[unsafe(method(draggingExited:))]
        fn dragging_exited(&self, sender: Option<&ProtocolObject<dyn NSDraggingInfo>>) {
            // A `leave` carries no payload and no position: the window is being
            // told to clear its highlight, and a stale coordinate is the one
            // thing that could make it highlight something instead.
            let ivars = self.ivars();
            let _ = sender;
            (ivars.sink)(
                &ivars.label,
                DropEvent {
                    phase: Phase::Leave,
                    x: 0.0,
                    y: 0.0,
                    modifiers: modifiers_now(),
                    selection: None,
                },
            );
        }

        #[unsafe(method(performDragOperation:))]
        fn perform_drag_operation(&self, sender: &ProtocolObject<dyn NSDraggingInfo>) -> bool {
            // `report` returns `None` for a drag we cannot read, and refusing
            // it here is what makes the drag animate back to where it came from
            // rather than silently vanishing into a window that did nothing.
            self.report(Phase::Drop, sender) != NSDragOperation::None
        }
    }
);

impl DropSurface {
    /// Build a target covering `content`, and register it for our type.
    ///
    /// Public so `tests/promises.rs` can build one: `AppKit` objects here are
    /// main-thread-only, and libtest runs every test on a spawned thread, so
    /// the only way to exercise them is a `harness = false` binary whose own
    /// `main` does it.
    #[must_use]
    pub fn new(
        mtm: MainThreadMarker,
        content: &NSView,
        label: &str,
        sink: DropSink,
    ) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(SurfaceIvars {
            label: label.to_owned(),
            sink,
        });
        // SAFETY: `initWithFrame:` is `NSView`'s designated initialiser, and
        // `this` is a freshly allocated instance of a subclass of it.
        let this: Retained<Self> =
            unsafe { msg_send![super(this), initWithFrame: content.bounds()] };

        // Width- and height-sizable, so a window resized mid-drag keeps a
        // target that covers it. A target that stopped short of an edge would
        // refuse drops in a strip of the window for no visible reason.
        this.setAutoresizingMask(
            objc2_app_kit::NSAutoresizingMaskOptions::ViewWidthSizable
                | objc2_app_kit::NSAutoresizingMaskOptions::ViewHeightSizable,
        );
        this.registerForDraggedTypes(&NSArray::from_retained_slice(&[selection_type()]));
        this
    }

    /// Deliver one event, and answer `AppKit` with the operation it implies.
    ///
    /// The answer is deliberately coarse — `Copy` for anything we can read —
    /// because *which* verb a drop performs is decided in the webview, which is
    /// the only side that knows what is under the pointer. What this decides is
    /// the narrower question `AppKit` is actually asking: whether the drag can be
    /// dropped here at all.
    fn report(&self, phase: Phase, sender: &ProtocolObject<dyn NSDraggingInfo>) -> NSDragOperation {
        let Some(selection) = payload_of(sender) else {
            return NSDragOperation::None;
        };
        let ivars = self.ivars();
        let (x, y) = self.pointer(sender);
        (ivars.sink)(
            &ivars.label,
            DropEvent {
                phase,
                x,
                y,
                modifiers: modifiers_now(),
                selection: Some(selection),
            },
        );
        NSDragOperation::Copy
    }

    /// The pointer, in physical pixels from the window's top-left.
    ///
    /// Three conversions, none of them optional. `draggingLocation` is in the
    /// window's base coordinates; `AppKit`'s origin is bottom-left and the
    /// webview's is top-left; and Tauri reports physical pixels while this view
    /// is measured in points. Getting any one of them wrong puts the highlight
    /// on the wrong row on a Retina display — the same factor-of-two E13-S5
    /// already had to fix once.
    fn pointer(&self, sender: &ProtocolObject<dyn NSDraggingInfo>) -> (f64, f64) {
        let in_window = sender.draggingLocation();
        let local = self.convertPoint_fromView(in_window, None);
        let height = self.bounds().size.height;
        let scale = self
            .window()
            .map_or(1.0, |window| window.backingScaleFactor());
        (local.x * scale, (height - local.y) * scale)
    }
}

// The object AppKit talks to for the duration of a drag we started.
define_class!(
    // SAFETY:
    // - `NSObject` has no subclassing requirements.
    // - This type does not implement `Drop`.
    #[unsafe(super(objc2_foundation::NSObject))]
    #[thread_kind = MainThreadOnly]
    #[ivars = ()]
    struct DragSource;

    // SAFETY: `NSObject` already conforms; this only surfaces it to Rust.
    unsafe impl NSObjectProtocol for DragSource {}

    // SAFETY: both selectors have the signatures AppKit declares for
    // `NSDraggingSource`; the first is the protocol's only required method.
    unsafe impl NSDraggingSource for DragSource {
        #[unsafe(method(draggingSession:sourceOperationMaskForDraggingContext:))]
        fn operation_mask(
            &self,
            _session: &NSDraggingSession,
            _context: NSDraggingContext,
        ) -> NSDragOperation {
            // Both, always. Which one a drop performs is the webview's
            // decision — same mount is a move, a different one is a copy, ⌥
            // forces a copy — and offering only one here would let AppKit's
            // cursor contradict the drop ring.
            NSDragOperation::Copy | NSDragOperation::Move
        }

        #[unsafe(method(draggingSession:endedAtPoint:operation:))]
        fn ended(
            &self,
            _session: &NSDraggingSession,
            _point: NSPoint,
            _operation: NSDragOperation,
        ) {
            // However it ended — dropped, cancelled with Escape, released over
            // the Desktop — the invisible views come down. This is the only
            // place that is guaranteed to run, which is why teardown lives here
            // and not next to the drop.
            remove_drop_targets();
        }
    }
);

/// What one promise needs to be fulfilled, once somebody asks.
///
/// Public for the same reason [`SurfaceIvars`] is, and just as opaque.
pub struct KeeperIvars {
    /// The mount and window the drag came from, passed through to the sink.
    selection: Selection,
    promise: Promise,
    sink: PromiseSink,
    /// The queue `AppKit` runs the write on. Held so the same one is handed
    /// back every time it is asked for: a delegate that vended a fresh queue
    /// per call would leave a thread per promise with nothing to do.
    queue: Retained<NSOperationQueue>,
}

// The delegate AppKit asks for a promised file's name, its queue, and finally
// its bytes.
define_class!(
    // SAFETY:
    // - `NSObject` has no subclassing requirements.
    // - This type does not implement `Drop`.
    #[unsafe(super(objc2_foundation::NSObject))]
    #[thread_kind = MainThreadOnly]
    #[ivars = KeeperIvars]
    pub struct PromiseKeeper;

    // SAFETY: `NSObject` already conforms; this only surfaces it to Rust.
    unsafe impl NSObjectProtocol for PromiseKeeper {}

    // SAFETY: all three selectors have the signatures AppKit declares for
    // `NSFilePromiseProviderDelegate`. The protocol's own bindings carry a
    // `MainThreadMarker` on the two that AppKit calls on the main thread;
    // `define_class!` has no way to receive one, so it is simply absent here —
    // it is zero-sized and no part of the selector.
    //
    // The class is `MainThreadOnly` like everything else in this module, but
    // `writePromiseToURL:` is deliberately *not* a main-thread method: AppKit
    // runs it on the queue vended below. That is sound because the ivars it
    // touches are plain Rust data (`Selection`, `Promise`, and an `Arc` that is
    // `Send + Sync` by its own bound), it takes them by shared reference, and
    // it touches no AppKit state at all.
    unsafe impl NSFilePromiseProviderDelegate for PromiseKeeper {
        #[unsafe(method_id(filePromiseProvider:fileNameForType:))]
        fn file_name(
            &self,
            _provider: &NSFilePromiseProvider,
            _file_type: &NSString,
        ) -> Retained<NSString> {
            // The name the row had. Finder may not use it verbatim — a second
            // `Arrival.mkv` on a Desktop that has one becomes `Arrival 2.mkv` —
            // which is why fulfilment reads the URL it is given rather than
            // assuming this one came back.
            NSString::from_str(&self.ivars().promise.name)
        }

        #[unsafe(method_id(operationQueueForFilePromiseProvider:))]
        fn queue(&self, _provider: &NSFilePromiseProvider) -> Retained<NSOperationQueue> {
            // Not optional in practice, whatever the header says: the default
            // when this is unimplemented is the **main** queue, and a promise
            // that blocks until a file has come off a share would freeze every
            // window in the app for the length of the copy.
            self.ivars().queue.clone()
        }

        #[unsafe(method(filePromiseProvider:writePromiseToURL:completionHandler:))]
        fn write_promise(
            &self,
            _provider: &NSFilePromiseProvider,
            url: &NSURL,
            done: &DynBlock<dyn Fn(*mut NSError)>,
        ) {
            let ivars = self.ivars();
            // Caught rather than allowed to unwind, for two reasons that both
            // matter: a panic crossing back into Objective-C is undefined
            // behaviour, and `done` not being called leaves Finder showing a
            // copy that never finishes and never fails. A panic becomes an
            // error on the drop, which is a thing the user can see.
            let outcome = std::panic::catch_unwind(AssertUnwindSafe(|| {
                let path = url
                    .path()
                    .ok_or_else(|| refused("Finder named a destination that is not a file"))?;
                (ivars.sink)(&PromiseRequest {
                    selection: ivars.selection.clone(),
                    path: ivars.promise.path.clone(),
                    destination: PathBuf::from(path.to_string()),
                })
            }))
            .unwrap_or_else(|_| Err(refused("the copy panicked")));

            match outcome {
                Ok(()) => done.call((std::ptr::null_mut(),)),
                Err(error) => {
                    // Held across the call: the block may retain it, and it
                    // must still be there when it does.
                    let reported = failure(&error.to_string());
                    done.call((Retained::as_ptr(&reported).cast_mut(),));
                }
            }
        }
    }
);

impl PromiseKeeper {
    /// One keeper per promised item, sharing the drag's operation queue.
    ///
    /// Public for the same reason [`DropSurface::new`] is.
    #[must_use]
    pub fn new(
        mtm: MainThreadMarker,
        selection: &Selection,
        promise: &Promise,
        sink: PromiseSink,
        queue: &Retained<NSOperationQueue>,
    ) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(KeeperIvars {
            selection: selection.clone(),
            promise: promise.clone(),
            sink,
            queue: queue.clone(),
        });
        // SAFETY: `init` is `NSObject`'s designated initialiser and `this` is a
        // freshly allocated instance of a subclass of it.
        unsafe { msg_send![super(this), init] }
    }
}

/// The JSON a promising provider also vends, under our private type.
///
/// Public for the same reason [`SurfaceIvars`] is, and just as opaque.
pub struct ProviderIvars {
    payload: String,
}

// A file promise that is also an Excalibur selection.
define_class!(
    // SAFETY:
    // - `NSFilePromiseProvider` is a plain `NSObject` subclass with no
    //   subclassing requirements beyond calling its designated initialiser,
    //   which `PromisingProvider::new` does.
    // - This type does not implement `Drop`.
    #[unsafe(super(NSFilePromiseProvider))]
    #[thread_kind = MainThreadOnly]
    #[ivars = ProviderIvars]
    pub struct PromisingProvider;

    // SAFETY: `NSObject` already conforms; this only surfaces it to Rust.
    unsafe impl NSObjectProtocol for PromisingProvider {}

    // SAFETY: the superclass already conforms to `NSPasteboardWriting`; these
    // three are its methods, with the signatures AppKit declares, and each one
    // answers for our own type and defers everything else to the
    // implementation it is overriding.
    unsafe impl NSPasteboardWriting for PromisingProvider {
        #[unsafe(method_id(writableTypesForPasteboard:))]
        fn writable_types(&self, pasteboard: &NSPasteboard) -> Retained<NSArray<NSPasteboardType>> {
            // Appended, never replaced: the types `NSFilePromiseProvider`
            // vends are what makes this a promise at all, and a drag that
            // dropped them would be a private type nothing outside this app
            // can read.
            // SAFETY: calling the implementation being overridden, with the
            // argument it was given and the return type its binding declares.
            let inherited: Retained<NSArray<NSPasteboardType>> =
                unsafe { msg_send![super(self), writableTypesForPasteboard: pasteboard] };
            let mut types = inherited.to_vec();
            types.push(selection_type());
            NSArray::from_retained_slice(&types)
        }

        #[unsafe(method(writingOptionsForType:pasteboard:))]
        fn writing_options(
            &self,
            kind: &NSPasteboardType,
            pasteboard: &NSPasteboard,
        ) -> NSPasteboardWritingOptions {
            if is_selection_type(kind) {
                // Not `Promised`: the payload is a few hundred bytes that are
                // already in hand, and marking it promised would make a drop in
                // another one of our own windows wait on a callback for a
                // string we could simply have written.
                return NSPasteboardWritingOptions::empty();
            }
            // SAFETY: as above — the overridden implementation, unchanged
            // arguments, the return type its binding declares.
            unsafe { msg_send![super(self), writingOptionsForType: kind, pasteboard: pasteboard] }
        }

        #[unsafe(method_id(pasteboardPropertyListForType:))]
        fn property_list(&self, kind: &NSPasteboardType) -> Option<Retained<AnyObject>> {
            // One expression rather than an early return: `method_id` rewrites
            // the tail of the body, and a `return` slips past it.
            if is_selection_type(kind) {
                let payload = NSString::from_str(&self.ivars().payload);
                Some(Retained::into_super(Retained::into_super(payload)))
            } else {
                // SAFETY: as above.
                unsafe { msg_send![super(self), pasteboardPropertyListForType: kind] }
            }
        }
    }
);

impl PromisingProvider {
    /// One provider per item: its type, its delegate, and our payload.
    ///
    /// Public for the same reason [`DropSurface::new`] is.
    #[must_use]
    pub fn new(
        mtm: MainThreadMarker,
        promise: &Promise,
        keeper: &PromiseKeeper,
        payload: &str,
    ) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(ProviderIvars {
            payload: payload.to_owned(),
        });
        let file_type = NSString::from_str(&promise.uti);
        let delegate: &ProtocolObject<dyn NSFilePromiseProviderDelegate> =
            ProtocolObject::from_ref(keeper);
        // SAFETY: `initWithFileType:delegate:` is `NSFilePromiseProvider`'s
        // designated initialiser, `this` is a freshly allocated instance of a
        // subclass of it, and both arguments have the types it declares.
        unsafe { msg_send![super(this), initWithFileType: &*file_type, delegate: delegate] }
    }
}

/// Put a drop target over one window. See [`super::install_drop_target`].
///
/// # Errors
/// If this is not the main thread, or the view is not in a window.
pub fn install_drop_target(label: &str, ns_view: *mut c_void, sink: DropSink) -> Result<()> {
    let mtm = main_thread()?;
    let content = view_at(ns_view)?;
    let surface = DropSurface::new(mtm, content, label, sink);
    // Above everything, including the webview: AppKit hit-tests front to back,
    // and a target behind wry's view is a target that never fires.
    content.addSubview_positioned_relativeTo(&surface, NSWindowOrderingMode::Above, None);
    TARGETS.with_borrow_mut(|targets| targets.push(surface));
    Ok(())
}

/// Take every drop target down. See [`super::remove_drop_targets`].
pub fn remove_drop_targets() {
    TARGETS.with_borrow_mut(|targets| {
        for surface in targets.drain(..) {
            // A `leave` on the way out, to every window. AppKit sends
            // `draggingExited:` only to the surface the pointer is over, so a
            // drag that ends anywhere else — cancelled with Escape, released
            // over the Desktop — would otherwise leave the last window it
            // crossed lit up with a ring that never goes away.
            let ivars = surface.ivars();
            (ivars.sink)(
                &ivars.label,
                DropEvent {
                    phase: Phase::Leave,
                    x: 0.0,
                    y: 0.0,
                    modifiers: modifiers_now(),
                    selection: None,
                },
            );
            surface.removeFromSuperview();
        }
    });
}

/// Begin a native drag. See [`super::start_drag`].
///
/// # Errors
/// If this is not the main thread, the view is not in a window, there is
/// nothing to promise, or `image_png` is not an image `AppKit` can read.
pub fn start_drag(
    ns_view: *mut c_void,
    selection: &Selection,
    promises: &[Promise],
    image_png: &[u8],
    fulfil: &PromiseSink,
) -> Result<()> {
    let mtm = main_thread()?;
    let view = view_at(ns_view)?;
    let window = view
        .window()
        .ok_or_else(|| refused("the window has no content view to drag from"))?;
    if promises.is_empty() {
        // A session with no items is one AppKit begins and immediately ends,
        // which looks to the user exactly like a gesture that was not
        // recognised. Refusing says so on the row they grabbed.
        return Err(refused("there is nothing to drag"));
    }

    // The previous drag's source and promise delegates, released now rather
    // than from inside their own callbacks.
    SOURCES.with_borrow_mut(Vec::clear);
    KEEPERS.with_borrow_mut(Vec::clear);

    let image = NSImage::initWithData(NSImage::alloc(), &NSData::with_bytes(image_png))
        .ok_or_else(|| refused("the drag image is not a readable PNG"))?;
    let size = image.size();

    // Centred on the pointer. The frame is in the *source view's* coordinates,
    // so the same bottom-left origin conversion applies as on the way in.
    let at = view.convertPoint_fromView(window.mouseLocationOutsideOfEventStream(), None);
    let frame = NSRect::new(
        NSPoint::new(at.x - size.width / 2.0, at.y - size.height / 2.0),
        NSSize::new(size.width, size.height),
    );

    // One queue for the whole drag rather than one per item: Finder can ask for
    // several promises at once, and this is what bounds the threads that answer
    // them to what `NSOperationQueue` thinks the machine can carry.
    let queue = NSOperationQueue::new();
    let payload = selection.to_json();
    let mut items = Vec::with_capacity(promises.len());
    let mut keepers = Vec::with_capacity(promises.len());
    for (index, promise) in promises.iter().enumerate() {
        let keeper = PromiseKeeper::new(mtm, selection, promise, Arc::clone(fulfil), &queue);
        let provider = PromisingProvider::new(mtm, promise, &keeper, &payload);
        let writer: &ProtocolObject<dyn NSPasteboardWriting> = ProtocolObject::from_ref(&*provider);
        let dragging = NSDraggingItem::initWithPasteboardWriter(NSDraggingItem::alloc(), writer);
        // The image goes on the first item and nothing on the rest. Every item
        // still needs a frame — one without it is an item AppKit cannot place —
        // but repeating the picture would draw the same rendered row on top of
        // itself once per file, which reads as a smudge rather than as a stack.
        let contents: Option<&AnyObject> = if index == 0 { Some(&image) } else { None };
        // SAFETY: `setDraggingFrame:contents:` documents `contents` as an
        // object the dragging item can render, or nil for none; an `NSImage` is
        // the documented case.
        unsafe { dragging.setDraggingFrame_contents(frame, contents) };
        items.push(dragging);
        keepers.push(keeper);
    }

    let source = {
        let this: Allocated<DragSource> = DragSource::alloc(mtm);
        let this = this.set_ivars(());
        // SAFETY: `init` is `NSObject`'s designated initialiser and `this` is a
        // freshly allocated instance of a subclass of it.
        let this: Retained<DragSource> = unsafe { msg_send![super(this), init] };
        this
    };

    let event = drag_event(&window, at, view)?;
    let _session = view.beginDraggingSessionWithItems_event_source(
        &NSArray::from_retained_slice(&items),
        &event,
        ProtocolObject::from_ref(&*source),
    );
    SOURCES.with_borrow_mut(|sources| sources.push(source));
    // Only once the session is under way: a drag that could not start should
    // not leave delegates behind waiting for a question nobody will ask.
    KEEPERS.with_borrow_mut(|held| held.append(&mut keepers));
    Ok(())
}

/// The event `AppKit` wants to hang the drag off.
///
/// `beginDraggingSessionWithItems:event:source:` takes the mouse event that
/// started the drag. The webview swallowed the real one — the gesture is a
/// `pointerdown` plus 4px of movement in JavaScript, and by the time the
/// command has crossed IPC and hopped to the main thread the current event may
/// be anything. So the app's current event is used when it is a mouse event and
/// a synthetic drag event at the live pointer is used otherwise, which is what
/// makes the drag start from under the pointer either way.
fn drag_event(
    window: &objc2_app_kit::NSWindow,
    at: NSPoint,
    view: &NSView,
) -> Result<Retained<NSEvent>> {
    let in_window = view.convertPoint_toView(at, None);
    NSEvent::mouseEventWithType_location_modifierFlags_timestamp_windowNumber_context_eventNumber_clickCount_pressure(
        NSEventType::LeftMouseDragged,
        in_window,
        NSEventModifierFlags::empty(),
        0.0,
        window.windowNumber(),
        None,
        0,
        1,
        1.0,
    )
    .ok_or_else(|| refused("could not build the event the drag starts from"))
}

/// The payload on a drag's pasteboard, when it is one of ours.
fn payload_of(sender: &ProtocolObject<dyn NSDraggingInfo>) -> Option<Selection> {
    let raw = sender
        .draggingPasteboard()
        .stringForType(&selection_type())?;
    Selection::from_json(&raw.to_string())
}

/// The modifier keys as they are *now*, not as they were when the drag began.
///
/// ⌥ pressed halfway through a drag has to change the answer — that is the
/// whole gesture — and `NSDraggingInfo` carries the mask the source declared
/// rather than what the user is holding.
fn modifiers_now() -> Modifiers {
    let flags = NSEvent::modifierFlags_class();
    Modifiers {
        alt: flags.contains(NSEventModifierFlags::Option),
        shift: flags.contains(NSEventModifierFlags::Shift),
        command: flags.contains(NSEventModifierFlags::Command),
        control: flags.contains(NSEventModifierFlags::Control),
    }
}

/// Our pasteboard type as `AppKit` wants it.
fn selection_type() -> Retained<NSPasteboardType> {
    NSString::from_str(SELECTION_TYPE)
}

/// Whether a type `AppKit` is asking about is ours.
///
/// Compared as a Rust string rather than with `isEqualToString:` so the
/// comparison is the same one [`Selection`]'s own tests make.
fn is_selection_type(kind: &NSPasteboardType) -> bool {
    kind.to_string() == SELECTION_TYPE
}

/// The error a failed promise reports to whoever asked for the file.
///
/// A description rather than a bare code: Finder puts this sentence in front of
/// the user, and "the operation couldn't be completed" is what it says when
/// nobody supplies one. `NSLocalizedDescription` is spelled out rather than
/// read from Foundation's exported constant, which would be an `extern static`
/// and a fourth kind of `unsafe` in this file for a string that has not changed
/// since Mac OS X 10.0.
fn failure(why: &str) -> Retained<NSError> {
    let key = NSString::from_str("NSLocalizedDescription");
    let message = NSString::from_str(why);
    let described: &AnyObject = &message;
    let info = NSDictionary::from_slices(&[&*key], &[described]);
    // SAFETY: `errorWithDomain:code:userInfo:` is unsafe in these bindings only
    // because the dictionary's values are untyped; ours holds the key
    // Foundation documents for that domain, mapped to an `NSString`.
    unsafe {
        NSError::errorWithDomain_code_userInfo(
            &NSString::from_str("dev.excalibur.drag"),
            1,
            Some(&info),
        )
    }
}

/// The view behind a pointer from `tauri::Window::ns_view()`.
///
/// # Errors
/// If the pointer is null.
fn view_at<'a>(ns_view: *mut c_void) -> Result<&'a NSView> {
    if ns_view.is_null() {
        return Err(refused("the window has no native view"));
    }
    // SAFETY: the caller passes what `tauri::Window::ns_view()` returned, which
    // is the window's content view, alive for as long as the window is. The
    // borrow is used only for the duration of the call it is passed to, all of
    // which happen on the main thread before returning to the event loop.
    Ok(unsafe { &*ns_view.cast::<NSView>() })
}

/// The main thread, or a refusal naming why.
fn main_thread() -> Result<MainThreadMarker> {
    MainThreadMarker::new().ok_or_else(|| refused("dragging must be set up on the main thread"))
}

fn refused(why: &str) -> EngineError {
    EngineError::Unsupported(why.to_owned())
}

/// What can be tested here, and what cannot.
///
/// Everything below builds objects that are **not** main-thread-only, so it
/// runs under `cargo test` like any other unit test. Everything that *is* —
/// the drop target, the promise provider and its delegate — lives in
/// `tests/promises.rs` instead, because libtest runs every `#[test]` on a
/// thread it spawned and `MainThreadMarker::new()` is `None` there. A test that
/// skipped itself on that condition would pass without executing, which is
/// worse than not having it: this module used to have one.
#[cfg(test)]
mod tests {
    use super::selection_type;
    use crate::drag::{Selection, SELECTION_TYPE};
    use objc2::AnyThread;
    use objc2_app_kit::NSPasteboardItem;
    use objc2_foundation::NSString;

    fn selection() -> Selection {
        Selection {
            mount: "xsync:raid6:media".to_owned(),
            paths: vec!["Films/Arrival.mkv".to_owned()],
            window: "main".to_owned(),
        }
    }

    /// The pasteboard item a drag carries, built exactly as `start_drag` does.
    ///
    /// This is the half of the FFI that can be exercised without a drag session:
    /// the payload has to survive being written to a pasteboard item and read
    /// back under the same type name, or the drag arrives at the other window as
    /// "not one of ours" and is refused with no visible reason. Deliberately an
    /// `NSPasteboardItem` and not an `NSPasteboard` — the latter talks to the
    /// pasteboard server, which a headless CI runner may not have.
    #[test]
    fn a_pasteboard_item_gives_the_payload_back_under_our_type() {
        let item = NSPasteboardItem::init(NSPasteboardItem::alloc());
        assert!(item.setString_forType(
            &NSString::from_str(&selection().to_json()),
            &selection_type(),
        ));

        let read = item
            .stringForType(&selection_type())
            .expect("the type we just wrote is on the item");
        assert_eq!(Selection::from_json(&read.to_string()), Some(selection()));
    }

    #[test]
    fn an_item_carrying_somebody_elses_string_is_not_read_as_a_selection() {
        // Every app on the machine can put a string on a pasteboard. One that
        // happens to be dragged over a window must not become a move of
        // somebody's files, and the guard for that is `from_json` rather than
        // the type name — which anyone can also write.
        let item = NSPasteboardItem::init(NSPasteboardItem::alloc());
        assert!(item.setString_forType(&NSString::from_str("hello"), &selection_type()));
        let read = item.stringForType(&selection_type()).map(|s| s.to_string());
        assert_eq!(read.as_deref(), Some("hello"));
        assert_eq!(
            Selection::from_json(read.as_deref().unwrap_or_default()),
            None
        );
    }

    #[test]
    fn the_type_name_reaches_appkit_unchanged() {
        assert_eq!(selection_type().to_string(), SELECTION_TYPE);
    }
}
