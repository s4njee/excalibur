//! Native application identity for direct GPUI launches.
//!
//! The GPUI binary is currently started directly (for example with
//! `cargo run -p excalibur-gpui`) and therefore does not receive the icon from
//! Tauri's `.app` bundling step. Keep the ICNS embedded in the executable so
//! the Dock and native windows do not depend on the process' working directory.

#![cfg(target_os = "macos")]
#![allow(unsafe_code)]

use objc2::{AnyThread, MainThreadMarker};
use objc2_app_kit::{NSApplication, NSImage};
use objc2_foundation::NSData;

const ICON_DATA: &[u8] = include_bytes!("../../../src-tauri/icons/icon.icns");

/// Set the `AppKit` application icon while GPUI is on its main thread.
pub fn install() {
    let Some(main_thread) = MainThreadMarker::new() else {
        return;
    };

    let data = NSData::with_bytes(ICON_DATA);
    let Some(image) = NSImage::initWithData(NSImage::alloc(), &data) else {
        return;
    };
    let application = NSApplication::sharedApplication(main_thread);

    // SAFETY: `image` is a valid, non-null NSImage retained for the duration
    // of this call; AppKit retains the image when assigning it as the
    // application icon.
    unsafe { application.setApplicationIconImage(Some(&image)) };
}
