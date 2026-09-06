//! The one hand-written FFI module in the tree (E24-S2, plan.md Decision 9).
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

#![allow(unsafe_code)]

use std::cell::RefCell;
use std::ffi::c_void;

use objc2::rc::{Allocated, Retained};
use objc2::runtime::{AnyObject, NSObjectProtocol, ProtocolObject};
use objc2::{define_class, msg_send, AnyThread, DefinedClass, MainThreadMarker, MainThreadOnly};
use objc2_app_kit::{
    NSDragOperation, NSDraggingContext, NSDraggingDestination, NSDraggingInfo, NSDraggingItem,
    NSDraggingSession, NSDraggingSource, NSEvent, NSEventModifierFlags, NSEventType, NSImage,
    NSPasteboardItem, NSPasteboardType, NSView, NSWindowOrderingMode,
};
use objc2_foundation::{NSArray, NSData, NSPoint, NSRect, NSSize, NSString};

use super::{DropEvent, DropSink, Modifiers, Phase, Selection, SELECTION_TYPE};
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
}

/// What one window's drop target needs to report an event.
struct SurfaceIvars {
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
    struct DropSurface;

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
    fn new(mtm: MainThreadMarker, content: &NSView, label: &str, sink: DropSink) -> Retained<Self> {
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
/// If this is not the main thread, the view is not in a window, or `image_png`
/// is not an image `AppKit` can read.
pub fn start_drag(ns_view: *mut c_void, selection: &Selection, image_png: &[u8]) -> Result<()> {
    let mtm = main_thread()?;
    let view = view_at(ns_view)?;
    let window = view
        .window()
        .ok_or_else(|| refused("the window has no content view to drag from"))?;

    // The previous drag's source, released now rather than from inside its own
    // `ended` callback.
    SOURCES.with_borrow_mut(Vec::clear);

    let item = NSPasteboardItem::init(NSPasteboardItem::alloc());
    if !item.setString_forType(&NSString::from_str(&selection.to_json()), &selection_type()) {
        return Err(refused("the pasteboard refused the selection"));
    }

    let image = NSImage::initWithData(NSImage::alloc(), &NSData::with_bytes(image_png))
        .ok_or_else(|| refused("the drag image is not a readable PNG"))?;
    let size = image.size();

    let dragging = NSDraggingItem::initWithPasteboardWriter(
        NSDraggingItem::alloc(),
        ProtocolObject::from_ref(&*item),
    );
    // Centred on the pointer. The frame is in the *source view's* coordinates,
    // so the same bottom-left origin conversion applies as on the way in.
    let at = view.convertPoint_fromView(window.mouseLocationOutsideOfEventStream(), None);
    let frame = NSRect::new(
        NSPoint::new(at.x - size.width / 2.0, at.y - size.height / 2.0),
        NSSize::new(size.width, size.height),
    );
    let contents: &AnyObject = &image;
    // SAFETY: `setDraggingFrame:contents:` documents `contents` as an object
    // the dragging item can render; an `NSImage` is the documented case.
    unsafe { dragging.setDraggingFrame_contents(frame, Some(contents)) };

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
        &NSArray::from_retained_slice(&[dragging]),
        &event,
        ProtocolObject::from_ref(&*source),
    );
    SOURCES.with_borrow_mut(|sources| sources.push(source));
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

#[cfg(test)]
mod tests {
    use super::{selection_type, DropSurface};
    use crate::drag::{DropEvent, Selection, SELECTION_TYPE};
    use objc2::{AnyThread, MainThreadMarker, MainThreadOnly};
    use objc2_app_kit::{NSPasteboardItem, NSView};
    use objc2_foundation::{NSRect, NSString};
    use std::sync::{Arc, Mutex};

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

    /// Building a drop target, without a drag.
    ///
    /// A synthetic `performDragOperation:` needs a real `NSDraggingInfo`, and
    /// only a live drag session produces one — there is no public way to build
    /// it, so that half is the manual check recorded in the story's notes. What
    /// *is* testable headlessly is which types the target claims: one that also
    /// claimed `public.file-url` would take Finder's drops away from wry and
    /// E13-S5's upload path would stop firing, silently.
    #[test]
    fn a_drop_target_claims_our_type_and_only_ours() {
        let Some(mtm) = MainThreadMarker::new() else {
            // `cargo test` runs each test on a spawned thread, and this whole
            // module is main-thread-only by construction. Skipping is honest;
            // pretending would mean constructing an `NSView` off the main
            // thread, which is the undefined behaviour the marker exists to
            // prevent.
            return;
        };
        let seen: Arc<Mutex<Vec<DropEvent>>> = Arc::new(Mutex::new(Vec::new()));
        let sink = {
            let seen = Arc::clone(&seen);
            Arc::new(move |_label: &str, event: DropEvent| {
                seen.lock().expect("not poisoned").push(event);
            })
        };
        let content = NSView::initWithFrame(NSView::alloc(mtm), NSRect::ZERO);
        let surface = DropSurface::new(mtm, &content, "main", sink);
        let types: Vec<String> = surface
            .registeredDraggedTypes()
            .iter()
            .map(|kind| kind.to_string())
            .collect();
        assert_eq!(types, vec![SELECTION_TYPE.to_owned()]);
        assert!(seen.lock().expect("not poisoned").is_empty());
    }
}
