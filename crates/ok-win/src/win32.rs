//! Hand-written Win32 bindings for a single-window, software-rendered, keyboard-first
//! text editor. No crates. 64-bit Windows only (the struct layouts below are the x64 ones
//! and are checked by compile-time size assertions).
//!
//! This file began as the Win32 probe that was written and run on the target machine
//! (target\agents\notes\win32-probe). Its declarations (constants, structs, imports) are
//! all kept, used or not, under the one `allow(dead_code)` below: each was link-checked
//! there and declares without doing. Its helpers are kept only where the shell calls
//! them. A helper does something, and one that sits here unused reads as approved for
//! whoever needs it next: the probe's `recycle` could delete for good on a volume with no
//! Recycle Bin, its `shell_open` handed the shell an address without the checks
//! `host::is_web_address` makes, and its `Fullscreen::toggle` and
//! `activate_existing_instance` took the focus. Those and every other helper nothing
//! called are gone; the probe still has them. What the shell added is marked "added for
//! the shell".
//!
//! In order: base types; constants; `#[repr(C)]` structs with their size assertions;
//! imports, one block per import library (every one is link-checked by the test
//! `every_import_resolves` in lib.rs); the LoadLibrary/GetProcAddress layer for what has
//! no import library here (dwmapi, shcore, imm32, uxtheme), where every entry is an
//! Option and degrades; and the helpers that are easy to get subtly wrong (strings,
//! WM_CHAR decoding, the wheel, the frame timer, blitting, glyph rasterising, the
//! clipboard, known folders, icons, single instance, frameless hit-testing, sizing).
//!
//! Handles are `isize` (0 is NULL). That keeps them Copy/Send/comparable and avoids
//! pointer-provenance noise for values that are never dereferenced.

#![allow(non_snake_case, non_camel_case_types, non_upper_case_globals, dead_code)]
#![allow(clippy::upper_case_acronyms, clippy::too_many_arguments, clippy::missing_safety_doc)]

#[cfg(not(all(windows, target_pointer_width = "64")))]
compile_error!("win32.rs assumes 64-bit Windows (x64 struct layouts).");

use core::ffi::c_void;
use core::ptr::{null, null_mut};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

pub type BOOL = i32;
pub type HRESULT = i32;
pub type ATOM = u16;
pub type COLORREF = u32; // 0x00BBGGRR  (note: the opposite byte order from a DIB pixel)
pub type WPARAM = usize;
pub type LPARAM = isize;
pub type LRESULT = isize;
pub type PCWSTR = *const u16;
pub type PWSTR = *mut u16;

pub type HANDLE = isize;
pub type HWND = isize;
pub type HDC = isize;
pub type HINSTANCE = isize;
pub type HMODULE = isize;
pub type HICON = isize;
pub type HCURSOR = isize;
pub type HBRUSH = isize;
pub type HBITMAP = isize;
pub type HFONT = isize;
pub type HGDIOBJ = isize;
pub type HRGN = isize;
pub type HMENU = isize;
pub type HMONITOR = isize;
pub type HGLOBAL = isize;
pub type HKL = isize;
pub type HKEY = isize;
pub type HIMC = isize;
pub type HDROP = isize;
pub type DPI_AWARENESS_CONTEXT = isize;

pub type WNDPROC = Option<unsafe extern "system" fn(HWND, u32, WPARAM, LPARAM) -> LRESULT>;
pub type TIMERPROC = Option<unsafe extern "system" fn(HWND, u32, usize, u32)>;
pub type MONITORENUMPROC = Option<unsafe extern "system" fn(HMONITOR, HDC, *mut RECT, LPARAM) -> BOOL>;
/// The pointers really are ENUMLOGFONTEXW / NEWTEXTMETRICEXW for EnumFontFamiliesExW.
pub type FONTENUMPROCW = Option<unsafe extern "system" fn(*const LOGFONTW, *const TEXTMETRICW, u32, LPARAM) -> i32>;

pub const TRUE: BOOL = 1;
pub const FALSE: BOOL = 0;
pub const INVALID_HANDLE_VALUE: HANDLE = -1;

pub const fn make_int_resource(id: u16) -> PCWSTR {
    id as usize as PCWSTR
}
pub const fn rgb(r: u8, g: u8, b: u8) -> COLORREF {
    (r as u32) | ((g as u32) << 8) | ((b as u32) << 16)
}

// window styles
pub const WS_OVERLAPPED: u32 = 0x0000_0000;
pub const WS_POPUP: u32 = 0x8000_0000;
pub const WS_CHILD: u32 = 0x4000_0000;
pub const WS_MINIMIZE: u32 = 0x2000_0000;
pub const WS_VISIBLE: u32 = 0x1000_0000;
pub const WS_DISABLED: u32 = 0x0800_0000;
pub const WS_CLIPSIBLINGS: u32 = 0x0400_0000;
pub const WS_CLIPCHILDREN: u32 = 0x0200_0000;
pub const WS_MAXIMIZE: u32 = 0x0100_0000;
pub const WS_CAPTION: u32 = 0x00C0_0000;
pub const WS_BORDER: u32 = 0x0080_0000;
pub const WS_DLGFRAME: u32 = 0x0040_0000;
pub const WS_SYSMENU: u32 = 0x0008_0000;
pub const WS_THICKFRAME: u32 = 0x0004_0000;
pub const WS_MINIMIZEBOX: u32 = 0x0002_0000;
pub const WS_MAXIMIZEBOX: u32 = 0x0001_0000;
pub const WS_OVERLAPPEDWINDOW: u32 = 0x00CF_0000;

pub const WS_EX_TOPMOST: u32 = 0x0000_0008;
pub const WS_EX_ACCEPTFILES: u32 = 0x0000_0010;
pub const WS_EX_TOOLWINDOW: u32 = 0x0000_0080;
pub const WS_EX_APPWINDOW: u32 = 0x0004_0000;
pub const WS_EX_LAYERED: u32 = 0x0008_0000;
pub const WS_EX_NOREDIRECTIONBITMAP: u32 = 0x0020_0000; // NOT for a GDI-blitted window
pub const WS_EX_COMPOSITED: u32 = 0x0200_0000; // NOT needed: one blit per frame is already atomic
pub const WS_EX_NOACTIVATE: u32 = 0x0800_0000;

// class styles
pub const CS_VREDRAW: u32 = 0x0001;
pub const CS_HREDRAW: u32 = 0x0002;
pub const CS_DBLCLKS: u32 = 0x0008;
pub const CS_OWNDC: u32 = 0x0020;

pub const CW_USEDEFAULT: i32 = 0x8000_0000u32 as i32;

// messages
pub const WM_NULL: u32 = 0x0000;
pub const WM_CREATE: u32 = 0x0001;
pub const WM_DESTROY: u32 = 0x0002;
pub const WM_MOVE: u32 = 0x0003;
pub const WM_SIZE: u32 = 0x0005;
pub const WM_ACTIVATE: u32 = 0x0006;
pub const WM_SETFOCUS: u32 = 0x0007;
pub const WM_KILLFOCUS: u32 = 0x0008;
pub const WM_SETTEXT: u32 = 0x000C;
pub const WM_PAINT: u32 = 0x000F;
pub const WM_CLOSE: u32 = 0x0010;
pub const WM_QUERYENDSESSION: u32 = 0x0011;
pub const WM_QUIT: u32 = 0x0012;
pub const WM_ERASEBKGND: u32 = 0x0014;
pub const WM_ENDSESSION: u32 = 0x0016;
pub const WM_SHOWWINDOW: u32 = 0x0018;
pub const WM_SETTINGCHANGE: u32 = 0x001A;
pub const WM_ACTIVATEAPP: u32 = 0x001C;
pub const WM_FONTCHANGE: u32 = 0x001D;
pub const WM_SETCURSOR: u32 = 0x0020;
pub const WM_MOUSEACTIVATE: u32 = 0x0021;
pub const WM_GETMINMAXINFO: u32 = 0x0024;
pub const WM_WINDOWPOSCHANGING: u32 = 0x0046;
pub const WM_WINDOWPOSCHANGED: u32 = 0x0047;
pub const WM_COPYDATA: u32 = 0x004A;
pub const WM_INPUTLANGCHANGE: u32 = 0x0051;
pub const WM_CONTEXTMENU: u32 = 0x007B;
pub const WM_DISPLAYCHANGE: u32 = 0x007E;
pub const WM_GETICON: u32 = 0x007F;
pub const WM_SETICON: u32 = 0x0080;
pub const WM_NCCREATE: u32 = 0x0081;
pub const WM_NCDESTROY: u32 = 0x0082;
pub const WM_NCCALCSIZE: u32 = 0x0083;
pub const WM_NCHITTEST: u32 = 0x0084;
pub const WM_NCPAINT: u32 = 0x0085;
pub const WM_NCACTIVATE: u32 = 0x0086;
pub const WM_NCMOUSEMOVE: u32 = 0x00A0;
pub const WM_NCLBUTTONDOWN: u32 = 0x00A1;
pub const WM_NCLBUTTONUP: u32 = 0x00A2;
pub const WM_NCLBUTTONDBLCLK: u32 = 0x00A3;
pub const WM_NCRBUTTONUP: u32 = 0x00A5;
pub const WM_KEYDOWN: u32 = 0x0100;
pub const WM_KEYUP: u32 = 0x0101;
pub const WM_CHAR: u32 = 0x0102;
pub const WM_DEADCHAR: u32 = 0x0103;
pub const WM_SYSKEYDOWN: u32 = 0x0104;
pub const WM_SYSKEYUP: u32 = 0x0105;
pub const WM_SYSCHAR: u32 = 0x0106;
pub const WM_SYSDEADCHAR: u32 = 0x0107;
pub const WM_UNICHAR: u32 = 0x0109;
pub const WM_IME_STARTCOMPOSITION: u32 = 0x010D;
pub const WM_IME_ENDCOMPOSITION: u32 = 0x010E;
pub const WM_IME_COMPOSITION: u32 = 0x010F;
pub const WM_COMMAND: u32 = 0x0111;
pub const WM_SYSCOMMAND: u32 = 0x0112;
pub const WM_TIMER: u32 = 0x0113;
pub const WM_ENTERIDLE: u32 = 0x0121;
pub const WM_MOUSEMOVE: u32 = 0x0200;
pub const WM_LBUTTONDOWN: u32 = 0x0201;
pub const WM_LBUTTONUP: u32 = 0x0202;
pub const WM_LBUTTONDBLCLK: u32 = 0x0203; // only with CS_DBLCLKS
pub const WM_RBUTTONDOWN: u32 = 0x0204;
pub const WM_RBUTTONUP: u32 = 0x0205;
pub const WM_RBUTTONDBLCLK: u32 = 0x0206;
pub const WM_MBUTTONDOWN: u32 = 0x0207;
pub const WM_MBUTTONUP: u32 = 0x0208;
pub const WM_MBUTTONDBLCLK: u32 = 0x0209;
pub const WM_MOUSEWHEEL: u32 = 0x020A; // coordinates in lParam are SCREEN coordinates
pub const WM_XBUTTONDOWN: u32 = 0x020B;
pub const WM_XBUTTONUP: u32 = 0x020C;
pub const WM_XBUTTONDBLCLK: u32 = 0x020D;
pub const WM_MOUSEHWHEEL: u32 = 0x020E; // positive delta = scroll RIGHT
pub const WM_ENTERMENULOOP: u32 = 0x0211;
pub const WM_EXITMENULOOP: u32 = 0x0212;
pub const WM_SIZING: u32 = 0x0214;
pub const WM_CAPTURECHANGED: u32 = 0x0215;
pub const WM_MOVING: u32 = 0x0216;
pub const WM_POWERBROADCAST: u32 = 0x0218;
pub const WM_ENTERSIZEMOVE: u32 = 0x0231;
pub const WM_EXITSIZEMOVE: u32 = 0x0232;
pub const WM_DROPFILES: u32 = 0x0233;
pub const WM_IME_SETCONTEXT: u32 = 0x0281;
pub const WM_IME_NOTIFY: u32 = 0x0282;
pub const WM_IME_CHAR: u32 = 0x0286;
pub const WM_NCMOUSELEAVE: u32 = 0x02A2;
pub const WM_MOUSEHOVER: u32 = 0x02A1;
pub const WM_MOUSELEAVE: u32 = 0x02A3;
pub const WM_DPICHANGED: u32 = 0x02E0;
pub const WM_GETDPISCALEDSIZE: u32 = 0x02E4;
pub const WM_CUT: u32 = 0x0300;
pub const WM_COPY: u32 = 0x0301;
pub const WM_PASTE: u32 = 0x0302;
pub const WM_THEMECHANGED: u32 = 0x031A;
pub const WM_CLIPBOARDUPDATE: u32 = 0x031D;
pub const WM_DWMCOMPOSITIONCHANGED: u32 = 0x031E;
pub const WM_DWMCOLORIZATIONCOLORCHANGED: u32 = 0x0320;
pub const WM_USER: u32 = 0x0400;
pub const WM_APP: u32 = 0x8000;

pub const UNICODE_NOCHAR: usize = 0xFFFF; // WM_UNICHAR capability probe

// WM_SIZE wParam
pub const SIZE_RESTORED: usize = 0;
pub const SIZE_MINIMIZED: usize = 1;
pub const SIZE_MAXIMIZED: usize = 2;
// WM_ACTIVATE LOWORD(wParam)
pub const WA_INACTIVE: u32 = 0;
pub const WA_ACTIVE: u32 = 1;
pub const WA_CLICKACTIVE: u32 = 2;
// WM_SIZING wParam
pub const WMSZ_LEFT: usize = 1;
pub const WMSZ_RIGHT: usize = 2;
pub const WMSZ_TOP: usize = 3;
pub const WMSZ_TOPLEFT: usize = 4;
pub const WMSZ_TOPRIGHT: usize = 5;
pub const WMSZ_BOTTOM: usize = 6;
pub const WMSZ_BOTTOMLEFT: usize = 7;
pub const WMSZ_BOTTOMRIGHT: usize = 8;
// WM_SYSCOMMAND (wParam & 0xFFF0)
pub const SC_SIZE: usize = 0xF000;
pub const SC_MOVE: usize = 0xF010;
pub const SC_MINIMIZE: usize = 0xF020;
pub const SC_MAXIMIZE: usize = 0xF030;
pub const SC_CLOSE: usize = 0xF060;
pub const SC_MOUSEMENU: usize = 0xF090;
pub const SC_KEYMENU: usize = 0xF100;
pub const SC_RESTORE: usize = 0xF120;
pub const SC_SCREENSAVE: usize = 0xF140;
pub const SC_MONITORPOWER: usize = 0xF170;
// WM_NCHITTEST results
pub const HTERROR: isize = -2;
pub const HTTRANSPARENT: isize = -1;
pub const HTNOWHERE: isize = 0;
pub const HTCLIENT: isize = 1;
pub const HTCAPTION: isize = 2;
pub const HTSYSMENU: isize = 3;
pub const HTMINBUTTON: isize = 8;
pub const HTMAXBUTTON: isize = 9;
pub const HTLEFT: isize = 10;
pub const HTRIGHT: isize = 11;
pub const HTTOP: isize = 12;
pub const HTTOPLEFT: isize = 13;
pub const HTTOPRIGHT: isize = 14;
pub const HTBOTTOM: isize = 15;
pub const HTBOTTOMLEFT: isize = 16;
pub const HTBOTTOMRIGHT: isize = 17;
pub const HTBORDER: isize = 18;
pub const HTCLOSE: isize = 20;
// WM_NCCALCSIZE return flags
pub const WVR_ALIGNTOP: isize = 0x0010;
pub const WVR_ALIGNLEFT: isize = 0x0020;
pub const WVR_HREDRAW: isize = 0x0100;
pub const WVR_VREDRAW: isize = 0x0200;
pub const WVR_VALIDRECTS: isize = 0x0400;
// mouse key flags (LOWORD(wParam) of mouse messages)
pub const MK_LBUTTON: u32 = 0x0001;
pub const MK_RBUTTON: u32 = 0x0002;
pub const MK_SHIFT: u32 = 0x0004;
pub const MK_CONTROL: u32 = 0x0008;
pub const MK_MBUTTON: u32 = 0x0010;
pub const MK_XBUTTON1: u32 = 0x0020;
pub const MK_XBUTTON2: u32 = 0x0040;
pub const WHEEL_DELTA: i32 = 120;
pub const WHEEL_PAGESCROLL: u32 = u32::MAX;
pub const XBUTTON1: u32 = 1;
pub const XBUTTON2: u32 = 2;
// power / session
pub const PBT_APMSUSPEND: usize = 0x0004;
pub const PBT_APMRESUMESUSPEND: usize = 0x0007;
pub const PBT_APMRESUMEAUTOMATIC: usize = 0x0012;
pub const ENDSESSION_CLOSEAPP: isize = 0x0000_0001;
pub const ENDSESSION_CRITICAL: isize = 0x4000_0000;
pub const ENDSESSION_LOGOFF: isize = 0x8000_0000u32 as i32 as isize;

