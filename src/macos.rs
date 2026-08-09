//! macOS specific workarounds.

use std::os::raw::c_void;

/// Stops the green titlebar button from offering *native* fullscreen.
///
/// Native fullscreen moves the window onto a Space of its own, which is where
/// the freeze on losing focus comes from. With `FullScreenNone` the button
/// zooms the window instead, so no route leads into a Space; the app's own
/// `-f` / `F` fullscreen uses the pre-Lion "simple fullscreen" mode, which
/// fills the screen while staying on the current Space.
///
/// Only the fullscreen bits of the collection behaviour are touched, so
/// whatever else winit set stays as it was.
pub fn disable_native_fullscreen(ns_window: *mut c_void) {
    use objc::runtime::Object;
    use objc::{msg_send, sel, sel_impl};

    // NSWindowCollectionBehavior, from NSWindow.h.
    const FULL_SCREEN_PRIMARY: u64 = 1 << 7;
    const FULL_SCREEN_AUXILIARY: u64 = 1 << 8;
    const FULL_SCREEN_NONE: u64 = 1 << 9;

    let window = ns_window as *mut Object;
    if window.is_null() {
        return;
    }

    unsafe {
        let current: u64 = msg_send![window, collectionBehavior];
        let updated = (current & !(FULL_SCREEN_PRIMARY | FULL_SCREEN_AUXILIARY)) | FULL_SCREEN_NONE;
        let _: () = msg_send![window, setCollectionBehavior: updated];
    }
}

/// Declares this process as doing user initiated, latency critical work so that
/// App Nap leaves it alone.
///
/// Without this, macOS throttles an application that is not frontmost. A
/// fullscreen window sits on its own Space and counts as fully occluded as soon
/// as you switch away, so the app's timers are suspended and the plot stops
/// updating — which is why the freeze does not happen to a normal window.
///
/// `beginActivityWithOptions:reason:` returns a token that must stay alive for
/// as long as the activity should last; it is retained and deliberately leaked,
/// since that is the whole run of the program.
pub fn disable_app_nap() {
    use objc::runtime::Object;
    use objc::{class, msg_send, sel, sel_impl};

    // NSActivityOptions, from NSProcessInfo.h.
    const IDLE_DISPLAY_SLEEP_DISABLED: u64 = 1 << 40;
    const IDLE_SYSTEM_SLEEP_DISABLED: u64 = 1 << 20;
    const USER_INITIATED: u64 = 0x00FF_FFFF | IDLE_SYSTEM_SLEEP_DISABLED;
    const LATENCY_CRITICAL: u64 = 0x0000_00FF_0000_0000;

    let options = USER_INITIATED | LATENCY_CRITICAL | IDLE_DISPLAY_SLEEP_DISABLED;

    unsafe {
        let process_info: *mut Object = msg_send![class!(NSProcessInfo), processInfo];
        if process_info.is_null() {
            return;
        }

        let reason: *mut Object = msg_send![
            class!(NSString),
            stringWithUTF8String: b"realtime audio visualisation\0".as_ptr()
                as *const std::os::raw::c_char
        ];

        // Messaging nil is a no-op in Objective-C, so a null token is harmless.
        let token: *mut Object =
            msg_send![process_info, beginActivityWithOptions: options reason: reason];
        let _: *mut Object = msg_send![token, retain];
    }
}
