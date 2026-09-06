//! The drag's `AppKit` objects, on the thread they are only legal on (E24-S1).
//!
//! **Why this is a `harness = false` binary and not `#[test]` functions.**
//! Every object here — the drop target, the file promise, its delegate — is
//! `MainThreadOnly`, and libtest runs each `#[test]` on a thread it spawned. A
//! unit test that asked for a `MainThreadMarker` therefore never gets one; it
//! can only skip itself, and a test that skips itself is a test that passes
//! without executing. `drag/macos.rs` used to have exactly that, which is how
//! this file came to exist. `main` here *is* the process's main thread, so the
//! marker is real and every assertion below runs.
//!
//! What is still not testable, and is stated plainly rather than faked: an
//! `NSDraggingInfo` cannot be constructed — there is no public API for one, and
//! only a live session produces it — so a synthetic drop is out of reach, and
//! so is a drag onto a real Finder window. Those are the manual checks in the
//! story's notes.

#[cfg(not(target_os = "macos"))]
fn main() {
    // The FFI is macOS-only until E24-S4. Nothing to say here on Linux.
}

#[cfg(target_os = "macos")]
fn main() {
    let mtm = objc2::MainThreadMarker::new()
        .expect("a `harness = false` test's main *is* the main thread");
    macos::a_drop_target_claims_our_type_and_only_ours(mtm);
    macos::a_promise_carries_our_payload_beside_its_own_type(mtm);
    macos::asking_for_a_promised_file_queues_it_into_the_folder_finder_named(mtm);
    macos::the_promised_name_is_the_row_the_user_grabbed(mtm);
    macos::a_promise_is_never_written_on_the_main_queue(mtm);
    println!("ok: 5 main-thread checks");
}

#[cfg(target_os = "macos")]
mod macos {
    use std::path::PathBuf;
    use std::sync::{Arc, Mutex};

    use excalibur_engine::drag::macos::{DropSurface, PromiseKeeper, PromisingProvider};
    use excalibur_engine::drag::{
        promises_for, DropEvent, PromiseRequest, PromiseSink, Selection, SELECTION_TYPE,
    };
    use objc2::rc::Retained;
    use objc2::{MainThreadMarker, MainThreadOnly};
    use objc2_app_kit::{NSFilePromiseProviderDelegate, NSPasteboard, NSPasteboardWriting, NSView};
    use objc2_foundation::{NSError, NSOperationQueue, NSRect, NSString, NSURL};

    fn selection() -> Selection {
        Selection {
            mount: "xsync:raid6:media".to_owned(),
            paths: vec!["Films/Arrival.mkv".to_owned()],
            window: "main".to_owned(),
        }
    }

    /// One promise, its delegate and a record of everything it was asked for.
    fn promising(
        mtm: MainThreadMarker,
    ) -> (
        Retained<PromisingProvider>,
        Retained<PromiseKeeper>,
        Arc<Mutex<Vec<PromiseRequest>>>,
    ) {
        let asked: Arc<Mutex<Vec<PromiseRequest>>> = Arc::new(Mutex::new(Vec::new()));
        let sink: PromiseSink = {
            let asked = Arc::clone(&asked);
            Arc::new(move |request: &PromiseRequest| {
                asked.lock().expect("not poisoned").push(request.clone());
                Ok(())
            })
        };
        let promise = promises_for(&selection(), &[])
            .into_iter()
            .next()
            .expect("one path, one promise");
        let queue = NSOperationQueue::new();
        let keeper = PromiseKeeper::new(mtm, &selection(), &promise, sink, &queue);
        let provider = PromisingProvider::new(mtm, &promise, &keeper, &selection().to_json());
        (provider, keeper, asked)
    }