// ShowWindow
pub const SW_HIDE: i32 = 0;
pub const SW_SHOWNORMAL: i32 = 1;
pub const SW_SHOWMINIMIZED: i32 = 2;
pub const SW_SHOWMAXIMIZED: i32 = 3;
pub const SW_MAXIMIZE: i32 = 3;
pub const SW_SHOWNOACTIVATE: i32 = 4;
pub const SW_SHOW: i32 = 5;
pub const SW_MINIMIZE: i32 = 6;
pub const SW_SHOWMINNOACTIVE: i32 = 7;
pub const SW_SHOWNA: i32 = 8;
pub const SW_RESTORE: i32 = 9;
pub const SW_SHOWDEFAULT: i32 = 10;
// SetWindowPos
pub const SWP_NOSIZE: u32 = 0x0001;
pub const SWP_NOMOVE: u32 = 0x0002;
pub const SWP_NOZORDER: u32 = 0x0004;
pub const SWP_NOREDRAW: u32 = 0x0008;
pub const SWP_NOACTIVATE: u32 = 0x0010;
pub const SWP_FRAMECHANGED: u32 = 0x0020;
pub const SWP_SHOWWINDOW: u32 = 0x0040;
pub const SWP_HIDEWINDOW: u32 = 0x0080;
pub const SWP_NOCOPYBITS: u32 = 0x0100;
pub const SWP_NOOWNERZORDER: u32 = 0x0200;
pub const SWP_NOSENDCHANGING: u32 = 0x0400;
pub const HWND_TOP: HWND = 0;
pub const HWND_BOTTOM: HWND = 1;
pub const HWND_TOPMOST: HWND = -1;
pub const HWND_NOTOPMOST: HWND = -2;
pub const HWND_MESSAGE: HWND = -3;
// Get/SetWindowLongPtr, Get/SetClassLongPtr
pub const GWLP_WNDPROC: i32 = -4;
pub const GWLP_HINSTANCE: i32 = -6;
pub const GWL_STYLE: i32 = -16;
pub const GWL_EXSTYLE: i32 = -20;
pub const GWLP_USERDATA: i32 = -21;
pub const GCLP_HBRBACKGROUND: i32 = -10;
pub const GCLP_HCURSOR: i32 = -12;
pub const GCLP_HICON: i32 = -14;
pub const GCLP_HICONSM: i32 = -34;
// PeekMessage / MsgWait
pub const PM_NOREMOVE: u32 = 0;
pub const PM_REMOVE: u32 = 1;
pub const QS_ALLEVENTS: u32 = 0x04BF;
pub const QS_ALLINPUT: u32 = 0x04FF;
pub const MWMO_ALERTABLE: u32 = 0x0002;
pub const MWMO_INPUTAVAILABLE: u32 = 0x0004;
pub const WAIT_OBJECT_0: u32 = 0;
pub const WAIT_TIMEOUT: u32 = 258;
pub const WAIT_FAILED: u32 = 0xFFFF_FFFF;
pub const INFINITE: u32 = 0xFFFF_FFFF;
pub const SMTO_ABORTIFHUNG: u32 = 0x0002;
pub const USER_TIMER_MINIMUM: u32 = 0x0000_000A;
// RedrawWindow
pub const RDW_INVALIDATE: u32 = 0x0001;
pub const RDW_INTERNALPAINT: u32 = 0x0002;
pub const RDW_ERASE: u32 = 0x0004;
pub const RDW_NOERASE: u32 = 0x0020;
pub const RDW_UPDATENOW: u32 = 0x0100;
pub const RDW_FRAME: u32 = 0x0400;
// GetSystemMetrics
pub const SM_CXSCREEN: i32 = 0;
pub const SM_CYSCREEN: i32 = 1;
pub const SM_CYCAPTION: i32 = 4;
pub const SM_CXICON: i32 = 11;
pub const SM_CYICON: i32 = 12;
pub const SM_CXFRAME: i32 = 32; // a.k.a. SM_CXSIZEFRAME
pub const SM_CYFRAME: i32 = 33;
pub const SM_CXMINTRACK: i32 = 34;
pub const SM_CYMINTRACK: i32 = 35;
pub const SM_CXDOUBLECLK: i32 = 36;
pub const SM_CYDOUBLECLK: i32 = 37;
pub const SM_CXSMICON: i32 = 49;
pub const SM_CYSMICON: i32 = 50;
pub const SM_CXDRAG: i32 = 68;
pub const SM_CYDRAG: i32 = 69;
pub const SM_XVIRTUALSCREEN: i32 = 76;
pub const SM_YVIRTUALSCREEN: i32 = 77;
pub const SM_CXVIRTUALSCREEN: i32 = 78;
pub const SM_CYVIRTUALSCREEN: i32 = 79;
pub const SM_CMONITORS: i32 = 80;
pub const SM_CXPADDEDBORDER: i32 = 92;
pub const SM_REMOTESESSION: i32 = 0x1000;
// SystemParametersInfo
pub const SPI_GETKEYBOARDSPEED: u32 = 0x000A;
pub const SPI_GETKEYBOARDDELAY: u32 = 0x0016;
pub const SPI_GETWORKAREA: u32 = 0x0030;
pub const SPI_GETWHEELSCROLLLINES: u32 = 0x0068;
pub const SPI_GETWHEELSCROLLCHARS: u32 = 0x006C;
pub const SPI_GETMOUSEVANISH: u32 = 0x1020; // the user's "hide pointer while typing" setting
pub const SPI_GETCLIENTAREAANIMATION: u32 = 0x1042; // the user's "show animations" setting
pub const SPI_GETCARETWIDTH: u32 = 0x2006;
// monitors
pub const MONITOR_DEFAULTTONULL: u32 = 0;
pub const MONITOR_DEFAULTTOPRIMARY: u32 = 1;
pub const MONITOR_DEFAULTTONEAREST: u32 = 2;
pub const MONITORINFOF_PRIMARY: u32 = 1;
pub const MDT_EFFECTIVE_DPI: u32 = 0;
// cursors / icons
pub const IDC_ARROW: PCWSTR = make_int_resource(32512);
pub const IDC_IBEAM: PCWSTR = make_int_resource(32513);
pub const IDC_WAIT: PCWSTR = make_int_resource(32514);
pub const IDC_CROSS: PCWSTR = make_int_resource(32515);
pub const IDC_SIZENWSE: PCWSTR = make_int_resource(32642);
pub const IDC_SIZENESW: PCWSTR = make_int_resource(32643);
pub const IDC_SIZEWE: PCWSTR = make_int_resource(32644);
pub const IDC_SIZENS: PCWSTR = make_int_resource(32645);
pub const IDC_SIZEALL: PCWSTR = make_int_resource(32646);
pub const IDC_NO: PCWSTR = make_int_resource(32648);
pub const IDC_HAND: PCWSTR = make_int_resource(32649);
pub const IDI_APPLICATION: PCWSTR = make_int_resource(32512);
pub const ICON_SMALL: usize = 0;
pub const ICON_BIG: usize = 1;
pub const IMAGE_ICON: u32 = 1;
pub const LR_DEFAULTCOLOR: u32 = 0x0000;
pub const LR_LOADFROMFILE: u32 = 0x0010;
pub const LR_DEFAULTSIZE: u32 = 0x0040;
pub const LR_SHARED: u32 = 0x8000;
pub const DI_NORMAL: u32 = 0x0003;
pub const CURSOR_SHOWING: u32 = 0x0001;
// TrackMouseEvent
pub const TME_HOVER: u32 = 0x0001;
pub const TME_LEAVE: u32 = 0x0002;
pub const TME_NONCLIENT: u32 = 0x0010;
pub const HOVER_DEFAULT: u32 = 0xFFFF_FFFF;
// clipboard / global memory
pub const CF_TEXT: u32 = 1;
pub const CF_UNICODETEXT: u32 = 13;
pub const CF_HDROP: u32 = 15;
pub const GMEM_MOVEABLE: u32 = 0x0002;
pub const GMEM_ZEROINIT: u32 = 0x0040;
// MessageBeep / MessageBox
pub const MB_OK: u32 = 0x0000;
pub const MB_ICONERROR: u32 = 0x0010;
pub const MB_ICONWARNING: u32 = 0x0030;
pub const MB_ICONINFORMATION: u32 = 0x0040;
pub const MB_SIMPLE_BEEP: u32 = 0xFFFF_FFFF;
// FlashWindowEx
pub const FLASHW_STOP: u32 = 0;
pub const FLASHW_CAPTION: u32 = 1;
pub const FLASHW_TRAY: u32 = 2;
pub const FLASHW_TIMERNOFG: u32 = 12;

// virtual keys ('0'..'9' are 0x30..0x39 and 'A'..'Z' are 0x41..0x5A: use b'A' as i32)
pub const VK_BACK: i32 = 0x08;
pub const VK_TAB: i32 = 0x09;
pub const VK_RETURN: i32 = 0x0D;
pub const VK_SHIFT: i32 = 0x10;
pub const VK_CONTROL: i32 = 0x11;
pub const VK_MENU: i32 = 0x12; // Alt
pub const VK_PAUSE: i32 = 0x13;
pub const VK_CAPITAL: i32 = 0x14;
pub const VK_ESCAPE: i32 = 0x1B;
pub const VK_SPACE: i32 = 0x20;
pub const VK_PRIOR: i32 = 0x21; // Page Up
pub const VK_NEXT: i32 = 0x22; // Page Down
pub const VK_END: i32 = 0x23;
pub const VK_HOME: i32 = 0x24;
pub const VK_LEFT: i32 = 0x25;
pub const VK_UP: i32 = 0x26;
pub const VK_RIGHT: i32 = 0x27;
pub const VK_DOWN: i32 = 0x28;
pub const VK_SNAPSHOT: i32 = 0x2C;
pub const VK_INSERT: i32 = 0x2D;
pub const VK_DELETE: i32 = 0x2E;
pub const VK_LWIN: i32 = 0x5B;
pub const VK_RWIN: i32 = 0x5C;
pub const VK_APPS: i32 = 0x5D;
pub const VK_NUMPAD0: i32 = 0x60;
pub const VK_MULTIPLY: i32 = 0x6A;
pub const VK_ADD: i32 = 0x6B;
pub const VK_SUBTRACT: i32 = 0x6D;
pub const VK_DECIMAL: i32 = 0x6E;
pub const VK_DIVIDE: i32 = 0x6F;
pub const VK_F1: i32 = 0x70;
pub const VK_F2: i32 = 0x71;
pub const VK_F3: i32 = 0x72;
pub const VK_F4: i32 = 0x73;
pub const VK_F5: i32 = 0x74;
pub const VK_F6: i32 = 0x75;
pub const VK_F7: i32 = 0x76;
pub const VK_F8: i32 = 0x77;
pub const VK_F9: i32 = 0x78;
pub const VK_F10: i32 = 0x79;
pub const VK_F11: i32 = 0x7A;
pub const VK_F12: i32 = 0x7B;
pub const VK_NUMLOCK: i32 = 0x90;
pub const VK_SCROLL: i32 = 0x91;
pub const VK_LSHIFT: i32 = 0xA0;
pub const VK_RSHIFT: i32 = 0xA1;
pub const VK_LCONTROL: i32 = 0xA2;
pub const VK_RCONTROL: i32 = 0xA3;
pub const VK_LMENU: i32 = 0xA4;
pub const VK_RMENU: i32 = 0xA5; // AltGr on layouts that have one
pub const VK_OEM_1: i32 = 0xBA; // ;: on US
pub const VK_OEM_PLUS: i32 = 0xBB;
pub const VK_OEM_COMMA: i32 = 0xBC;
pub const VK_OEM_MINUS: i32 = 0xBD;
pub const VK_OEM_PERIOD: i32 = 0xBE;
pub const VK_OEM_2: i32 = 0xBF; // /? on US
pub const VK_OEM_3: i32 = 0xC0; // `~ on US
pub const VK_OEM_4: i32 = 0xDB; // [{ on US
pub const VK_OEM_5: i32 = 0xDC; // \| on US
pub const VK_OEM_6: i32 = 0xDD; // ]} on US
pub const VK_OEM_7: i32 = 0xDE; // '" on US
pub const VK_OEM_102: i32 = 0xE2;
pub const VK_PROCESSKEY: i32 = 0xE5; // an IME ate the key
pub const VK_PACKET: i32 = 0xE7; // injected Unicode (SendInput KEYEVENTF_UNICODE): never a command; its WM_CHAR follows
pub const MAPVK_VK_TO_VSC: u32 = 0;
pub const MAPVK_VSC_TO_VK: u32 = 1;
pub const MAPVK_VK_TO_CHAR: u32 = 2; // bit 31 set = dead key
pub const MAPVK_VSC_TO_VK_EX: u32 = 3;

// DPI
pub const USER_DEFAULT_SCREEN_DPI: u32 = 96;
pub const DPI_AWARENESS_CONTEXT_UNAWARE: DPI_AWARENESS_CONTEXT = -1;
pub const DPI_AWARENESS_CONTEXT_SYSTEM_AWARE: DPI_AWARENESS_CONTEXT = -2;
pub const DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE: DPI_AWARENESS_CONTEXT = -3;
pub const DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2: DPI_AWARENESS_CONTEXT = -4;
pub const DPI_AWARENESS_CONTEXT_UNAWARE_GDISCALED: DPI_AWARENESS_CONTEXT = -5;

