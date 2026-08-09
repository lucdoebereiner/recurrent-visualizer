//! macOS specific workarounds.

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
