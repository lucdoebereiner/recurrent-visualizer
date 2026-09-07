//! macOS specific workarounds.

use cocoa::appkit::{NSScreen, NSWindow};
use cocoa::base::{id, YES};
use cocoa::foundation::{NSPoint, NSRect, NSSize};
use std::os::raw::c_void;

/// `NSWindow` level that sits just above the menu bar (`NSMainMenuWindowLevel`
/// is 24). A window at this level covers the menu bar whatever the application
/// presentation options happen to be.
pub const LEVEL_ABOVE_MENU_BAR: i64 = 25;
/// `NSNormalWindowLevel`.
pub const LEVEL_NORMAL: i64 = 0;

/// Sets the window's level, used to lift a fullscreen window over the menu bar
/// and to drop it back when the app is not in front.
pub fn set_window_level(ns_window: *mut c_void, level: i64) {
    use objc::runtime::Object;
    use objc::{msg_send, sel, sel_impl};

    let window = ns_window as *mut Object;
    if window.is_null() {
        return;
    }
    unsafe {
        let _: () = msg_send![window, setLevel: level];
    }
}

/// Brings the process to the front.
///
/// Presentation options are only honoured for the *active* application, and
/// winit notes that activation is unreliable for an unbundled binary — which
/// this is, when run straight from a terminal.
pub fn activate() {
    use objc::runtime::{Object, YES};
    use objc::{class, msg_send, sel, sel_impl};

    unsafe {
        let app: *mut Object = msg_send![class!(NSApplication), sharedApplication];
        if app.is_null() {
            return;
        }
        let _: () = msg_send![app, activateIgnoringOtherApps: YES];
    }
}

/// Puts the menu bar and Dock back (`NSApplicationPresentationDefault`).
///
/// winit used to restore these when leaving its own simple fullscreen; the
/// fullscreen here is done by hand, so the restore is too.
pub fn restore_presentation_options() {
    use objc::runtime::Object;
    use objc::{class, msg_send, sel, sel_impl};

    unsafe {
        let app: *mut Object = msg_send![class!(NSApplication), sharedApplication];
        if app.is_null() {
            return;
        }
        let _: () = msg_send![app, setPresentationOptions: 0u64];
    }
}

/// Hides the menu bar and the Dock outright.
///
/// winit's simple fullscreen sets only the *auto* hide options, so the menu bar
/// slides back in whenever the pointer nears the top of the screen. For a
/// projected visualisation it should stay gone.
///
/// Call this after entering simple fullscreen: winit saves the presentation
/// options before it changes them, and restores that saved value on exit, so
/// overriding afterwards does not leak into windowed mode.
pub fn hide_menu_bar_and_dock() {
    use objc::runtime::Object;
    use objc::{class, msg_send, sel, sel_impl};

    // NSApplicationPresentationOptions. HideMenuBar is only a legal combination
    // together with HideDock; on its own it raises an exception.
    const HIDE_DOCK: u64 = 1 << 1;
    const HIDE_MENU_BAR: u64 = 1 << 3;

    unsafe {
        let app: *mut Object = msg_send![class!(NSApplication), sharedApplication];
        if app.is_null() {
            return;
        }
        let _: () = msg_send![app, setPresentationOptions: HIDE_DOCK | HIDE_MENU_BAR];
    }
}

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

/// A rectangle in Cocoa points, bottom-left origin — the coordinate system
/// `NSWindow`'s `setFrame:display:` works in, and the one `NSScreen` reports
/// its own geometry in.
///
/// Points are what make this worth having. A point is a point on whichever
/// display the rectangle lands on, so a screen's frame describes the target
/// absolutely, with no scale factor anywhere in the description. Every winit
/// route to the same job converts through the *window's* scale factor as it is
/// at the moment of the call, which is the wrong one whenever the window has
/// not reached the target display yet.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Frame {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

impl Frame {
    fn from_ns(rect: NSRect) -> Frame {
        Frame {
            x: rect.origin.x,
            y: rect.origin.y,
            width: rect.size.width,
            height: rect.size.height,
        }
    }

    fn to_ns(self) -> NSRect {
        NSRect::new(
            NSPoint::new(self.x, self.y),
            NSSize::new(self.width, self.height),
        )
    }

    /// Whether the window has arrived. AppKit rounds frames to the backing
    /// grid, so a fraction of a point out is not a failure.
    pub fn matches(self, other: Frame) -> bool {
        let close = |a: f64, b: f64| (a - b).abs() <= 1.0;
        close(self.x, other.x)
            && close(self.y, other.y)
            && close(self.width, other.width)
            && close(self.height, other.height)
    }

    /// This rectangle's size, centred on `screen`.
    pub fn centered_on(self, screen: Frame) -> Frame {
        Frame {
            x: screen.x + (screen.width - self.width) / 2.0,
            y: screen.y + (screen.height - self.height) / 2.0,
            ..self
        }
    }
}

/// The full frame of a screen, menu bar area included — `visibleFrame` would
/// stop short of it, and the menu bar is hidden anyway.
pub fn screen_frame(ns_screen: *mut c_void) -> Option<Frame> {
    let screen = ns_screen as id;
    if screen.is_null() {
        return None;
    }
    Some(Frame::from_ns(unsafe { NSScreen::frame(screen) }))
}

pub fn window_frame(ns_window: *mut c_void) -> Option<Frame> {
    let window = ns_window as id;
    if window.is_null() {
        return None;
    }
    Some(Frame::from_ns(unsafe { NSWindow::frame(window) }))
}

/// Sets origin and size in one call, which `set_outer_position` followed by
/// `set_inner_size` does not: `setContentSize:` keeps the frame's *bottom*-left
/// corner where it is, so resizing after positioning by the top-left corner
/// drops the window down the screen by the difference.
pub fn set_window_frame(ns_window: *mut c_void, frame: Frame) {
    let window = ns_window as id;
    if window.is_null() {
        return;
    }
    unsafe {
        NSWindow::setFrame_display_(window, frame.to_ns(), YES);
    }
}