// GDI
pub const SRCCOPY: u32 = 0x00CC_0020;
pub const DIB_RGB_COLORS: u32 = 0;
pub const BI_RGB: u32 = 0;
pub const BI_BITFIELDS: u32 = 3;
pub const COLORONCOLOR: i32 = 3; // nearest-neighbour stretch (what an integer-scaled bitmap face wants)
pub const HALFTONE: i32 = 4;
pub const GDI_ERROR: u32 = 0xFFFF_FFFF;
pub const CLR_INVALID: COLORREF = 0xFFFF_FFFF;
pub const TRANSPARENT: i32 = 1;
pub const OPAQUE: i32 = 2;
pub const WHITE_BRUSH: i32 = 0;
pub const BLACK_BRUSH: i32 = 4;
pub const NULL_BRUSH: i32 = 5;
pub const HORZRES: i32 = 8;
pub const VERTRES: i32 = 10;
pub const BITSPIXEL: i32 = 12;
pub const LOGPIXELSX: i32 = 88;
pub const LOGPIXELSY: i32 = 90;
pub const VREFRESH: i32 = 116;
// fonts
pub const LF_FACESIZE: usize = 32;
pub const LF_FULLFACESIZE: usize = 64;
pub const FW_NORMAL: i32 = 400;
pub const FW_MEDIUM: i32 = 500;
pub const FW_SEMIBOLD: i32 = 600;
pub const FW_BOLD: i32 = 700;
pub const ANSI_CHARSET: u32 = 0;
pub const DEFAULT_CHARSET: u32 = 1;
pub const SYMBOL_CHARSET: u32 = 2;
pub const OEM_CHARSET: u32 = 255;
pub const OUT_DEFAULT_PRECIS: u32 = 0;
pub const OUT_TT_PRECIS: u32 = 4;
pub const OUT_TT_ONLY_PRECIS: u32 = 7;
pub const OUT_OUTLINE_PRECIS: u32 = 8;
pub const CLIP_DEFAULT_PRECIS: u32 = 0;
pub const DEFAULT_QUALITY: u32 = 0;
pub const NONANTIALIASED_QUALITY: u32 = 3;
pub const ANTIALIASED_QUALITY: u32 = 4;
pub const CLEARTYPE_QUALITY: u32 = 5;
pub const DEFAULT_PITCH: u32 = 0;
pub const FIXED_PITCH: u32 = 1; // LOGFONT.lfPitchAndFamily & 3
pub const VARIABLE_PITCH: u32 = 2;
pub const FF_DONTCARE: u32 = 0x00;
pub const FF_MODERN: u32 = 0x30;
/// TEXTMETRIC.tmPitchAndFamily bit 0. Misnamed by history: SET means the face is VARIABLE pitch.
pub const TMPF_FIXED_PITCH: u8 = 0x01;
pub const TMPF_VECTOR: u8 = 0x02;
pub const TMPF_TRUETYPE: u8 = 0x04;
pub const RASTER_FONTTYPE: u32 = 0x0001;
pub const DEVICE_FONTTYPE: u32 = 0x0002;
pub const TRUETYPE_FONTTYPE: u32 = 0x0004;
pub const FR_PRIVATE: u32 = 0x10;
pub const FR_NOT_ENUM: u32 = 0x20;
pub const GGO_METRICS: u32 = 0;
pub const GGO_BITMAP: u32 = 1; // 1 bpp, rows padded to 32 bits, MSB first
pub const GGO_NATIVE: u32 = 2;
pub const GGO_GRAY2_BITMAP: u32 = 4;
pub const GGO_GRAY4_BITMAP: u32 = 5;
pub const GGO_GRAY8_BITMAP: u32 = 6; // one byte per pixel, values 0..=64 (65 levels), rows padded to 4 bytes
pub const GGO_GLYPH_INDEX: u32 = 0x0080;
pub const GGO_UNHINTED: u32 = 0x0100;
pub const GGI_MARK_NONEXISTING_GLYPHS: u32 = 0x0001; // missing glyphs come back as 0xFFFF

// kernel32
pub const ERROR_SUCCESS: u32 = 0;
pub const ERROR_FILE_NOT_FOUND: u32 = 2;
pub const ERROR_PATH_NOT_FOUND: u32 = 3;
pub const ERROR_ACCESS_DENIED: u32 = 5;
pub const ERROR_ALREADY_EXISTS: u32 = 183;
pub const ERROR_FILENAME_EXCED_RANGE: u32 = 206;
pub const INVALID_FILE_ATTRIBUTES: u32 = 0xFFFF_FFFF;
pub const LOAD_LIBRARY_SEARCH_SYSTEM32: u32 = 0x0000_0800;
pub const CREATE_WAITABLE_TIMER_MANUAL_RESET: u32 = 0x0000_0001;
pub const CREATE_WAITABLE_TIMER_HIGH_RESOLUTION: u32 = 0x0000_0002; // Windows 10 1803+
pub const TIMER_ALL_ACCESS: u32 = 0x001F_0003;
pub const MOVEFILE_REPLACE_EXISTING: u32 = 0x1;
pub const MOVEFILE_WRITE_THROUGH: u32 = 0x8;
pub const REPLACEFILE_WRITE_THROUGH: u32 = 0x1;
pub const REPLACEFILE_IGNORE_MERGE_ERRORS: u32 = 0x2;
pub const FILE_NOTIFY_CHANGE_FILE_NAME: u32 = 0x01;
pub const FILE_NOTIFY_CHANGE_DIR_NAME: u32 = 0x02;
pub const FILE_NOTIFY_CHANGE_SIZE: u32 = 0x08;
pub const FILE_NOTIFY_CHANGE_LAST_WRITE: u32 = 0x10;
pub const S_OK: HRESULT = 0;
pub const E_INVALIDARG: HRESULT = 0x8007_0057u32 as i32;
pub const E_NOTIMPL: HRESULT = 0x8000_4001u32 as i32;
pub const COINIT_APARTMENTTHREADED: u32 = 0x2;
// resources (for stamping an icon/manifest into the built exe with UpdateResourceW)
pub const RT_ICON: PCWSTR = make_int_resource(3);
pub const RT_GROUP_ICON: PCWSTR = make_int_resource(14);
pub const RT_VERSION: PCWSTR = make_int_resource(16);
pub const RT_MANIFEST: PCWSTR = make_int_resource(24);
// registry
pub const HKEY_CURRENT_USER: HKEY = 0x8000_0001u32 as i32 as isize;
pub const HKEY_LOCAL_MACHINE: HKEY = 0x8000_0002u32 as i32 as isize;
pub const RRF_RT_REG_SZ: u32 = 0x0000_0002;
pub const RRF_RT_REG_DWORD: u32 = 0x0000_0010;

// shell32
pub const FO_MOVE: u32 = 1;
pub const FO_COPY: u32 = 2;
pub const FO_DELETE: u32 = 3;
pub const FO_RENAME: u32 = 4;
pub const FOF_SILENT: u32 = 0x0004;
pub const FOF_NOCONFIRMATION: u32 = 0x0010;
pub const FOF_ALLOWUNDO: u32 = 0x0040; // -> Recycle Bin
pub const FOF_NOCONFIRMMKDIR: u32 = 0x0200;
pub const FOF_NOERRORUI: u32 = 0x0400;
pub const FOF_WANTNUKEWARNING: u32 = 0x4000;
pub const KF_FLAG_DEFAULT: u32 = 0;
pub const KF_FLAG_DONT_VERIFY: u32 = 0x4000;
pub const KF_FLAG_CREATE: u32 = 0x8000;
pub const ABM_GETSTATE: u32 = 4;
pub const ABM_GETAUTOHIDEBAREX: u32 = 11;
pub const ABS_AUTOHIDE: usize = 1;

// DWM window attributes
pub const DWMWA_NCRENDERING_ENABLED: u32 = 1;
pub const DWMWA_TRANSITIONS_FORCEDISABLED: u32 = 3;
pub const DWMWA_EXTENDED_FRAME_BOUNDS: u32 = 9;
pub const DWMWA_CLOAK: u32 = 13;
pub const DWMWA_CLOAKED: u32 = 14;
pub const DWMWA_USE_IMMERSIVE_DARK_MODE_BEFORE_20H1: u32 = 19; // builds 17763..18362
pub const DWMWA_USE_IMMERSIVE_DARK_MODE: u32 = 20; // builds 18985+ (so 19041..19045 and Windows 11)
pub const DWMWA_WINDOW_CORNER_PREFERENCE: u32 = 33; // Windows 11
pub const DWMWA_BORDER_COLOR: u32 = 34; // Windows 11 22000+
pub const DWMWA_CAPTION_COLOR: u32 = 35; // Windows 11 22000+
pub const DWMWA_TEXT_COLOR: u32 = 36; // Windows 11 22000+

// IMM
pub const CFS_POINT: u32 = 0x0002;
pub const CFS_FORCE_POSITION: u32 = 0x0020;
pub const CFS_CANDIDATEPOS: u32 = 0x0040;
pub const CFS_EXCLUDE: u32 = 0x0080;

// known folders
pub const FOLDERID_Documents: GUID = GUID::new(0xFDD39AD0, 0x238F, 0x46AF, [0xAD, 0xB4, 0x6C, 0x85, 0x48, 0x03, 0x69, 0xC7]);
pub const FOLDERID_LocalAppData: GUID = GUID::new(0xF1B32785, 0x6FBA, 0x4FCF, [0x9D, 0x55, 0x7B, 0x8E, 0x7F, 0x15, 0x70, 0x91]);
pub const FOLDERID_RoamingAppData: GUID = GUID::new(0x3EB685DB, 0x65F9, 0x4CF6, [0xA0, 0x3A, 0xE3, 0xEF, 0x65, 0x72, 0x9F, 0x3D]);
pub const FOLDERID_Desktop: GUID = GUID::new(0xB4BFCC3A, 0xDB2C, 0x424C, [0xB0, 0x29, 0x7F, 0xE9, 0x9A, 0x87, 0xC6, 0x41]);
pub const FOLDERID_Profile: GUID = GUID::new(0x5E6C858F, 0x0E22, 0x4760, [0x9A, 0xFE, 0xEA, 0x33, 0x17, 0xB6, 0x71, 0x73]);
pub const FOLDERID_Fonts: GUID = GUID::new(0xFD228CB7, 0xAE11, 0x4AE3, [0x86, 0x4C, 0x16, 0xF3, 0x91, 0x0A, 0xB8, 0xFE]);

// added for the shell
pub const WM_KEYFIRST: u32 = 0x0100;
pub const WM_KEYLAST: u32 = 0x0109;
pub const MA_NOACTIVATE: isize = 3;
pub const PM_NOYIELD: u32 = 0x0002;
pub const ERROR_CLASS_ALREADY_EXISTS: u32 = 1410;
pub const LOAD_LIBRARY_AS_DATAFILE: u32 = 0x0000_0002;
pub const WPF_RESTORETOMAXIMIZED: u32 = 0x0002;
pub const USER_TIMER_MAXIMUM: u32 = 0x7FFF_FFFF;
pub const MB_SETFOREGROUND: u32 = 0x0001_0000;
/// The `cmap` table's tag as GetFontData wants it: the four bytes in file order, read as a
/// little-endian DWORD.
pub const TAG_CMAP: u32 = u32::from_le_bytes(*b"cmap");