    /// The drop target registers for our type and nothing else.
    ///
    /// One that also claimed `public.file-url` would take Finder's drops away
    /// from wry, and E13-S5's upload path would stop firing, silently.
    pub fn a_drop_target_claims_our_type_and_only_ours(mtm: MainThreadMarker) {
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

    /// The promise's own type and our payload, on **one** pasteboard writer.
    ///
    /// The acceptance criterion in one check. A drag carrying only the promise
    /// would be invisible to another Excalibur window; one carrying only the
    /// private type would be invisible to Finder. They have to be on the same
    /// item, because a dragging item has exactly one writer and a second item
    /// would be one more file than the user dragged.
    pub fn a_promise_carries_our_payload_beside_its_own_type(mtm: MainThreadMarker) {
        let (provider, _keeper, asked) = promising(mtm);

        let ours = NSString::from_str(SELECTION_TYPE);
        let payload = provider
            .pasteboardPropertyListForType(&ours)
            .expect("our type is one the provider vends");
        let json = payload
            .downcast::<NSString>()
            .expect("a string, which is what `stringForType:` on the other side reads");
        assert_eq!(Selection::from_json(&json.to_string()), Some(selection()));

        // The promise's own type is still AppKit's answer, untouched. This is
        // the half a naive override deletes.
        assert_eq!(provider.fileType().to_string(), "public.movie");

        // And the list AppKit reads to decide what it may *ask* for carries
        // both: the five `com.apple.pasteboard.promised-*` types that make this
        // a promise, and ours. A private pasteboard rather than the general
        // one — nothing here should touch what the user has copied — though it
        // is still the pasteboard *server*, so a runner without one fails here
        // rather than skipping, which is the point.
        let board = NSPasteboard::pasteboardWithUniqueName();
        let offered: Vec<String> = provider
            .writableTypesForPasteboard(&board)
            .iter()
            .map(|kind| kind.to_string())
            .collect();
        assert!(
            offered.iter().any(|kind| kind == SELECTION_TYPE),
            "{offered:?}"
        );
        assert!(
            offered
                .iter()
                .any(|kind| kind.starts_with("com.apple.pasteboard.promised-")),
            "{offered:?}"
        );

        // And building all of that fetched nothing.
        assert!(asked.lock().expect("not poisoned").is_empty());
    }

    /// Fulfilment asks for the folder Finder named, and only when it names one.
    ///
    /// `writePromiseToURL:` is the **only** door from a drag to the transfer
    /// queue, which is what makes "a drag cancelled over the Desktop moves
    /// nothing" true by construction rather than by a check somebody has to
    /// remember. So this drives that one method and asserts what came through
    /// it — and asserts the counter was zero right up until it did.
    pub fn asking_for_a_promised_file_queues_it_into_the_folder_finder_named(
        mtm: MainThreadMarker,
    ) {
        let (provider, keeper, asked) = promising(mtm);
        assert!(asked.lock().expect("not poisoned").is_empty());

        let url = NSURL::fileURLWithPath(&NSString::from_str("/tmp/excalibur-drag/Arrival 2.mkv"));
        let done = block2::RcBlock::new(|_error: *mut NSError| {});
        keeper.filePromiseProvider_writePromiseToURL_completionHandler(&provider, &url, &done);

        let requests = asked.lock().expect("not poisoned");
        assert_eq!(requests.len(), 1);
        let request = requests.first().expect("exactly one request");
        assert_eq!(request.path, "Films/Arrival.mkv");
        // The *folder*, not the file: that is what a transfer is queued to.
        assert_eq!(request.folder(), PathBuf::from("/tmp/excalibur-drag"));
        // And Finder's de-duplicated name kept verbatim. Writing the name we
        // promised instead would overwrite the `Arrival.mkv` already there.
        assert_eq!(
            request.destination,
            PathBuf::from("/tmp/excalibur-drag/Arrival 2.mkv")
        );
        assert_eq!(request.selection.mount, "xsync:raid6:media");
    }

    /// The name Finder is offered is the name the row had.
    pub fn the_promised_name_is_the_row_the_user_grabbed(mtm: MainThreadMarker) {
        let (provider, keeper, _asked) = promising(mtm);
        let name = keeper.filePromiseProvider_fileNameForType(
            &provider,
            &NSString::from_str("public.movie"),
            mtm,
        );
        assert_eq!(name.to_string(), "Arrival.mkv");
    }

    /// The write never lands on the main queue.
    ///
    /// The default when `operationQueueForFilePromiseProvider:` is not
    /// implemented is the **main** queue, and fulfilment blocks until the bytes
    /// have landed — which on the main queue is every window in the app frozen
    /// for the length of the copy.
    pub fn a_promise_is_never_written_on_the_main_queue(mtm: MainThreadMarker) {
        let (provider, keeper, _asked) = promising(mtm);
        let queue = keeper.operationQueueForFilePromiseProvider(&provider, mtm);
        assert_ne!(&*queue, &*NSOperationQueue::mainQueue());
    }
}
