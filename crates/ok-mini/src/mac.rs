//! The three things asked of AppKit directly, through the Objective-C
//! runtime: how many pixels a point is, full screen, and making Quit close
//! the window (so the text is put on disk first) instead of ending the
//! process on the spot.

use std::ffi::{c_char, c_void, CStr};

type Id = *mut c_void;
type Sel = *mut c_void;

#[link(name = "objc")]
extern "C" {
    fn objc_getClass(name: *const c_char) -> Id;
    fn sel_registerName(name: *const c_char) -> Sel;
    fn objc_msgSend();
}

fn sel(name: &CStr) -> Sel {
    // SAFETY: a nul-terminated selector name.
    unsafe { sel_registerName(name.as_ptr()) }
}

/// `[receiver name]` returning an object or a pointer-sized value.
unsafe fn send_id(receiver: Id, name: &CStr) -> Id {
    let send: unsafe extern "C" fn(Id, Sel) -> Id = std::mem::transmute(objc_msgSend as *const ());
    send(receiver, sel(name))
}

/// `[receiver name]` returning a CGFloat.
unsafe fn send_f64(receiver: Id, name: &CStr) -> f64 {
    let send: unsafe extern "C" fn(Id, Sel) -> f64 = std::mem::transmute(objc_msgSend as *const ());
    send(receiver, sel(name))
}

/// `[receiver name:argument]`.
unsafe fn send_with(receiver: Id, name: &CStr, argument: Id) -> Id {
    let send: unsafe extern "C" fn(Id, Sel, Id) -> Id = std::mem::transmute(objc_msgSend as *const ());
    send(receiver, sel(name), argument)
}

/// `[receiver name:index]` with an integer argument.
unsafe fn send_index(receiver: Id, name: &CStr, index: isize) -> Id {
    let send: unsafe extern "C" fn(Id, Sel, isize) -> Id = std::mem::transmute(objc_msgSend as *const ());
    send(receiver, sel(name), index)
}

/// Pixels to a point on the screen the window is on: 2 on a Retina screen.
pub(crate) fn density(window: *mut c_void) -> f64 {
    if window.is_null() {
        return 1.0;
    }
    // SAFETY: `window` is minifb's live NSWindow.
    let scale = unsafe { send_f64(window, c"backingScaleFactor") };
    if scale.is_finite() && (1.0..=4.0).contains(&scale) {
        scale
    } else {
        1.0
    }
}

/// Puts the window into the Mac's own full screen, or takes it out.
pub(crate) fn set_fullscreen(window: *mut c_void, on: bool) {
    if window.is_null() {
        return;
    }
    const FULL_SCREEN_MASK: usize = 1 << 14;
    // SAFETY: `window` is minifb's live NSWindow; styleMask is an integer.
    unsafe {
        let mask = send_id(window, c"styleMask") as usize;
        if (mask & FULL_SCREEN_MASK != 0) != on {
            send_with(window, c"toggleFullScreen:", std::ptr::null_mut());
        }
    }
}

/// Points every Quit in the menu bar at closing the window instead, so that
/// Cmd Q is heard as a close and the program finishes before it goes.
pub(crate) fn quit_by_closing() {
    // SAFETY: plain messages to AppKit objects, each checked for nil.
    unsafe {
        let app = send_id(objc_getClass(c"NSApplication".as_ptr()), c"sharedApplication");
        if app.is_null() {
            return;
        }
        let bar = send_id(app, c"mainMenu");
        if bar.is_null() {
            return;
        }
        let terminate = sel(c"terminate:");
        let menus = send_id(bar, c"numberOfItems") as isize;
        for m in 0..menus {
            let menu = send_id(send_index(bar, c"itemAtIndex:", m), c"submenu");
            if menu.is_null() {
                continue;
            }
            let items = send_id(menu, c"numberOfItems") as isize;
            for i in 0..items {
                let item = send_index(menu, c"itemAtIndex:", i);
                if !item.is_null() && send_id(item, c"action") == terminate {
                    send_with(item, c"setAction:", sel(c"performClose:"));
                }
            }
        }
    }
}