/// Implements Default as all-zero for plain-data structs (every field here is valid as zero:
/// integers, raw pointers, isize handles, `Option<fn>`).
macro_rules! zero_default {
    ($($t:ty),* $(,)?) => { $( impl Default for $t { fn default() -> Self { unsafe { core::mem::zeroed() } } } )* };
}
macro_rules! assert_size {
    ($($t:ty = $n:expr),* $(,)?) => { $( const _: () = assert!(core::mem::size_of::<$t>() == $n); )* };
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct POINT {
    pub x: i32,
    pub y: i32,
}
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SIZE {
    pub cx: i32,
    pub cy: i32,
}
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RECT {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}
impl RECT {
    pub const fn w(&self) -> i32 {
        self.right - self.left
    }
    pub const fn h(&self) -> i32 {
        self.bottom - self.top
    }
    pub const fn is_empty(&self) -> bool {
        self.right <= self.left || self.bottom <= self.top
    }
}
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GUID {
    pub data1: u32,
    pub data2: u16,
    pub data3: u16,
    pub data4: [u8; 8],
}
impl GUID {
    pub const fn new(data1: u32, data2: u16, data3: u16, data4: [u8; 8]) -> Self {
        GUID { data1, data2, data3, data4 }
    }
}
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct FILETIME {
    pub dwLowDateTime: u32,
    pub dwHighDateTime: u32,
}
/// added for the shell
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct SYSTEMTIME {
    pub wYear: u16,
    pub wMonth: u16,
    /// 0 is Sunday.
    pub wDayOfWeek: u16,
    pub wDay: u16,
    pub wHour: u16,
    pub wMinute: u16,
    pub wSecond: u16,
    pub wMilliseconds: u16,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct WNDCLASSEXW {
    pub cbSize: u32,
    pub style: u32,
    pub lpfnWndProc: WNDPROC,
    pub cbClsExtra: i32,
    pub cbWndExtra: i32,
    pub hInstance: HINSTANCE,
    pub hIcon: HICON,
    pub hCursor: HCURSOR,
    pub hbrBackground: HBRUSH,
    pub lpszMenuName: PCWSTR,
    pub lpszClassName: PCWSTR,
    pub hIconSm: HICON,
}
#[repr(C)]
#[derive(Clone, Copy)]
pub struct CREATESTRUCTW {
    pub lpCreateParams: *mut c_void,
    pub hInstance: HINSTANCE,
    pub hMenu: HMENU,
    pub hwndParent: HWND,
    pub cy: i32,
    pub cx: i32,
    pub y: i32,
    pub x: i32,
    pub style: i32,
    pub lpszName: PCWSTR,
    pub lpszClass: PCWSTR,
    pub dwExStyle: u32,
}
#[repr(C)]
#[derive(Clone, Copy)]
pub struct MSG {
    pub hwnd: HWND,
    pub message: u32,
    pub wParam: WPARAM,
    pub lParam: LPARAM,
    pub time: u32,
    pub pt: POINT,
}
#[repr(C)]
#[derive(Clone, Copy)]
pub struct PAINTSTRUCT {
    pub hdc: HDC,
    pub fErase: BOOL,
    pub rcPaint: RECT,
    pub fRestore: BOOL,
    pub fIncUpdate: BOOL,
    pub rgbReserved: [u8; 32],
}
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct MINMAXINFO {
    pub ptReserved: POINT,
    pub ptMaxSize: POINT,
    pub ptMaxPosition: POINT,
    pub ptMinTrackSize: POINT,
    pub ptMaxTrackSize: POINT,
}
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct WINDOWPLACEMENT {
    pub length: u32,
    pub flags: u32,
    pub showCmd: u32,
    pub ptMinPosition: POINT,
    pub ptMaxPosition: POINT,
    /// Workspace coordinates (work-area relative), not screen coordinates. Only ever feed it
    /// back to SetWindowPlacement.
    pub rcNormalPosition: RECT,
}
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct WINDOWPOS {
    pub hwnd: HWND,
    pub hwndInsertAfter: HWND,
    pub x: i32,
    pub y: i32,
    pub cx: i32,
    pub cy: i32,
    pub flags: u32,
}
#[repr(C)]
#[derive(Clone, Copy)]
pub struct NCCALCSIZE_PARAMS {
    pub rgrc: [RECT; 3],
    pub lppos: *mut WINDOWPOS,
}
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct MONITORINFO {
    pub cbSize: u32,
    pub rcMonitor: RECT,
    pub rcWork: RECT,
    pub dwFlags: u32,
}
#[repr(C)]
#[derive(Clone, Copy)]
pub struct MONITORINFOEXW {
    pub cbSize: u32,
    pub rcMonitor: RECT,
    pub rcWork: RECT,
    pub dwFlags: u32,
    pub szDevice: [u16; 32],
}
#[repr(C)]
#[derive(Clone, Copy)]
pub struct TRACKMOUSEEVENT {
    pub cbSize: u32,
    pub dwFlags: u32,
    pub hwndTrack: HWND,
    pub dwHoverTime: u32,
}
#[repr(C)]
#[derive(Clone, Copy)]
pub struct CURSORINFO {
    pub cbSize: u32,
    pub flags: u32,
    pub hCursor: HCURSOR,
    pub ptScreenPos: POINT,
}
#[repr(C)]
#[derive(Clone, Copy)]
pub struct FLASHWINFO {
    pub cbSize: u32,
    pub hwnd: HWND,
    pub dwFlags: u32,
    pub uCount: u32,
    pub dwTimeout: u32,
}
#[repr(C)]
#[derive(Clone, Copy)]
pub struct COPYDATASTRUCT {
    pub dwData: usize,
    pub cbData: u32,
    pub lpData: *mut c_void,
}
#[repr(C)]
#[derive(Clone, Copy)]
pub struct ICONINFO {
    pub fIcon: BOOL,
    pub xHotspot: u32,
    pub yHotspot: u32,
    pub hbmMask: HBITMAP,
    pub hbmColor: HBITMAP,
}
#[repr(C)]
#[derive(Clone, Copy)]
pub struct BITMAP {
    pub bmType: i32,
    pub bmWidth: i32,
    pub bmHeight: i32,
    pub bmWidthBytes: i32,
    pub bmPlanes: u16,
    pub bmBitsPixel: u16,
    pub bmBits: *mut c_void,
}
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct BITMAPINFOHEADER {
    pub biSize: u32,
    pub biWidth: i32,
    /// Negative = top-down (row 0 is the top row), which is what a framebuffer wants.
    pub biHeight: i32,
    pub biPlanes: u16,
    pub biBitCount: u16,
    pub biCompression: u32,
    pub biSizeImage: u32,
    pub biXPelsPerMeter: i32,
    pub biYPelsPerMeter: i32,
    pub biClrUsed: u32,
    pub biClrImportant: u32,
}
/// BITMAPINFO with room for the three BI_BITFIELDS masks. For 32 bpp BI_RGB the masks are
/// unused and a pixel is 0x00RRGGBB as a little-endian u32 (bytes B, G, R, x).
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct BITMAPINFO {
    pub bmiHeader: BITMAPINFOHEADER,
    pub bmiColors: [u32; 3],
}
#[repr(C)]
#[derive(Clone, Copy)]
pub struct LOGFONTW {
    pub lfHeight: i32,
    pub lfWidth: i32,
    pub lfEscapement: i32,
    pub lfOrientation: i32,
    pub lfWeight: i32,
    pub lfItalic: u8,
    pub lfUnderline: u8,
    pub lfStrikeOut: u8,
    pub lfCharSet: u8,
    pub lfOutPrecision: u8,
    pub lfClipPrecision: u8,
    pub lfQuality: u8,
    pub lfPitchAndFamily: u8,
    pub lfFaceName: [u16; LF_FACESIZE],
}
#[repr(C)]
#[derive(Clone, Copy)]
pub struct ENUMLOGFONTEXW {
    pub elfLogFont: LOGFONTW,
    pub elfFullName: [u16; LF_FULLFACESIZE],
    pub elfStyle: [u16; LF_FACESIZE],
    pub elfScript: [u16; LF_FACESIZE],
}
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct TEXTMETRICW {
    pub tmHeight: i32,
    pub tmAscent: i32,
    pub tmDescent: i32,
    pub tmInternalLeading: i32,
    pub tmExternalLeading: i32,
    pub tmAveCharWidth: i32,
    pub tmMaxCharWidth: i32,
    pub tmWeight: i32,
    pub tmOverhang: i32,
    pub tmDigitizedAspectX: i32,
    pub tmDigitizedAspectY: i32,
    pub tmFirstChar: u16,
    pub tmLastChar: u16,
    pub tmDefaultChar: u16,
    pub tmBreakChar: u16,
    pub tmItalic: u8,
    pub tmUnderlined: u8,
    pub tmStruckOut: u8,
    pub tmPitchAndFamily: u8,
    pub tmCharSet: u8,
}
#[repr(C)]
#[derive(Clone, Copy)]
pub struct NEWTEXTMETRICEXW {
    pub tm: TEXTMETRICW,
    pub ntmFlags: u32,
    pub ntmSizeEM: u32,
    pub ntmCellHeight: u32,
    pub ntmAvgWidth: u32,
    pub fsUsb: [u32; 4],
    pub fsCsb: [u32; 2],
}
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct GLYPHMETRICS {
    pub gmBlackBoxX: u32,
    pub gmBlackBoxY: u32,
    /// x = left bearing; y = distance from the baseline UP to the top of the black box.
    pub gmptGlyphOrigin: POINT,
    pub gmCellIncX: i16,
    pub gmCellIncY: i16,
}
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct FIXED {
    pub fract: u16,
    pub value: i16,
}
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct MAT2 {
    pub eM11: FIXED,
    pub eM12: FIXED,
    pub eM21: FIXED,
    pub eM22: FIXED,
}
impl MAT2 {
    pub const IDENTITY: MAT2 = MAT2 {
        eM11: FIXED { fract: 0, value: 1 },
        eM12: FIXED { fract: 0, value: 0 },
        eM21: FIXED { fract: 0, value: 0 },
        eM22: FIXED { fract: 0, value: 1 },
    };
}
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct ABC {
    pub abcA: i32,
    pub abcB: u32,
    pub abcC: i32,
}
#[repr(C)]
#[derive(Clone, Copy)]
pub struct OSVERSIONINFOEXW {
    pub dwOSVersionInfoSize: u32,
    pub dwMajorVersion: u32,
    pub dwMinorVersion: u32,
    pub dwBuildNumber: u32,
    pub dwPlatformId: u32,
    pub szCSDVersion: [u16; 128],
    pub wServicePackMajor: u16,
    pub wServicePackMinor: u16,
    pub wSuiteMask: u16,
    pub wProductType: u8,
    pub wReserved: u8,
}
/// x64 layout (on 32-bit Windows this struct is packed(1); do not reuse there).
#[repr(C)]
#[derive(Clone, Copy)]
pub struct SHFILEOPSTRUCTW {
    pub hwnd: HWND,
    pub wFunc: u32,
    pub pFrom: PCWSTR, // double-NUL terminated list
    pub pTo: PCWSTR,
    pub fFlags: u16,
    pub fAnyOperationsAborted: BOOL,
    pub hNameMappings: *mut c_void,
    pub lpszProgressTitle: PCWSTR,
}
#[repr(C)]
#[derive(Clone, Copy)]
pub struct SHQUERYRBINFO {
    pub cbSize: u32,
    pub i64Size: i64,
    pub i64NumItems: i64,
}
#[repr(C)]
#[derive(Clone, Copy)]
pub struct APPBARDATA {
    pub cbSize: u32,
    pub hWnd: HWND,
    pub uCallbackMessage: u32,
    pub uEdge: u32,
    pub rc: RECT,
    pub lParam: LPARAM,
}
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct MARGINS {
    pub cxLeftWidth: i32,
    pub cxRightWidth: i32,
    pub cyTopHeight: i32,
    pub cyBottomHeight: i32,
}
/// Only the head of DWM_TIMING_INFO is named; the struct is packed(1) and 292 bytes in all.
#[repr(C, packed(1))]
#[derive(Clone, Copy)]
pub struct DWM_TIMING_INFO {
    pub cbSize: u32,
    pub rateRefresh_num: u32,
    pub rateRefresh_den: u32,
    pub qpcRefreshPeriod: u64,
    pub rateCompose_num: u32,
    pub rateCompose_den: u32,
    pub qpcVBlank: u64,
    pub rest: [u8; 256],
}
#[repr(C)]
#[derive(Clone, Copy)]
pub struct COMPOSITIONFORM {
    pub dwStyle: u32,
    pub ptCurrentPos: POINT,
    pub rcArea: RECT,
}
#[repr(C)]
#[derive(Clone, Copy)]
pub struct CANDIDATEFORM {
    pub dwIndex: u32,
    pub dwStyle: u32,
    pub ptCurrentPos: POINT,
    pub rcArea: RECT,
}

zero_default!(
    WNDCLASSEXW, CREATESTRUCTW, MSG, PAINTSTRUCT, MINMAXINFO, WINDOWPLACEMENT, WINDOWPOS,
    NCCALCSIZE_PARAMS, MONITORINFO, MONITORINFOEXW, TRACKMOUSEEVENT, CURSORINFO, FLASHWINFO,
    COPYDATASTRUCT, ICONINFO, BITMAP, BITMAPINFOHEADER, BITMAPINFO, LOGFONTW, ENUMLOGFONTEXW,
    TEXTMETRICW, NEWTEXTMETRICEXW, OSVERSIONINFOEXW, SHFILEOPSTRUCTW, SHQUERYRBINFO, APPBARDATA,
    DWM_TIMING_INFO, COMPOSITIONFORM, CANDIDATEFORM,
);

assert_size!(
    POINT = 8, SIZE = 8, RECT = 16, GUID = 16, FILETIME = 8, SYSTEMTIME = 16,
    WNDCLASSEXW = 80, CREATESTRUCTW = 80, MSG = 48, PAINTSTRUCT = 72, MINMAXINFO = 40,
    WINDOWPLACEMENT = 44, WINDOWPOS = 40, NCCALCSIZE_PARAMS = 56, MONITORINFO = 40,
    MONITORINFOEXW = 104, TRACKMOUSEEVENT = 24, CURSORINFO = 24, FLASHWINFO = 32,
    COPYDATASTRUCT = 24, ICONINFO = 32, BITMAP = 32, BITMAPINFOHEADER = 40, BITMAPINFO = 52,
    LOGFONTW = 92, ENUMLOGFONTEXW = 348, TEXTMETRICW = 60, NEWTEXTMETRICEXW = 100,
    GLYPHMETRICS = 20, MAT2 = 16, ABC = 12, OSVERSIONINFOEXW = 284, SHFILEOPSTRUCTW = 56,
    SHQUERYRBINFO = 24, APPBARDATA = 48, MARGINS = 16, DWM_TIMING_INFO = 292,
    COMPOSITIONFORM = 28, CANDIDATEFORM = 32,
);

/// Declares an import block and, beside it, a function returning (name, address) for every
/// import. Taking the addresses forces the linker to resolve each symbol; the probe prints
/// the counts, which is the "check by actually linking" for every name below.
/// (Use `//` comments inside; `///` would not match the pattern.)
macro_rules! imports {
    ($lib:literal => $table:ident { $( fn $name:ident ( $( $arg:ident : $ty:ty ),* $(,)? ) $( -> $ret:ty )? ; )* }) => {
        #[link(name = $lib)]
        unsafe extern "system" {
            $( pub fn $name ( $( $arg : $ty ),* ) $( -> $ret )? ; )*
        }
        pub fn $table() -> Vec<(&'static str, usize)> {
            vec![ $( (stringify!($name), $name as *const () as usize) ),* ]
        }
    };
}

imports! { "user32" => user32_imports {
    // class, window, messages
    fn RegisterClassExW(lpwcx: *const WNDCLASSEXW) -> ATOM;
    fn UnregisterClassW(lpClassName: PCWSTR, hInstance: HINSTANCE) -> BOOL;
    fn CreateWindowExW(dwExStyle: u32, lpClassName: PCWSTR, lpWindowName: PCWSTR, dwStyle: u32, x: i32, y: i32, nWidth: i32, nHeight: i32, hWndParent: HWND, hMenu: HMENU, hInstance: HINSTANCE, lpParam: *mut c_void) -> HWND;
    fn DestroyWindow(hWnd: HWND) -> BOOL;
    fn DefWindowProcW(hWnd: HWND, msg: u32, wParam: WPARAM, lParam: LPARAM) -> LRESULT;
    fn ShowWindow(hWnd: HWND, nCmdShow: i32) -> BOOL;
    fn UpdateWindow(hWnd: HWND) -> BOOL;
    fn IsWindow(hWnd: HWND) -> BOOL;
    fn IsWindowVisible(hWnd: HWND) -> BOOL;
    fn IsWindowUnicode(hWnd: HWND) -> BOOL;
    fn IsIconic(hWnd: HWND) -> BOOL;
    fn IsZoomed(hWnd: HWND) -> BOOL;
    fn GetMessageW(lpMsg: *mut MSG, hWnd: HWND, wMsgFilterMin: u32, wMsgFilterMax: u32) -> BOOL;
    fn PeekMessageW(lpMsg: *mut MSG, hWnd: HWND, wMsgFilterMin: u32, wMsgFilterMax: u32, wRemoveMsg: u32) -> BOOL;
    fn TranslateMessage(lpMsg: *const MSG) -> BOOL;
    fn DispatchMessageW(lpMsg: *const MSG) -> LRESULT;
    fn PostMessageW(hWnd: HWND, msg: u32, wParam: WPARAM, lParam: LPARAM) -> BOOL;
    fn SendMessageW(hWnd: HWND, msg: u32, wParam: WPARAM, lParam: LPARAM) -> LRESULT;
    fn SendMessageTimeoutW(hWnd: HWND, msg: u32, wParam: WPARAM, lParam: LPARAM, fuFlags: u32, uTimeout: u32, lpdwResult: *mut usize) -> LRESULT;
    fn PostQuitMessage(nExitCode: i32);
    fn WaitMessage() -> BOOL;
    fn MsgWaitForMultipleObjectsEx(nCount: u32, pHandles: *const HANDLE, dwMilliseconds: u32, dwWakeMask: u32, dwFlags: u32) -> u32;
    fn GetMessageTime() -> i32;
    fn GetQueueStatus(flags: u32) -> u32;
    fn RegisterWindowMessageW(lpString: PCWSTR) -> u32;
    // painting
    fn BeginPaint(hWnd: HWND, lpPaint: *mut PAINTSTRUCT) -> HDC;
    fn EndPaint(hWnd: HWND, lpPaint: *const PAINTSTRUCT) -> BOOL;
    fn InvalidateRect(hWnd: HWND, lpRect: *const RECT, bErase: BOOL) -> BOOL;
    fn ValidateRect(hWnd: HWND, lpRect: *const RECT) -> BOOL;
    fn GetUpdateRect(hWnd: HWND, lpRect: *mut RECT, bErase: BOOL) -> BOOL;
    fn RedrawWindow(hWnd: HWND, lprcUpdate: *const RECT, hrgnUpdate: HRGN, flags: u32) -> BOOL;
    fn GetDC(hWnd: HWND) -> HDC;
    fn ReleaseDC(hWnd: HWND, hDC: HDC) -> i32;
    fn FillRect(hDC: HDC, lprc: *const RECT, hbr: HBRUSH) -> i32;
    // geometry and styles
    fn GetClientRect(hWnd: HWND, lpRect: *mut RECT) -> BOOL;
    fn GetWindowRect(hWnd: HWND, lpRect: *mut RECT) -> BOOL;
    fn ClientToScreen(hWnd: HWND, lpPoint: *mut POINT) -> BOOL;
    fn ScreenToClient(hWnd: HWND, lpPoint: *mut POINT) -> BOOL;
    fn AdjustWindowRectEx(lpRect: *mut RECT, dwStyle: u32, bMenu: BOOL, dwExStyle: u32) -> BOOL;
    fn AdjustWindowRectExForDpi(lpRect: *mut RECT, dwStyle: u32, bMenu: BOOL, dwExStyle: u32, dpi: u32) -> BOOL;
    fn SetWindowPos(hWnd: HWND, hWndInsertAfter: HWND, x: i32, y: i32, cx: i32, cy: i32, uFlags: u32) -> BOOL;
    fn GetWindowLongPtrW(hWnd: HWND, nIndex: i32) -> isize;
    fn SetWindowLongPtrW(hWnd: HWND, nIndex: i32, dwNewLong: isize) -> isize;
    fn GetClassLongPtrW(hWnd: HWND, nIndex: i32) -> usize;
    fn SetClassLongPtrW(hWnd: HWND, nIndex: i32, dwNewLong: isize) -> usize;
    fn GetWindowPlacement(hWnd: HWND, lpwndpl: *mut WINDOWPLACEMENT) -> BOOL;
    fn SetWindowPlacement(hWnd: HWND, lpwndpl: *const WINDOWPLACEMENT) -> BOOL;
    fn SetWindowTextW(hWnd: HWND, lpString: PCWSTR) -> BOOL;
    fn GetWindowTextW(hWnd: HWND, lpString: PWSTR, nMaxCount: i32) -> i32;
    fn GetSystemMetrics(nIndex: i32) -> i32;
    fn GetSystemMetricsForDpi(nIndex: i32, dpi: u32) -> i32;
    fn SystemParametersInfoW(uiAction: u32, uiParam: u32, pvParam: *mut c_void, fWinIni: u32) -> BOOL;
    // keyboard
    fn GetKeyState(nVirtKey: i32) -> i16;
    fn GetAsyncKeyState(vKey: i32) -> i16;
    fn GetKeyboardState(lpKeyState: *mut u8) -> BOOL;
    fn SetKeyboardState(lpKeyState: *const u8) -> BOOL;
    fn MapVirtualKeyW(uCode: u32, uMapType: u32) -> u32;
    fn VkKeyScanW(ch: u16) -> i16;
    fn GetKeyboardLayout(idThread: u32) -> HKL;
    fn GetKeyboardLayoutList(nBuff: i32, lpList: *mut HKL) -> i32;
    fn GetDoubleClickTime() -> u32;
    fn GetCaretBlinkTime() -> u32;
    // the system caret: never shown, but tells IMEs, the emoji panel, Magnifier and Narrator
    // where the insertion point is
    fn CreateCaret(hWnd: HWND, hBitmap: HBITMAP, nWidth: i32, nHeight: i32) -> BOOL;
    fn SetCaretPos(x: i32, y: i32) -> BOOL;
    fn DestroyCaret() -> BOOL;
    // mouse and cursor
    fn SetCapture(hWnd: HWND) -> HWND;
    fn ReleaseCapture() -> BOOL;
    fn GetCapture() -> HWND;
    fn GetCursorPos(lpPoint: *mut POINT) -> BOOL;
    fn TrackMouseEvent(lpEventTrack: *mut TRACKMOUSEEVENT) -> BOOL;
    fn ShowCursor(bShow: BOOL) -> i32;
    fn SetCursor(hCursor: HCURSOR) -> HCURSOR;
    fn LoadCursorW(hInstance: HINSTANCE, lpCursorName: PCWSTR) -> HCURSOR;
    fn GetCursorInfo(pci: *mut CURSORINFO) -> BOOL;
    // focus and activation
    fn SetFocus(hWnd: HWND) -> HWND;
    fn GetFocus() -> HWND;
    fn GetActiveWindow() -> HWND;
    fn SetActiveWindow(hWnd: HWND) -> HWND;
    fn GetForegroundWindow() -> HWND;
    fn SetForegroundWindow(hWnd: HWND) -> BOOL;
    fn BringWindowToTop(hWnd: HWND) -> BOOL;
    fn AllowSetForegroundWindow(dwProcessId: u32) -> BOOL;
    fn FlashWindowEx(pfwi: *const FLASHWINFO) -> BOOL;
    // timers
    fn SetTimer(hWnd: HWND, nIDEvent: usize, uElapse: u32, lpTimerFunc: TIMERPROC) -> usize;
    fn KillTimer(hWnd: HWND, uIDEvent: usize) -> BOOL;
    // clipboard
    fn OpenClipboard(hWndNewOwner: HWND) -> BOOL;
    fn CloseClipboard() -> BOOL;
    fn EmptyClipboard() -> BOOL;
    fn GetClipboardData(uFormat: u32) -> HANDLE;
    fn SetClipboardData(uFormat: u32, hMem: HANDLE) -> HANDLE;
    fn IsClipboardFormatAvailable(format: u32) -> BOOL;
    fn GetClipboardSequenceNumber() -> u32;
    // monitors
    fn MonitorFromWindow(hwnd: HWND, dwFlags: u32) -> HMONITOR;
    fn MonitorFromPoint(pt: POINT, dwFlags: u32) -> HMONITOR;
    fn MonitorFromRect(lprc: *const RECT, dwFlags: u32) -> HMONITOR;
    fn GetMonitorInfoW(hMonitor: HMONITOR, lpmi: *mut MONITORINFO) -> BOOL;
    fn EnumDisplayMonitors(hdc: HDC, lprcClip: *const RECT, lpfnEnum: MONITORENUMPROC, dwData: LPARAM) -> BOOL;
    // other instances
    fn FindWindowW(lpClassName: PCWSTR, lpWindowName: PCWSTR) -> HWND;
    fn GetWindowThreadProcessId(hWnd: HWND, lpdwProcessId: *mut u32) -> u32;
    // icons
    fn CreateIconIndirect(piconinfo: *const ICONINFO) -> HICON;
    fn CreateIconFromResourceEx(presbits: *const u8, dwResSize: u32, fIcon: BOOL, dwVer: u32, cxDesired: i32, cyDesired: i32, flags: u32) -> HICON;
    fn DestroyIcon(hIcon: HICON) -> BOOL;
    fn GetIconInfo(hIcon: HICON, piconinfo: *mut ICONINFO) -> BOOL;
    fn DrawIconEx(hdc: HDC, xLeft: i32, yTop: i32, hIcon: HICON, cxWidth: i32, cyWidth: i32, istepIfAniCur: u32, hbrFlickerFreeDraw: HBRUSH, diFlags: u32) -> BOOL;
    fn LoadImageW(hInst: HINSTANCE, name: PCWSTR, kind: u32, cx: i32, cy: i32, fuLoad: u32) -> HANDLE;
    fn LoadIconW(hInstance: HINSTANCE, lpIconName: PCWSTR) -> HICON;
    // sound and last-resort reporting
    fn MessageBeep(uType: u32) -> BOOL;
    fn MessageBoxW(hWnd: HWND, lpText: PCWSTR, lpCaption: PCWSTR, uType: u32) -> i32;
    // DPI (all present in this toolchain's libuser32.a; Windows 10 1607 / 1703 and later)
    fn SetProcessDpiAwarenessContext(value: DPI_AWARENESS_CONTEXT) -> BOOL;
    fn GetThreadDpiAwarenessContext() -> DPI_AWARENESS_CONTEXT;
    fn GetWindowDpiAwarenessContext(hwnd: HWND) -> DPI_AWARENESS_CONTEXT;
    fn AreDpiAwarenessContextsEqual(a: DPI_AWARENESS_CONTEXT, b: DPI_AWARENESS_CONTEXT) -> BOOL;
    fn GetAwarenessFromDpiAwarenessContext(value: DPI_AWARENESS_CONTEXT) -> i32;
    fn GetDpiForWindow(hwnd: HWND) -> u32;
    fn GetDpiForSystem() -> u32;
    fn EnableNonClientDpiScaling(hwnd: HWND) -> BOOL;
    fn SetProcessDPIAware() -> BOOL;
    // shutdown
    fn ShutdownBlockReasonCreate(hWnd: HWND, pwszReason: PCWSTR) -> BOOL;
    fn ShutdownBlockReasonDestroy(hWnd: HWND) -> BOOL;
    // added for the shell
    fn GetCaretPos(lpPoint: *mut POINT) -> BOOL;
    fn WindowFromPoint(pt: POINT) -> HWND;
}}

imports! { "gdi32" => gdi32_imports {
    fn CreateCompatibleDC(hdc: HDC) -> HDC;
    fn DeleteDC(hdc: HDC) -> BOOL;
    fn CreateCompatibleBitmap(hdc: HDC, cx: i32, cy: i32) -> HBITMAP;
    fn CreateBitmap(nWidth: i32, nHeight: i32, nPlanes: u32, nBitCount: u32, lpBits: *const c_void) -> HBITMAP;
    fn CreateDIBSection(hdc: HDC, pbmi: *const BITMAPINFO, usage: u32, ppvBits: *mut *mut c_void, hSection: HANDLE, offset: u32) -> HBITMAP;
    fn SelectObject(hdc: HDC, h: HGDIOBJ) -> HGDIOBJ;
    fn DeleteObject(ho: HGDIOBJ) -> BOOL;
    fn GetStockObject(i: i32) -> HGDIOBJ;
    fn GetObjectW(h: HANDLE, c: i32, pv: *mut c_void) -> i32;
    fn CreateSolidBrush(color: COLORREF) -> HBRUSH;
    fn BitBlt(hdc: HDC, x: i32, y: i32, cx: i32, cy: i32, hdcSrc: HDC, x1: i32, y1: i32, rop: u32) -> BOOL;
    fn SetDIBitsToDevice(hdc: HDC, xDest: i32, yDest: i32, w: u32, h: u32, xSrc: i32, ySrc: i32, startScan: u32, cLines: u32, lpvBits: *const c_void, lpbmi: *const BITMAPINFO, colorUse: u32) -> i32;
    fn StretchDIBits(hdc: HDC, xDest: i32, yDest: i32, destWidth: i32, destHeight: i32, xSrc: i32, ySrc: i32, srcWidth: i32, srcHeight: i32, lpBits: *const c_void, lpbmi: *const BITMAPINFO, iUsage: u32, rop: u32) -> i32;
    fn GetDIBits(hdc: HDC, hbm: HBITMAP, start: u32, cLines: u32, lpvBits: *mut c_void, lpbmi: *mut BITMAPINFO, usage: u32) -> i32;
    fn SetStretchBltMode(hdc: HDC, mode: i32) -> i32;
    fn GetPixel(hdc: HDC, x: i32, y: i32) -> COLORREF;
    fn SetPixel(hdc: HDC, x: i32, y: i32, color: COLORREF) -> COLORREF;
    fn GdiFlush() -> BOOL;
    fn GetDeviceCaps(hdc: HDC, index: i32) -> i32;
    // fonts
    fn CreateFontW(cHeight: i32, cWidth: i32, cEscapement: i32, cOrientation: i32, cWeight: i32, bItalic: u32, bUnderline: u32, bStrikeOut: u32, iCharSet: u32, iOutPrecision: u32, iClipPrecision: u32, iQuality: u32, iPitchAndFamily: u32, pszFaceName: PCWSTR) -> HFONT;
    fn CreateFontIndirectW(lplf: *const LOGFONTW) -> HFONT;
    fn AddFontResourceExW(name: PCWSTR, fl: u32, res: *mut c_void) -> i32;
    fn RemoveFontResourceExW(name: PCWSTR, fl: u32, pdv: *mut c_void) -> BOOL;
    fn AddFontMemResourceEx(pFileView: *const c_void, cjSize: u32, pvResrved: *mut c_void, pNumFonts: *mut u32) -> HANDLE;
    fn RemoveFontMemResourceEx(h: HANDLE) -> BOOL;
    fn GetGlyphOutlineW(hdc: HDC, uChar: u32, fuFormat: u32, lpgm: *mut GLYPHMETRICS, cjBuffer: u32, pvBuffer: *mut c_void, lpmat2: *const MAT2) -> u32;
    fn GetTextMetricsW(hdc: HDC, lptm: *mut TEXTMETRICW) -> BOOL;
    fn GetTextFaceW(hdc: HDC, c: i32, lpName: PWSTR) -> i32;
    fn GetGlyphIndicesW(hdc: HDC, lpstr: PCWSTR, c: i32, pgi: *mut u16, fl: u32) -> u32;
    fn GetCharWidth32W(hdc: HDC, iFirst: u32, iLast: u32, lpBuffer: *mut i32) -> BOOL;
    fn GetCharABCWidthsW(hdc: HDC, wFirst: u32, wLast: u32, lpABC: *mut ABC) -> BOOL;
    fn GetTextExtentPoint32W(hdc: HDC, lpString: PCWSTR, c: i32, psizl: *mut SIZE) -> BOOL;
    fn GetFontUnicodeRanges(hdc: HDC, lpgs: *mut c_void) -> u32;
    fn EnumFontFamiliesExW(hdc: HDC, lpLogfont: *const LOGFONTW, lpProc: FONTENUMPROCW, lParam: LPARAM, dwFlags: u32) -> i32;
    // GDI text into a DIB: the fallback for what GetGlyphOutlineW cannot do (astral code points)
    fn SetTextColor(hdc: HDC, color: COLORREF) -> COLORREF;
    fn SetBkColor(hdc: HDC, color: COLORREF) -> COLORREF;
    fn SetBkMode(hdc: HDC, mode: i32) -> i32;
    fn ExtTextOutW(hdc: HDC, x: i32, y: i32, options: u32, lprect: *const RECT, lpString: PCWSTR, c: u32, lpDx: *const i32) -> BOOL;
    // added for the shell: reading a face's own cmap, the only way to ask about a character
    // above U+FFFF
    fn GetFontData(hdc: HDC, dwTable: u32, dwOffset: u32, pvBuffer: *mut c_void, cjBuffer: u32) -> u32;
}}

imports! { "kernel32" => kernel32_imports {
    fn GetModuleHandleW(lpModuleName: PCWSTR) -> HMODULE;
    fn GetLastError() -> u32;
    fn SetLastError(dwErrCode: u32);
    fn LoadLibraryW(lpLibFileName: PCWSTR) -> HMODULE;
    fn LoadLibraryExW(lpLibFileName: PCWSTR, hFile: HANDLE, dwFlags: u32) -> HMODULE;
    fn GetProcAddress(hModule: HMODULE, lpProcName: *const u8) -> *const c_void;
    fn FreeLibrary(hLibModule: HMODULE) -> BOOL;
    fn CreateMutexW(lpMutexAttributes: *const c_void, bInitialOwner: BOOL, lpName: PCWSTR) -> HANDLE;
    fn ReleaseMutex(hMutex: HANDLE) -> BOOL;
    fn CloseHandle(hObject: HANDLE) -> BOOL;
    fn WaitForSingleObject(hHandle: HANDLE, dwMilliseconds: u32) -> u32;
    fn QueryPerformanceCounter(lpPerformanceCount: *mut i64) -> BOOL;
    fn QueryPerformanceFrequency(lpFrequency: *mut i64) -> BOOL;
    fn Sleep(dwMilliseconds: u32);
    fn GetTickCount64() -> u64;
    fn GetCurrentProcess() -> HANDLE;
    fn GetCurrentProcessId() -> u32;
    fn GetCurrentThreadId() -> u32;
    fn GetProcessTimes(hProcess: HANDLE, lpCreationTime: *mut FILETIME, lpExitTime: *mut FILETIME, lpKernelTime: *mut FILETIME, lpUserTime: *mut FILETIME) -> BOOL;
    fn QueryProcessCycleTime(hProcess: HANDLE, cycleTime: *mut u64) -> BOOL;
    fn GlobalAlloc(uFlags: u32, dwBytes: usize) -> HGLOBAL;
    fn GlobalLock(hMem: HGLOBAL) -> *mut c_void;
    fn GlobalUnlock(hMem: HGLOBAL) -> BOOL;
    fn GlobalFree(hMem: HGLOBAL) -> HGLOBAL;
    fn GlobalSize(hMem: HGLOBAL) -> usize;
    fn CreateWaitableTimerExW(lpTimerAttributes: *const c_void, lpTimerName: PCWSTR, dwFlags: u32, dwDesiredAccess: u32) -> HANDLE;
    fn SetWaitableTimer(hTimer: HANDLE, lpDueTime: *const i64, lPeriod: i32, pfnCompletionRoutine: *const c_void, lpArgToCompletionRoutine: *mut c_void, fResume: BOOL) -> BOOL;
    fn CancelWaitableTimer(hTimer: HANDLE) -> BOOL;
    // durable saving and watching the folder
    fn MoveFileExW(lpExistingFileName: PCWSTR, lpNewFileName: PCWSTR, dwFlags: u32) -> BOOL;
    fn ReplaceFileW(lpReplacedFileName: PCWSTR, lpReplacementFileName: PCWSTR, lpBackupFileName: PCWSTR, dwReplaceFlags: u32, lpExclude: *mut c_void, lpReserved: *mut c_void) -> BOOL;
    fn GetFileAttributesW(lpFileName: PCWSTR) -> u32;
    fn GetFullPathNameW(lpFileName: PCWSTR, nBufferLength: u32, lpBuffer: PWSTR, lpFilePart: *mut PWSTR) -> u32;
    fn FindFirstChangeNotificationW(lpPathName: PCWSTR, bWatchSubtree: BOOL, dwNotifyFilter: u32) -> HANDLE;
    fn FindNextChangeNotification(hChangeHandle: HANDLE) -> BOOL;
    fn FindCloseChangeNotification(hChangeHandle: HANDLE) -> BOOL;
    fn RegisterApplicationRestart(pwzCommandline: PCWSTR, dwFlags: u32) -> HRESULT;
    fn OutputDebugStringW(lpOutputString: PCWSTR);
    fn GetVersionExW(lpVersionInformation: *mut OSVERSIONINFOEXW) -> BOOL;
    fn GetACP() -> u32;
    // writing resources into a finished exe: the stand-in for a resource compiler
    fn BeginUpdateResourceW(pFileName: PCWSTR, bDeleteExistingResources: BOOL) -> HANDLE;
    fn UpdateResourceW(hUpdate: HANDLE, lpType: PCWSTR, lpName: PCWSTR, wLanguage: u16, lpData: *const c_void, cb: u32) -> BOOL;
    fn EndUpdateResourceW(hUpdate: HANDLE, fDiscard: BOOL) -> BOOL;
    // added for the shell
    fn GetLocalTime(lpSystemTime: *mut SYSTEMTIME);
    fn FindResourceW(hModule: HMODULE, lpName: PCWSTR, lpType: PCWSTR) -> HANDLE;
    fn SizeofResource(hModule: HMODULE, hResInfo: HANDLE) -> u32;
    fn LoadResource(hModule: HMODULE, hResInfo: HANDLE) -> HGLOBAL;
    fn LockResource(hResData: HGLOBAL) -> *const c_void;
}}

imports! { "ntdll" => ntdll_imports {
    // The honest OS version. GetVersionExW lies (6.2.9200) to a process with no manifest.
    fn RtlGetVersion(lpVersionInformation: *mut OSVERSIONINFOEXW) -> i32;
}}

imports! { "shell32" => shell32_imports {
    fn SHGetKnownFolderPath(rfid: *const GUID, dwFlags: u32, hToken: HANDLE, ppszPath: *mut PWSTR) -> HRESULT;
    fn SHFileOperationW(lpFileOp: *mut SHFILEOPSTRUCTW) -> i32;
    fn SHQueryRecycleBinW(pszRootPath: PCWSTR, pSHQueryRBInfo: *mut SHQUERYRBINFO) -> HRESULT;
    fn ShellExecuteW(hwnd: HWND, lpOperation: PCWSTR, lpFile: PCWSTR, lpParameters: PCWSTR, lpDirectory: PCWSTR, nShowCmd: i32) -> HINSTANCE;
    fn SetCurrentProcessExplicitAppUserModelID(appID: PCWSTR) -> HRESULT;
    fn ILCreateFromPathW(pszPath: PCWSTR) -> *mut c_void;
    fn ILFree(pidl: *mut c_void);
    fn SHOpenFolderAndSelectItems(pidlFolder: *const c_void, cidl: u32, apidl: *const *const c_void, dwFlags: u32) -> HRESULT;
    fn SHAppBarMessage(dwMessage: u32, pData: *mut APPBARDATA) -> usize;
    fn DragAcceptFiles(hWnd: HWND, fAccept: BOOL);
    fn DragQueryFileW(hDrop: HDROP, iFile: u32, lpszFile: PWSTR, cch: u32) -> u32;
    fn DragFinish(hDrop: HDROP);
}}

imports! { "ole32" => ole32_imports {
    fn CoTaskMemFree(pv: *mut c_void);
    fn CoInitializeEx(pvReserved: *mut c_void, dwCoInit: u32) -> HRESULT;
    fn CoUninitialize();
}}

imports! { "advapi32" => advapi32_imports {
    fn RegGetValueW(hkey: HKEY, lpSubKey: PCWSTR, lpValue: PCWSTR, dwFlags: u32, pdwType: *mut u32, pvData: *mut c_void, pcbData: *mut u32) -> i32;
}}

imports! { "winmm" => winmm_imports {
    fn timeBeginPeriod(uPeriod: u32) -> u32;
    fn timeEndPeriod(uPeriod: u32) -> u32;
}}

pub fn all_imports() -> Vec<(&'static str, Vec<(&'static str, usize)>)> {
    vec![
        ("user32", user32_imports()),
        ("gdi32", gdi32_imports()),
        ("kernel32", kernel32_imports()),
        ("ntdll", ntdll_imports()),
        ("shell32", shell32_imports()),
        ("ole32", ole32_imports()),
        ("advapi32", advapi32_imports()),
        ("winmm", winmm_imports()),
    ]
}

/// Everything that has no import library on this toolchain (dwmapi, shcore, imm32, uxtheme)
/// plus one undocumented ntdll export. Each field is None when the DLL or the export is
/// missing, so every caller has to say what happens without it.
pub struct Dyn {
    // dwmapi.dll
    pub DwmSetWindowAttribute: Option<unsafe extern "system" fn(HWND, u32, *const c_void, u32) -> HRESULT>,
    pub DwmGetWindowAttribute: Option<unsafe extern "system" fn(HWND, u32, *mut c_void, u32) -> HRESULT>,
    pub DwmFlush: Option<unsafe extern "system" fn() -> HRESULT>,
    pub DwmIsCompositionEnabled: Option<unsafe extern "system" fn(*mut BOOL) -> HRESULT>,
    pub DwmExtendFrameIntoClientArea: Option<unsafe extern "system" fn(HWND, *const MARGINS) -> HRESULT>,
    pub DwmGetCompositionTimingInfo: Option<unsafe extern "system" fn(HWND, *mut DWM_TIMING_INFO) -> HRESULT>,
    // shcore.dll
    pub GetDpiForMonitor: Option<unsafe extern "system" fn(HMONITOR, u32, *mut u32, *mut u32) -> HRESULT>,
    // imm32.dll
    pub ImmGetContext: Option<unsafe extern "system" fn(HWND) -> HIMC>,
    pub ImmReleaseContext: Option<unsafe extern "system" fn(HWND, HIMC) -> BOOL>,
    pub ImmSetCompositionWindow: Option<unsafe extern "system" fn(HIMC, *const COMPOSITIONFORM) -> BOOL>,
    pub ImmSetCandidateWindow: Option<unsafe extern "system" fn(HIMC, *const CANDIDATEFORM) -> BOOL>,
    // uxtheme.dll ordinals 135/136. UNDOCUMENTED, builds 18362+ only; makes the system
    // (Alt+Space) menu follow dark mode. 0 Default, 1 AllowDark, 2 ForceDark, 3 ForceLight.
    pub SetPreferredAppMode: Option<unsafe extern "system" fn(i32) -> i32>,
    pub FlushMenuThemes: Option<unsafe extern "system" fn()>,
    // ntdll.dll, undocumented but stable since 1607
    pub RtlAreLongPathsEnabled: Option<unsafe extern "system" fn() -> u8>,
    /// What failed to load, for the log: "dwmapi.dll" or "dwmapi.dll!DwmFlush".
    pub missing: Vec<String>,
}

/// Load a DLL from System32 only (never from the exe's folder or the current directory).
pub fn load_system_dll(name: &str) -> HMODULE {
    unsafe { LoadLibraryExW(wide(name).as_ptr(), 0, LOAD_LIBRARY_SEARCH_SYSTEM32) }
}

/// `T` must be an `unsafe extern "system" fn` pointer type. `name` must end in NUL.
pub unsafe fn sym<T: Copy>(module: HMODULE, name: &[u8]) -> Option<T> {
    assert!(core::mem::size_of::<T>() == core::mem::size_of::<usize>());
    assert!(name.last() == Some(&0));
    if module == 0 {
        return None;
    }
    let p = unsafe { GetProcAddress(module, name.as_ptr()) };
    if p.is_null() { None } else { Some(unsafe { core::mem::transmute_copy::<*const c_void, T>(&p) }) }
}

/// By ordinal (GetProcAddress accepts MAKEINTRESOURCEA(ordinal) as the name).
pub unsafe fn sym_ordinal<T: Copy>(module: HMODULE, ordinal: u16) -> Option<T> {
    assert!(core::mem::size_of::<T>() == core::mem::size_of::<usize>());
    if module == 0 {
        return None;
    }
    let p = unsafe { GetProcAddress(module, ordinal as usize as *const u8) };
    if p.is_null() { None } else { Some(unsafe { core::mem::transmute_copy::<*const c_void, T>(&p) }) }
}

impl Dyn {
    pub fn load() -> Dyn {
        let mut missing = Vec::new();
        macro_rules! get {
            ($m:expr, $dll:literal, $name:literal) => {{
                let f = unsafe { sym($m, concat!($name, "\0").as_bytes()) };
                if f.is_none() {
                    missing.push(format!("{}!{}", $dll, $name));
                }
                f
            }};
        }
        let dwm = load_system_dll("dwmapi.dll");
        let shcore = load_system_dll("shcore.dll");
        let imm = load_system_dll("imm32.dll");
        let ntdll = unsafe { GetModuleHandleW(wide("ntdll.dll").as_ptr()) };
        let build = os_version().2;
        // uxtheme's ordinal 135 was a different function before 1903: gate on the build.
        let ux = if build >= 18362 { load_system_dll("uxtheme.dll") } else { 0 };
        Dyn {
            DwmSetWindowAttribute: get!(dwm, "dwmapi.dll", "DwmSetWindowAttribute"),
            DwmGetWindowAttribute: get!(dwm, "dwmapi.dll", "DwmGetWindowAttribute"),
            DwmFlush: get!(dwm, "dwmapi.dll", "DwmFlush"),
            DwmIsCompositionEnabled: get!(dwm, "dwmapi.dll", "DwmIsCompositionEnabled"),
            DwmExtendFrameIntoClientArea: get!(dwm, "dwmapi.dll", "DwmExtendFrameIntoClientArea"),
            DwmGetCompositionTimingInfo: get!(dwm, "dwmapi.dll", "DwmGetCompositionTimingInfo"),
            GetDpiForMonitor: get!(shcore, "shcore.dll", "GetDpiForMonitor"),
            ImmGetContext: get!(imm, "imm32.dll", "ImmGetContext"),
            ImmReleaseContext: get!(imm, "imm32.dll", "ImmReleaseContext"),
            ImmSetCompositionWindow: get!(imm, "imm32.dll", "ImmSetCompositionWindow"),
            ImmSetCandidateWindow: get!(imm, "imm32.dll", "ImmSetCandidateWindow"),
            SetPreferredAppMode: {
                let f = unsafe { sym_ordinal(ux, 135) };
                if f.is_none() {
                    missing.push("uxtheme.dll!#135".into());
                }
                f
            },
            FlushMenuThemes: {
                let f = unsafe { sym_ordinal(ux, 136) };
                if f.is_none() {
                    missing.push("uxtheme.dll!#136".into());
                }
                f
            },
            RtlAreLongPathsEnabled: get!(ntdll, "ntdll.dll", "RtlAreLongPathsEnabled"),
            missing,
        }
    }

    /// DwmSetWindowAttribute with a typed value; E_NOTIMPL when dwmapi is absent.
    pub fn set_window_attr<T: Copy>(&self, hwnd: HWND, attr: u32, value: &T) -> HRESULT {
        match self.DwmSetWindowAttribute {
            Some(f) => unsafe { f(hwnd, attr, value as *const T as *const c_void, core::mem::size_of::<T>() as u32) },
            None => E_NOTIMPL,
        }
    }
    pub fn get_window_attr<T: Copy>(&self, hwnd: HWND, attr: u32, value: &mut T) -> HRESULT {
        match self.DwmGetWindowAttribute {
            Some(f) => unsafe { f(hwnd, attr, value as *mut T as *mut c_void, core::mem::size_of::<T>() as u32) },
            None => E_NOTIMPL,
        }
    }
    /// Dark caption. Attribute 20 from build 18985, attribute 19 on 17763..18362.
    /// Call before the window is first shown; Windows 10 does not repaint a visible caption.
    pub fn set_dark_title_bar(&self, hwnd: HWND, dark: bool) -> HRESULT {
        let v: BOOL = dark as BOOL;
        let build = os_version().2;
        if build >= 18985 {
            self.set_window_attr(hwnd, DWMWA_USE_IMMERSIVE_DARK_MODE, &v)
        } else if build >= 17763 {
            self.set_window_attr(hwnd, DWMWA_USE_IMMERSIVE_DARK_MODE_BEFORE_20H1, &v)
        } else {
            E_NOTIMPL
        }
    }
    /// Refresh period of the compositor in seconds, if DWM will say.
    pub fn refresh_period(&self) -> Option<f64> {
        let f = self.DwmGetCompositionTimingInfo?;
        let mut ti = DWM_TIMING_INFO { cbSize: core::mem::size_of::<DWM_TIMING_INFO>() as u32, ..Default::default() };
        if unsafe { f(0, &mut ti) } < 0 {
            return None;
        }
        let (n, d) = (ti.rateRefresh_num, ti.rateRefresh_den);
        if n == 0 { None } else { Some(d as f64 / n as f64) }
    }
    /// Tell the IME where the caret is so its composition/candidate window opens there.
    pub fn ime_set_caret(&self, hwnd: HWND, x: i32, y: i32, line_height: i32) {
        let (Some(get), Some(release), Some(set_comp), Some(set_cand)) =
            (self.ImmGetContext, self.ImmReleaseContext, self.ImmSetCompositionWindow, self.ImmSetCandidateWindow)
        else {
            return;
        };
        unsafe {
            let himc = get(hwnd);
            if himc == 0 {
                return;
            }
            let cf = COMPOSITIONFORM { dwStyle: CFS_FORCE_POSITION, ptCurrentPos: POINT { x, y }, rcArea: RECT::default() };
            set_comp(himc, &cf);
            let cand = CANDIDATEFORM {
                dwIndex: 0,
                dwStyle: CFS_EXCLUDE,
                ptCurrentPos: POINT { x, y },
                rcArea: RECT { left: x, top: y, right: x + 1, bottom: y + line_height },
            };
            set_cand(himc, &cand);
            release(hwnd, himc);
        }
    }
}

static DYN: OnceLock<Dyn> = OnceLock::new();
/// The process-wide table, loaded on first use.
pub fn dynapi() -> &'static Dyn {
    DYN.get_or_init(Dyn::load)
}

/// First show without a white frame. On this Windows 10 machine a shown but not yet painted
/// window with no class brush is composed as solid white (#FFFFFF, measured). Cloaked, the
/// window still receives and handles WM_PAINT but is not composed; uncloaked afterwards, its
/// first visible frame is the painted one (also measured). Without dwmapi it is a plain show.
pub fn show_without_flash(hwnd: HWND, cmd: i32) {
    let d = dynapi();
    let on: BOOL = TRUE;
    let off: BOOL = FALSE;
    let cloaked = d.set_window_attr(hwnd, DWMWA_CLOAK, &on) >= 0;
    unsafe {
        ShowWindow(hwnd, cmd);
        UpdateWindow(hwnd); // synchronous WM_PAINT
    }
    if cloaked {
        d.set_window_attr(hwnd, DWMWA_CLOAK, &off);
    }
}

/// A frame period, in seconds, that divides the display's refresh evenly and is as close to
/// `target_hz` as that allows: on a 165 Hz panel a 60 fps target becomes every third
/// vertical blank, 18.2 ms, 55 fps. Falls back to 1/60 s when DWM will not say.
pub fn even_frame_period(target_hz: f64) -> f64 {
    let refresh = dynapi().refresh_period().filter(|p| *p > 0.0005 && *p < 0.1).unwrap_or(1.0 / 60.0);
    let k = ((1.0 / target_hz) / refresh).round().max(1.0);
    refresh * k
}

/// UTF-8 -> NUL-terminated UTF-16.
pub fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(core::iter::once(0)).collect()
}
/// Path -> NUL-terminated UTF-16, lossless (paths are not always valid Unicode).
pub fn wide_path(p: &Path) -> Vec<u16> {
    use std::os::windows::ffi::OsStrExt;
    p.as_os_str().encode_wide().chain(core::iter::once(0)).collect()
}
pub unsafe fn wide_len(p: PCWSTR) -> usize {
    let mut n = 0;
    while unsafe { *p.add(n) } != 0 {
        n += 1;
    }
    n
}
/// A fixed UTF-16 buffer up to its first NUL.
pub fn from_wide_buf(buf: &[u16]) -> String {
    let n = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    String::from_utf16_lossy(&buf[..n])
}

pub const fn loword(v: usize) -> u32 {
    (v & 0xFFFF) as u32
}
pub const fn hiword(v: usize) -> u32 {
    ((v >> 16) & 0xFFFF) as u32
}
/// Signed client x of a mouse message (negative on a monitor left of the primary, or while captured).
pub const fn get_x_lparam(l: LPARAM) -> i32 {
    (l as usize & 0xFFFF) as u16 as i16 as i32
}
pub const fn get_y_lparam(l: LPARAM) -> i32 {
    ((l as usize >> 16) & 0xFFFF) as u16 as i16 as i32
}
pub const fn make_lparam(lo: i32, hi: i32) -> LPARAM {
    (((hi as u32 & 0xFFFF) << 16) | (lo as u32 & 0xFFFF)) as LPARAM
}
/// Signed wheel delta: multiples of 120 from a notched wheel, small values from a precision
/// touchpad or free-spinning wheel.
pub const fn get_wheel_delta(w: WPARAM) -> i32 {
    ((w >> 16) & 0xFFFF) as u16 as i16 as i32
}
pub const fn get_keystate_wparam(w: WPARAM) -> u32 {
    (w & 0xFFFF) as u32
}
/// Bit 29 of a WM_SYSKEYDOWN lParam: Alt was down when the key went down.
pub const fn key_alt_down(l: LPARAM) -> bool {
    (l as usize >> 29) & 1 != 0
}
/// true = this WM_KEYDOWN is an auto-repeat.
pub const fn key_was_down(l: LPARAM) -> bool {
    (l as usize >> 30) & 1 != 0
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Mods {
    pub ctrl: bool,
    pub shift: bool,
    pub alt: bool,
    pub win: bool,
    /// Ctrl and Alt are both down. On a layout with AltGr that is how AltGr arrives
    /// (left Ctrl + right Alt), so never bind a command to Ctrl+Alt with a printing key.
    pub altgr_like: bool,
}
/// Modifier state as of the message being processed (GetKeyState, not GetAsyncKeyState).
pub fn mods() -> Mods {
    unsafe {
        let ctrl = GetKeyState(VK_CONTROL) < 0;
        let alt = GetKeyState(VK_MENU) < 0;
        Mods {
            ctrl,
            alt,
            shift: GetKeyState(VK_SHIFT) < 0,
            win: GetKeyState(VK_LWIN) < 0 || GetKeyState(VK_RWIN) < 0,
            altgr_like: ctrl && alt,
        }
    }
}

/// Reassembles the UTF-16 code units that WM_CHAR delivers one message at a time.
/// Characters outside the BMP arrive as two WM_CHAR messages: high surrogate, then low.
#[derive(Default)]
pub struct CharDecoder {
    high: u16,
}
impl CharDecoder {
    /// Feed `wParam as u16` of a WM_CHAR. Returns a complete scalar value when there is one.
    /// Unpaired surrogates are dropped (they cannot be stored in a Rust String anyway).
    pub fn push(&mut self, unit: u16) -> Option<char> {
        match unit {
            0xD800..=0xDBFF => {
                self.high = unit;
                None
            }
            0xDC00..=0xDFFF => {
                let h = core::mem::take(&mut self.high);
                if h == 0 {
                    return None;
                }
                char::from_u32(0x10000 + (((h - 0xD800) as u32) << 10) + (unit - 0xDC00) as u32)
            }
            _ => {
                self.high = 0;
                char::from_u32(unit as u32)
            }
        }
    }
    /// Feed a WM_UNICHAR wParam (already UTF-32). Remember to answer TRUE to UNICODE_NOCHAR.
    pub fn push_utf32(&mut self, cp: u32) -> Option<char> {
        self.high = 0;
        char::from_u32(cp)
    }
}
/// What WM_CHAR may insert. Everything below U+0020 is a control character that Ctrl+letter,
/// Enter, Tab, Backspace and Escape produce; take those keys from WM_KEYDOWN instead, or
/// Ctrl+M becomes a newline, Ctrl+H a backspace and Ctrl+I a tab.
pub const fn is_text_char(c: char) -> bool {
    let u = c as u32;
    u >= 0x20 && u != 0x7F && !(u >= 0x80 && u < 0xA0)
}

/// Accumulates high-resolution wheel deltas into whole lines (or columns) without losing
/// the remainder. One instance per axis.
#[derive(Default)]
pub struct WheelAccumulator {
    acc: i32,
}
impl WheelAccumulator {
    /// `delta` from get_wheel_delta; `per_notch` = lines per 120 units. Returns whole lines.
    /// Vertical: positive = away from the user = scroll up. Horizontal: positive = right.
    pub fn feed(&mut self, delta: i32, per_notch: i32) -> i32 {
        if (delta > 0) != (self.acc > 0) {
            self.acc = 0; // direction changed: drop the stale remainder
        }
        self.acc += delta * per_notch;
        let whole = self.acc / WHEEL_DELTA;
        self.acc -= whole * WHEEL_DELTA;
        whole
    }
}
/// (lines per notch, columns per notch) from the user's mouse settings. Lines may be
/// WHEEL_PAGESCROLL, meaning "one screen".
pub fn wheel_settings() -> (u32, u32) {
    let (mut lines, mut chars) = (3u32, 3u32);
    unsafe {
        SystemParametersInfoW(SPI_GETWHEELSCROLLLINES, 0, &mut lines as *mut u32 as *mut c_void, 0);
        SystemParametersInfoW(SPI_GETWHEELSCROLLCHARS, 0, &mut chars as *mut u32 as *mut c_void, 0);
    }
    (lines, chars)
}

pub fn qpc() -> i64 {
    let mut t = 0i64;
    unsafe { QueryPerformanceCounter(&mut t) };
    t
}
pub fn qpc_frequency() -> i64 {
    static F: OnceLock<i64> = OnceLock::new();
    *F.get_or_init(|| {
        let mut f = 0i64;
        unsafe { QueryPerformanceFrequency(&mut f) };
        f.max(1)
    })
}
/// Monotonic seconds.
pub fn now() -> f64 {
    qpc() as f64 / qpc_frequency() as f64
}

/// A waitable timer for frame pacing. Auto-reset; high-resolution where the OS has it
/// (Windows 10 1803+), ordinary (about 15.6 ms granularity) otherwise.
///
/// Measured on the target machine (build 19045, 165 Hz panel), 60 intervals each:
///   SetTimer(16)                     mean 27.5 ms, sd 7     (it rounds UP to two 15.6 ms ticks)
///   SetTimer(10)                     mean 15.6 ms, sd 1-6   (one tick; timeBeginPeriod(1) changes nothing)
///   ordinary waitable timer, 16.67   mean 16.8 ms, sd 4     (alternates 15.6 / 31.2)
///   this timer, HIGH_RESOLUTION      mean 16.67 ms, sd 0.5  (and 8.33 ms, sd 0.5)
///   DwmFlush()                       mean 6.06 ms, sd 0.1   (one per vertical blank; x3 = 18.20 ms, sd 0.06)
/// All of them cost under 0.1 ms of CPU per second of waiting; blocked in GetMessageW the
/// process uses nothing at all.
///
/// The shell arms it with `arm_in` and waits on `handle` and the message queue together
/// (`Shell::wait_for_frame`), with a timeout under the wait so a timer that never fires
/// cannot hold the loop.
pub struct FrameTimer {
    pub handle: HANDLE,
    pub high_resolution: bool,
}
impl FrameTimer {
    pub fn new(want_high_resolution: bool) -> FrameTimer {
        unsafe {
            if want_high_resolution {
                let h = CreateWaitableTimerExW(null(), null(), CREATE_WAITABLE_TIMER_HIGH_RESOLUTION, TIMER_ALL_ACCESS);
                if h != 0 {
                    return FrameTimer { handle: h, high_resolution: true };
                }
            }
            FrameTimer { handle: CreateWaitableTimerExW(null(), null(), 0, TIMER_ALL_ACCESS), high_resolution: false }
        }
    }
    /// One shot, `seconds` from now.
    pub fn arm_in(&self, seconds: f64) -> bool {
        let due: i64 = -((seconds * 1e7) as i64).max(1); // negative = relative, 100 ns units
        unsafe { SetWaitableTimer(self.handle, &due, 0, null(), null_mut(), 0) != 0 }
    }
}
impl Drop for FrameTimer {
    fn drop(&mut self) {
        if self.handle != 0 {
            unsafe { CloseHandle(self.handle) };
        }
    }
}

/// (major, minor, build) from RtlGetVersion, which does not depend on a manifest.
pub fn os_version() -> (u32, u32, u32) {
    static V: OnceLock<(u32, u32, u32)> = OnceLock::new();
    *V.get_or_init(|| {
        let mut v = OSVERSIONINFOEXW { dwOSVersionInfoSize: core::mem::size_of::<OSVERSIONINFOEXW>() as u32, ..Default::default() };
        unsafe { RtlGetVersion(&mut v) };
        (v.dwMajorVersion, v.dwMinorVersion, v.dwBuildNumber)
    })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DpiMode {
    PerMonitorV2,
    PerMonitor,
    System,
    /// Somebody set it first (a manifest, or an earlier call); whatever is in force stays.
    AlreadySet,
    Unaware,
}
/// The runtime stand-in for the manifest's `<dpiAwareness>`. Must run before the first window
/// is created and before anything measures the screen.
pub fn init_dpi_awareness() -> DpiMode {
    unsafe {
        if SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) != 0 {
            return DpiMode::PerMonitorV2;
        }
        if GetLastError() == ERROR_ACCESS_DENIED {
            return DpiMode::AlreadySet;
        }
        if SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE) != 0 {
            return DpiMode::PerMonitor;
        }
        if SetProcessDPIAware() != 0 { DpiMode::System } else { DpiMode::Unaware }
    }
}

/// A monitor's rectangle and work area. None when Windows will not say.
pub fn monitor_info(mon: HMONITOR) -> Option<MONITORINFOEXW> {
    let mut mi = MONITORINFOEXW { cbSize: core::mem::size_of::<MONITORINFOEXW>() as u32, ..Default::default() };
    let ok = unsafe { GetMonitorInfoW(mon, &mut mi as *mut MONITORINFOEXW as *mut MONITORINFO) };
    if ok != 0 { Some(mi) } else { None }
}

/// Header for a top-down 32 bpp DIB of `w` x `h`: pixel (x, y) is `fb[y * w + x]`,
/// value 0x00RRGGBB.
pub fn dib_header(w: i32, h: i32) -> BITMAPINFO {
    BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: core::mem::size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: w,
            biHeight: -h,
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB,
            ..Default::default()
        },
        bmiColors: [0; 3],
    }
}

/// Copy rectangle `r` of the framebuffer to the same rectangle of `hdc`, 1:1.
///
/// Only the rows of `r` are described to GDI (a band `r.h()` rows tall starting at
/// `fb[r.top * fb_w]`), so the source y is always 0. That sidesteps the upside-down
/// source-y rule that SetDIBitsToDevice applies to a partial copy out of a larger DIB.
/// Returns the number of scan lines GDI copied (0 on failure or an empty rect).
pub fn blit_rect(hdc: HDC, fb: &[u32], fb_w: i32, fb_h: i32, r: RECT) -> i32 {
    let r = RECT { left: r.left.max(0), top: r.top.max(0), right: r.right.min(fb_w), bottom: r.bottom.min(fb_h) };
    if r.is_empty() || fb.len() < (fb_w as usize) * (fb_h as usize) {
        return 0;
    }
    let bmi = dib_header(fb_w, r.h());
    let band = fb[(r.top as usize) * (fb_w as usize)..].as_ptr();
    unsafe {
        SetDIBitsToDevice(hdc, r.left, r.top, r.w() as u32, r.h() as u32, r.left, 0, 0, r.h() as u32, band as *const c_void, &bmi, DIB_RGB_COLORS)
    }
}

/// The face GDI actually selected into `hdc`. GDI substitutes silently when the face asked
/// for does not exist, so this is how a caller finds out.
pub fn font_face_name(hdc: HDC) -> String {
    let mut buf = [0u16; 64];
    unsafe { GetTextFaceW(hdc, buf.len() as i32, buf.as_mut_ptr()) };
    from_wide_buf(&buf)
}
pub fn text_metrics(hdc: HDC) -> Option<TEXTMETRICW> {
    let mut tm = TEXTMETRICW::default();
    if unsafe { GetTextMetricsW(hdc, &mut tm) } != 0 { Some(tm) } else { None }
}
/// true when the selected font has a glyph for this BMP code unit (not the .notdef box).
pub fn font_has_glyph(hdc: HDC, unit: u16) -> bool {
    let mut gi = 0u16;
    let n = unsafe { GetGlyphIndicesW(hdc, &unit, 1, &mut gi, GGI_MARK_NONEXISTING_GLYPHS) };
    n == 1 && gi != 0xFFFF
}

pub struct GlyphBitmap {
    pub w: usize,
    pub h: usize,
    /// Bytes per row in `data` (rows are padded to a multiple of 4 bytes).
    pub pitch: usize,
    /// Pen-relative x of the bitmap's left edge.
    pub left: i32,
    /// Rows from the baseline up to the bitmap's top row. Row 0 of the bitmap goes at
    /// cell_y = tmAscent - top.
    pub top: i32,
    pub advance: i32,
    /// GGO_GRAY8_BITMAP: coverage 0..=64 per byte. GGO_BITMAP: 1 bit per pixel, MSB first.
    pub data: Vec<u8>,
}
/// Rasterise one BMP character with the font selected into `hdc`.
/// `format` is GGO_GRAY8_BITMAP or GGO_BITMAP. None = GDI_ERROR (bitmap-only faces fail here:
/// GetGlyphOutlineW works for TrueType/OpenType outlines only).
pub fn glyph_outline(hdc: HDC, ch: u32, format: u32) -> Option<GlyphBitmap> {
    let mat = MAT2::IDENTITY;
    let mut gm = GLYPHMETRICS::default();
    unsafe {
        let size = GetGlyphOutlineW(hdc, ch, format, &mut gm, 0, null_mut(), &mat);
        if size == GDI_ERROR {
            return None;
        }
        let mut data = vec![0u8; size as usize];
        if size > 0 && GetGlyphOutlineW(hdc, ch, format, &mut gm, size, data.as_mut_ptr() as *mut c_void, &mat) == GDI_ERROR {
            return None;
        }
        // A blank glyph (space) reports size 0 but a 1x1 black box: treat it as empty.
        let (w, h) = if size == 0 { (0, 0) } else { (gm.gmBlackBoxX as usize, gm.gmBlackBoxY as usize) };
        let pitch = if format == GGO_BITMAP { w.div_ceil(32) * 4 } else { (w + 3) & !3 };
        Some(GlyphBitmap { w, h, pitch, left: gm.gmptGlyphOrigin.x, top: gm.gmptGlyphOrigin.y, advance: gm.gmCellIncX as i32, data })
    }
}

/// The fallback rasteriser: GDI draws the text (ExtTextOutW) into a private 32 bpp DIB and
/// the coverage is read back. Slower than GetGlyphOutlineW, but it covers everything GDI
/// can draw and GetGlyphOutlineW cannot: raster .fon faces, surrogate pairs, and glyphs GDI
/// pulls in from a linked fallback font. Create the font with ANTIALIASED_QUALITY or
/// NONANTIALIASED_QUALITY (ClearType would put colour fringes in the result).
pub struct TextRaster {
    dc: HDC,
    bmp: HBITMAP,
    old_bmp: HGDIOBJ,
    bits: *mut u32,
    pub w: i32,
    pub h: i32,
}
impl TextRaster {
    pub fn new(w: i32, h: i32) -> Option<TextRaster> {
        unsafe {
            let dc = CreateCompatibleDC(0);
            let bmi = dib_header(w, h);
            let mut bits: *mut c_void = null_mut();
            let bmp = CreateDIBSection(dc, &bmi, DIB_RGB_COLORS, &mut bits, 0, 0);
            if dc == 0 || bmp == 0 || bits.is_null() {
                DeleteObject(bmp);
                DeleteDC(dc);
                return None;
            }
            let old_bmp = SelectObject(dc, bmp);
            SetBkMode(dc, TRANSPARENT);
            SetTextColor(dc, 0x00FF_FFFF);
            Some(TextRaster { dc, bmp, old_bmp, bits: bits as *mut u32, w, h })
        }
    }
    /// Draw UTF-16 `text` with `font`, cell top-left at (0, 0) (so the baseline is at
    /// tmAscent). Returns coverage 0..=255 per pixel, `w * h` bytes, top row first.
    pub fn draw(&mut self, font: HFONT, text: &[u16]) -> Vec<u8> {
        unsafe {
            let px = core::slice::from_raw_parts_mut(self.bits, (self.w * self.h) as usize);
            px.fill(0);
            let old = SelectObject(self.dc, font);
            ExtTextOutW(self.dc, 0, 0, 0, null(), text.as_ptr(), text.len() as u32, null());
            GdiFlush();
            SelectObject(self.dc, old);
            px.iter().map(|p| ((p >> 8) & 0xFF) as u8).collect()
        }
    }
}
impl Drop for TextRaster {
    fn drop(&mut self) {
        unsafe {
            SelectObject(self.dc, self.old_bmp);
            DeleteObject(self.bmp);
            DeleteDC(self.dc);
        }
    }
}

#[derive(Clone, Debug)]
pub struct FontFamily {
    pub name: String,
    pub truetype: bool,
    pub raster: bool,
}
/// Every installed (and privately added) fixed-pitch family, deduplicated and sorted.
/// Vertical-writing aliases (names starting with '@') are skipped.
pub fn fixed_pitch_families() -> Vec<FontFamily> {
    unsafe extern "system" fn each(lf: *const LOGFONTW, _tm: *const TEXTMETRICW, font_type: u32, data: LPARAM) -> i32 {
        unsafe {
            let lf = &*lf;
            let out = &mut *(data as *mut Vec<FontFamily>);
            if (lf.lfPitchAndFamily as u32 & 3) == FIXED_PITCH && lf.lfFaceName[0] != b'@' as u16 {
                out.push(FontFamily {
                    name: from_wide_buf(&lf.lfFaceName),
                    truetype: font_type & TRUETYPE_FONTTYPE != 0,
                    raster: font_type & RASTER_FONTTYPE != 0,
                });
            }
        }
        1
    }
    let mut out: Vec<FontFamily> = Vec::new();
    unsafe {
        let hdc = CreateCompatibleDC(0);
        let lf = LOGFONTW { lfCharSet: DEFAULT_CHARSET as u8, ..Default::default() };
        EnumFontFamiliesExW(hdc, &lf, Some(each), &mut out as *mut Vec<FontFamily> as LPARAM, 0);
        DeleteDC(hdc);
    }
    out.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    out.dedup_by(|a, b| a.name == b.name);
    out
}

fn open_clipboard_retry(hwnd: HWND) -> bool {
    // Another program (a clipboard manager, RDP) may hold it for a few milliseconds.
    for _ in 0..10 {
        if unsafe { OpenClipboard(hwnd) } != 0 {
            return true;
        }
        unsafe { Sleep(5) };
    }
    false
}
/// Text on the clipboard with CR LF and lone CR folded to LF. None if there is no text.
pub fn clipboard_get_text(hwnd: HWND) -> Option<String> {
    unsafe {
        if IsClipboardFormatAvailable(CF_UNICODETEXT) == 0 || !open_clipboard_retry(hwnd) {
            return None;
        }
        let mut out = None;
        let h = GetClipboardData(CF_UNICODETEXT); // owned by the clipboard: never free it
        if h != 0 {
            let p = GlobalLock(h) as *const u16;
            if !p.is_null() {
                let max = GlobalSize(h) / 2; // do not trust the data to be NUL-terminated
                let units = core::slice::from_raw_parts(p, max);
                let n = units.iter().position(|&c| c == 0).unwrap_or(max);
                let s = String::from_utf16_lossy(&units[..n]);
                out = Some(s.replace("\r\n", "\n").replace('\r', "\n"));
                GlobalUnlock(h);
            }
        }
        CloseClipboard();
        out
    }
}
/// The CF_UNICODETEXT payload for `text`: UTF-16, bare LF expanded to CR LF, NUL-terminated.
pub fn clipboard_units(text: &str) -> Vec<u16> {
    let mut units: Vec<u16> = Vec::with_capacity(text.len() + 16);
    let mut prev = 0u16;
    for u in text.encode_utf16() {
        if u == b'\n' as u16 && prev != b'\r' as u16 {
            units.push(b'\r' as u16);
        }
        units.push(u);
        prev = u;
    }
    units.push(0);
    units
}
/// Put text on the clipboard, LF expanded to CR LF (what every other Windows program expects).
pub fn clipboard_set_text(hwnd: HWND, text: &str) -> bool {
    let units = clipboard_units(text);
    unsafe {
        let h = GlobalAlloc(GMEM_MOVEABLE, units.len() * 2);
        if h == 0 {
            return false;
        }
        let p = GlobalLock(h) as *mut u16;
        if p.is_null() {
            GlobalFree(h);
            return false;
        }
        core::ptr::copy_nonoverlapping(units.as_ptr(), p, units.len());
        GlobalUnlock(h);
        if !open_clipboard_retry(hwnd) {
            GlobalFree(h);
            return false;
        }
        EmptyClipboard();
        let ok = SetClipboardData(CF_UNICODETEXT, h) != 0; // on success the system owns `h`
        if !ok {
            GlobalFree(h);
        }
        CloseClipboard();
        ok
    }
}

/// Documents may be redirected (OneDrive "known folder move"); this returns where it really is.
pub fn known_folder(id: &GUID) -> Option<PathBuf> {
    use std::os::windows::ffi::OsStringExt;
    unsafe {
        let mut p: PWSTR = null_mut();
        let hr = SHGetKnownFolderPath(id, KF_FLAG_DEFAULT, 0, &mut p);
        let out = if hr >= 0 && !p.is_null() {
            Some(PathBuf::from(std::ffi::OsString::from_wide(core::slice::from_raw_parts(p, wide_len(p)))))
        } else {
            None
        };
        CoTaskMemFree(p as *mut c_void); // must be freed even on failure
        out
    }
}

/// Build an HICON from straight-alpha 0xAARRGGBB pixels, top row first.
/// The colour plane is a 32 bpp DIB section; the mask is all zero (alpha does the work).
pub fn icon_from_argb(w: i32, h: i32, px: &[u32]) -> HICON {
    if px.len() < (w * h) as usize {
        return 0;
    }
    unsafe {
        let bmi = dib_header(w, h);
        let mut bits: *mut c_void = null_mut();
        let color = CreateDIBSection(0, &bmi, DIB_RGB_COLORS, &mut bits, 0, 0);
        if color == 0 || bits.is_null() {
            return 0;
        }
        core::ptr::copy_nonoverlapping(px.as_ptr(), bits as *mut u32, (w * h) as usize);
        let mask_bytes = vec![0u8; (((w + 15) / 16) * 2 * h) as usize]; // 1 bpp, rows padded to 16 bits
        let mask = CreateBitmap(w, h, 1, 1, mask_bytes.as_ptr() as *const c_void);
        let ii = ICONINFO { fIcon: TRUE, xHotspot: 0, yHotspot: 0, hbmMask: mask, hbmColor: color };
        let icon = CreateIconIndirect(&ii); // copies both bitmaps
        DeleteObject(color);
        DeleteObject(mask);
        icon
    }
}

/// One icon image in .ico/RT_ICON form (BITMAPINFOHEADER, bottom-up BGRA, AND mask) from
/// straight-alpha 0xAARRGGBB pixels. Usable with CreateIconFromResourceEx at run time and
/// with UpdateResourceW(RT_ICON) to stamp the exe.
pub fn icon_resource_bytes(w: i32, h: i32, px: &[u32]) -> Vec<u8> {
    let mut out = Vec::new();
    let hdr = BITMAPINFOHEADER {
        biSize: 40,
        biWidth: w,
        biHeight: h * 2, // XOR image + AND mask
        biPlanes: 1,
        biBitCount: 32,
        biCompression: BI_RGB,
        ..Default::default()
    };
    out.extend_from_slice(unsafe { core::slice::from_raw_parts(&hdr as *const _ as *const u8, 40) });
    for y in (0..h).rev() {
        for x in 0..w {
            out.extend_from_slice(&px[(y * w + x) as usize].to_le_bytes());
        }
    }
    let mask_row = (((w + 31) / 32) * 4) as usize;
    out.resize(out.len() + mask_row * h as usize, 0);
    out
}
/// The RT_GROUP_ICON directory for images stored as RT_ICON ids 1, 2, 3...
pub fn icon_group_bytes(images: &[(i32, i32, usize)]) -> Vec<u8> {
    let mut out = vec![0u8, 0, 1, 0];
    out.extend_from_slice(&(images.len() as u16).to_le_bytes());
    for (i, &(w, h, len)) in images.iter().enumerate() {
        out.push(if w >= 256 { 0 } else { w as u8 });
        out.push(if h >= 256 { 0 } else { h as u8 });
        out.extend_from_slice(&[0, 0]); // colour count, reserved
        out.extend_from_slice(&1u16.to_le_bytes()); // planes
        out.extend_from_slice(&32u16.to_le_bytes()); // bit count
        out.extend_from_slice(&(len as u32).to_le_bytes());
        out.extend_from_slice(&((i + 1) as u16).to_le_bytes()); // RT_ICON id
    }
    out
}

pub struct InstanceLock {
    pub handle: HANDLE,
    pub already_running: bool,
}
/// A named mutex is the race-free "am I first?" test. Names are per logon session by default.
/// Keep the returned handle for the life of the process; the OS releases it even on a crash.
pub fn instance_lock(name: &str) -> InstanceLock {
    unsafe {
        SetLastError(0);
        let handle = CreateMutexW(null(), FALSE, wide(name).as_ptr());
        let err = GetLastError();
        InstanceLock { handle, already_running: err == ERROR_ALREADY_EXISTS || (handle == 0 && err == ERROR_ACCESS_DENIED) }
    }
}
/// Hand a blob to another instance's window and wait up to two seconds for it to be taken.
/// The shell sends the path to open this way, as its UTF-16 units.
pub fn send_copydata(target: HWND, sender: HWND, tag: usize, bytes: &[u8]) -> bool {
    let cds = COPYDATASTRUCT { dwData: tag, cbData: bytes.len() as u32, lpData: bytes.as_ptr() as *mut c_void };
    let mut result = 0usize;
    unsafe { SendMessageTimeoutW(target, WM_COPYDATA, sender as WPARAM, &cds as *const _ as LPARAM, SMTO_ABORTIFHUNG, 2000, &mut result) != 0 }
}

/// Resize-border thickness the system uses at this window's DPI (8 px at 96 DPI on Windows 10:
/// SM_CXFRAME 4 + SM_CXPADDEDBORDER 4).
pub fn resize_border(hwnd: HWND) -> i32 {
    unsafe {
        let dpi = GetDpiForWindow(hwnd);
        GetSystemMetricsForDpi(SM_CXFRAME, dpi) + GetSystemMetricsForDpi(SM_CXPADDEDBORDER, dpi)
    }
}
/// WM_NCCALCSIZE for a window that keeps WS_OVERLAPPEDWINDOW (so snap, the shadow and the
/// minimise/maximise animations survive) but draws its own title row.
/// Lets DefWindowProc take the left, right and bottom resize borders, then gives the top back
/// to the client. Returns None when the default handling should run untouched.
pub unsafe fn frameless_nccalcsize(hwnd: HWND, wparam: WPARAM, lparam: LPARAM) -> Option<LRESULT> {
    unsafe {
        let style = GetWindowLongPtrW(hwnd, GWL_STYLE) as u32;
        if style & WS_THICKFRAME == 0 {
            return None; // borderless fullscreen: there is no frame to adjust
        }
        // wParam TRUE: lParam -> NCCALCSIZE_PARAMS. wParam FALSE (as sent during window
        // creation): lParam -> RECT. Either way the first RECT there is the proposed window
        // rectangle on entry and must be the client rectangle on return.
        let rc = lparam as *mut RECT;
        let top = (*rc).top;
        DefWindowProcW(hwnd, WM_NCCALCSIZE, wparam, lparam);
        (*rc).top = top;
        if IsZoomed(hwnd) != 0 {
            // Maximized, the window rectangle overhangs the monitor by the border thickness
            // on every side. DefWindowProc already pulled left/right/bottom in; do the top.
            (*rc).top += resize_border(hwnd);
        }
        Some(0)
    }
}
/// WM_NCHITTEST to go with it. `title_px` is the height in pixels of the row(s) that drag the
/// window. Returns HTCLIENT for everything the app should treat as its own.
pub unsafe fn frameless_hittest(hwnd: HWND, lparam: LPARAM, title_px: i32) -> LRESULT {
    unsafe {
        // The left/right/bottom borders are still real non-client area: ask the system first.
        let def = DefWindowProcW(hwnd, WM_NCHITTEST, 0, lparam);
        if def != HTCLIENT {
            return def;
        }
        let mut pt = POINT { x: get_x_lparam(lparam), y: get_y_lparam(lparam) };
        ScreenToClient(hwnd, &mut pt);
        let mut rc = RECT::default();
        GetClientRect(hwnd, &mut rc);
        let style = GetWindowLongPtrW(hwnd, GWL_STYLE) as u32;
        let sizable = style & WS_THICKFRAME != 0 && IsZoomed(hwnd) == 0;
        let b = resize_border(hwnd);
        if sizable && pt.y < b {
            // The top resize border now lives inside the client area.
            return if pt.x < b {
                HTTOPLEFT
            } else if pt.x >= rc.right - b {
                HTTOPRIGHT
            } else {
                HTTOP
            };
        }
        if pt.y < title_px && style & WS_CAPTION == WS_CAPTION { HTCAPTION } else { HTCLIENT }
    }
}

/// Call from WM_SIZING with `edge = wParam` and `rc = lParam as *mut RECT`, then return TRUE.
/// `nc_w`/`nc_h` are the non-client extents (window size minus client size, from
/// GetWindowRect and GetClientRect). The edge being dragged is the one that moves.
pub unsafe fn snap_sizing_rect(edge: WPARAM, rc: *mut RECT, cell_w: i32, cell_h: i32, nc_w: i32, nc_h: i32, min_cols: i32, min_rows: i32) {
    unsafe {
        let r = &mut *rc;
        let cols = ((r.w() - nc_w + cell_w / 2) / cell_w).max(min_cols);
        let rows = ((r.h() - nc_h + cell_h / 2) / cell_h).max(min_rows);
        let (w, h) = (cols * cell_w + nc_w, rows * cell_h + nc_h);
        match edge {
            WMSZ_LEFT | WMSZ_TOPLEFT | WMSZ_BOTTOMLEFT => r.left = r.right - w,
            _ => r.right = r.left + w,
        }
        match edge {
            WMSZ_TOP | WMSZ_TOPLEFT | WMSZ_TOPRIGHT => r.top = r.bottom - h,
            _ => r.bottom = r.top + h,
        }
    }
}
