//! Provides abstractions for all entries in the `apfnSimpleCall` table.
//!
//! Depending on the operating system the program is running on, the syscalls are invoked differently:
//! - On Windows 11 or newer, the function is loaded from `win32u.dll`.
//! - On older operating systems the function is invoked via the `NtUserCall*` family of syscalls, loaded from `win32u.dll`.
//! - On Windows 7 to 8.1, `NtUserCall*` syscalls are not exported, and the syscalls are invoked directly via inline assembly.
//!
//! Function resolution happens the first time the function is called.
//!
//! Errors:
//! - [`UserCallError::OsNotSupported`]: The crate does not contain table entry indices.
//! - [`UserCallError::LibraryNotFound`]: A required DLL has not been loaded.
//! - [`UserCallError::CallNotFound`]: The function cannot be invoked on the current operating system.

use std::ffi::c_void;
use std::sync::atomic::{AtomicPtr, AtomicU32, Ordering};

use windows::Win32::Devices::Display::HDEV;
use windows::Win32::Graphics::Gdi::HMONITOR;
use windows::Win32::UI::Input::KeyboardAndMouse::HKL;
use windows::Win32::UI::WindowsAndMessaging::MESSAGEBOX_STYLE;
use windows::{
    core::{w, PCSTR},
    Win32::{
        Foundation::{
            BOOL, HANDLE, HWND, LPARAM, LRESULT, NTSTATUS, POINT, UNICODE_STRING, WPARAM,
        },
        Graphics::Gdi::{HDC, HRGN},
        System::{
            LibraryLoader::{GetModuleHandleW, GetProcAddress},
            StationsAndDesktops::HDESK,
        },
        UI::WindowsAndMessaging::{GET_CLASS_LONG_INDEX, HDWP, HICON, HMENU, SYSTEM_METRICS_INDEX},
    },
};

use crate::{
    error::UserCallError,
    indices::get_index,
    version::{get_os_version, has_dedicated_syscalls, OsVersion},
};
trait IntoCallParam {
    fn into_call_param(self) -> usize;
}

macro_rules! into_call_param_self_as {
    ($($type:ty),+) => {
        $(
        impl IntoCallParam for $type {
            fn into_call_param(self) -> usize {
                self as _
            }
        }
    )+
    };
}

macro_rules! into_call_param_self_0_as {
    ($($type:ty),+) => {
        $(
        impl IntoCallParam for $type {
            fn into_call_param(self) -> usize {
                self.0 as _
            }
        }
    )+
    };
}

macro_rules! into_call_param_transmute {
    ($($type:ty),+) => {
        $(
        impl IntoCallParam for $type {
            fn into_call_param(self) -> usize {
                // SAFETY: Self is layout-compatible with `usize`.
                unsafe {
                    std::mem::transmute(self)
                }
            }
        }
    )+
    };
}

impl<T> IntoCallParam for *const T {
    fn into_call_param(self) -> usize {
        self as _
    }
}

impl<T> IntoCallParam for *mut T {
    fn into_call_param(self) -> usize {
        self as _
    }
}

into_call_param_self_as!(i16, i32, u32, usize);
into_call_param_self_0_as!(
    BOOL,
    GET_CLASS_LONG_INDEX,
    LRESULT,
    LPARAM,
    MESSAGEBOX_STYLE,
    SYSTEM_METRICS_INDEX,
    WPARAM
);
into_call_param_transmute!(HANDLE, HDC, HDESK, HDEV, HRGN, HWND);

trait FromCallReturn {
    fn from_call_return(value: usize) -> Self;
}

macro_rules! from_call_return_as {
    ($($type:ty),+) => {
        $(
        impl FromCallReturn for $type {
            fn from_call_return(value: usize) -> Self {
                value as _
            }
        }
    )+
    };
}

macro_rules! from_call_return_self {
    ($($type:ty),+) => {
        $(
        impl FromCallReturn for $type {
            fn from_call_return(value: usize) -> Self {
                Self(value as _)
            }
        }
    )+
    };
}

impl FromCallReturn for () {
    fn from_call_return(_value: usize) -> Self {}
}

impl<T> FromCallReturn for *const T {
    fn from_call_return(value: usize) -> Self {
        value as _
    }
}

impl<T> FromCallReturn for *mut T {
    fn from_call_return(value: usize) -> Self {
        value as _
    }
}

from_call_return_as!(i32, u32, usize);
from_call_return_self!(
    BOOL, HANDLE, HDESK, HDWP, HICON, HKL, HMENU, HMONITOR, HWND, LPARAM, LRESULT, NTSTATUS
);

macro_rules! nt_user_call_fn_body {
    ( $syscall:ident $call:ident ) => {{
        user_call::$syscall($call)
    }};

    ( $syscall:ident $call:ident $($paramname:ident)* ) => {{
        user_call::$syscall($(IntoCallParam::into_call_param($paramname)),*, $call)
    }};
}

macro_rules! nt_user_call_fn {
    (
        #[doc = $doc:literal] $syscall:ident $call:ident $vis:vis fn $name:ident ($($paramname:ident: $paramtype:ty),*) -> $rettype:ty
    ) => {
        paste::paste! {
            #[doc = $doc]
            #[allow(clippy::empty_docs, clippy::missing_safety_doc)]
            #[expect(non_snake_case)]
            $vis unsafe fn [< NtUser $name >] ($($paramname: $paramtype),*) -> Result<$rettype, UserCallError> {
                if has_dedicated_syscalls() {
                    // Starting with Windows 11, NtUserCall* has been replaced with dedicated syscalls in win32u.
                    crate::macros::load_runtime_fn_body!(["win32u"] $name($($paramname: $paramtype),*) -> $rettype)
                } else {
                    static CALL_ATOMIC: AtomicU32 = AtomicU32::new(u16::MAX as u32 + 1);

                    let call_index = match CALL_ATOMIC.load(Ordering::Relaxed) {
                        index@..=0xFFFFu32 => index,
                        u32::MAX => return Err(UserCallError::CallNotFound),
                        _ => match get_index(NtUserCall::$name) {
                            Some(index) => {
                                CALL_ATOMIC.store(index as _, Ordering::SeqCst);
                                index as _
                            },
                            None => {
                                CALL_ATOMIC.store(u32::MAX, Ordering::SeqCst);
                                return Err(UserCallError::CallNotFound);
                            }
                        }
                    };

                    let $call = call_index;

                    nt_user_call_fn_body!($syscall $call $($paramname)*).map(FromCallReturn::from_call_return)
                }
            }
        }
    };
}

macro_rules! nt_user_call {
    ( #![doc = $enumdoc:literal] $(#[doc = $doc:literal] $syscall:ident $vis:vis fn $name:ident ($($funcdef:tt)*) -> $rettype:ty;)+ ) => {
        #[doc = $enumdoc]
        #[allow(non_camel_case_types)]
        #[derive(Debug, Clone, Copy)]
        pub enum NtUserCall {
            $($name),+
        }

        $(nt_user_call_fn! { #[doc = $doc] $syscall CALL $vis fn $name ($($funcdef)*) -> $rettype })+
    };
}

nt_user_call! {
    #![doc = r#"
    The sum of all functions accessible via the `NtUserCall*` family of system calls in all supported operating systems.
    The variants will be mapped to the respective function indices in [`crate::indices`] at runtime.
    "#]

    // NoParam
    #[doc = "<https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-createmenu>"]
    NtUserCallNoParam pub fn CreateMenu() -> HMENU;

    #[doc = "<https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-createpopupmenu>"]
    NtUserCallNoParam pub fn CreatePopupMenu() -> HMENU;

    #[doc = ""]
    NtUserCallNoParam pub fn AllowForegroundActivation() -> ();

    #[doc = ""]
    NtUserCallNoParam pub fn CancelQueueEventCompletionPacket() -> ();

    #[doc = ""]
    NtUserCallNoParam pub fn ClearWakeMask() -> ();

    #[doc = ""]
    NtUserCallNoParam pub fn CreateSystemThreads() -> ();

    #[doc = ""]
    NtUserCallNoParam pub fn DesktopHasWatermarkText() -> BOOL;

    #[doc = "<https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-destroycaret>"]
    NtUserCallNoParam pub fn DestroyCaret() -> BOOL;

    #[doc = "<https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-disableprocesswindowsghosting>"]
    NtUserCallNoParam pub fn DisableProcessWindowsGhosting() -> ();

    #[doc = ""]
    NtUserCallNoParam pub fn DrainThreadCoreMessagingCompletions() -> BOOL;

    #[doc = ""]
    NtUserCallNoParam pub fn GetDeviceChangeInfo() -> u32;

    #[doc = ""]
    NtUserCallNoParam pub fn GetIMEShowStatus() -> BOOL;

    #[doc = ""]
    NtUserCallNoParam pub fn GetInputDesktop() -> HDESK;

    #[doc = "<https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-getmessagepos>"]
    NtUserCallNoParam pub fn GetMessagePos() -> u32;

    #[doc = ""]
    NtUserCallNoParam pub fn GetQueueIocp() -> HANDLE;

    #[doc = ""]
    NtUserCallNoParam pub fn GetUnpredictedMessagePos() -> u32;

    #[doc = ""]
    NtUserCallNoParam pub fn HandleSystemThreadCreationFailure() -> BOOL;

    #[doc = ""]
    NtUserCallNoParam pub fn HideCursorNoCapture() -> ();

    #[doc = ""]
    NtUserCallNoParam pub fn IsQueueAttached() -> BOOL;

    #[doc = ""]
    NtUserCallNoParam pub fn LoadCursorsAndIcons() -> BOOL;

    #[doc = ""]
    NtUserCallNoParam pub fn LoadUserApiHook() -> ();

    #[doc = ""]
    NtUserCallNoParam pub fn PrepareForLogoff() -> BOOL;

    #[doc = ""]
    NtUserCallNoParam pub fn ReassociateQueueEventCompletionPacket() -> BOOL;

    #[doc = ""]
    NtUserCallNoParam pub fn ReleaseCapture() -> BOOL;

    #[doc = ""]
    NtUserCallNoParam pub fn RemoveQueueCompletion() -> BOOL;

    #[doc = ""]
    NtUserCallNoParam pub fn ResetDblClk() -> BOOL;

    #[doc = ""]
    NtUserCallNoParam pub fn ZapActiveAndFocus() -> BOOL;

    #[doc = ""]
    NtUserCallNoParam pub fn RemoteConsoleShadowStop() -> ();

    #[doc = ""]
    NtUserCallNoParam pub fn RemoteDisconnect() -> ();

    #[doc = ""]
    NtUserCallNoParam pub fn RemoteLogoff() -> NTSTATUS;

    #[doc = "Always returns STATUS_NOT_SUPPORTED."]
    NtUserCallNoParam pub fn RemoteNtSecurity() -> NTSTATUS;

    #[doc = "Always returns STATUS_NOT_SUPPORTED."]
    NtUserCallNoParam pub fn EditionPostKeyboardInputMessage() -> NTSTATUS;

    #[doc = "May only be called by CSRSS, returns STATUS_ACCESS_DENIED otherwise."]
    NtUserCallNoParam pub fn RemoteShadowSetup() -> NTSTATUS;

    #[doc = "May only be called by CSRSS, returns STATUS_ACCESS_DENIED otherwise."]
    NtUserCallNoParam pub fn RemoteShadowStop() -> NTSTATUS;

    #[doc = "May only be called by CSRSS, returns STATUS_ACCESS_DENIED otherwise."]
    NtUserCallNoParam pub fn RemotePassthruEnable() -> NTSTATUS;

    #[doc = "May only be called by CSRSS, returns STATUS_ACCESS_DENIED otherwise."]
    NtUserCallNoParam pub fn RemotePassthruDisable() -> NTSTATUS;

    #[doc = ""]
    NtUserCallNoParam pub fn RemoteConnectState() -> usize;

    #[doc = ""]
    NtUserCallNoParam pub fn TraceLoggingSendMixedModeTelemetry() -> BOOL;

    #[doc = ""]
    NtUserCallNoParam pub fn UpdatePerUserImmEnabling() -> BOOL;

    #[doc = ""]
    NtUserCallNoParam pub fn UserPowerCalloutWorker() -> BOOL;

    #[doc = "May only be called by CSRSS, returns STATUS_UNSUPPORTED otherwise."]
    NtUserCallNoParam pub fn WakeRITForShutdown() -> NTSTATUS;

    #[doc = ""]
    NtUserCallNoParam pub fn DoInitMessagePumpHook() -> BOOL;

    #[doc = ""]
    NtUserCallNoParam pub fn DoUninitMessagePumpHook() -> BOOL;

    #[doc = ""]
    NtUserCallNoParam pub fn EnableMiPShellThread() -> BOOL;

    #[doc = ""]
    NtUserCallNoParam pub fn IsMiPShellThreadEnabled() -> BOOL;

    #[doc = ""]
    NtUserCallNoParam pub fn EnableMouseInPointerForThread() -> BOOL;

    #[doc = ""]
    NtUserCallNoParam pub fn DeferredDesktopRotation() -> i32;

    #[doc = ""]
    NtUserCallNoParam pub fn EnablePerMonitorMenuScaling() -> BOOL;

    #[doc = "<https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-begindeferwindowpos>"]
    NtUserCallOneParam pub fn BeginDeferWindowPos(nNumWindows: i32) -> HDWP;

    #[doc = ""]
    NtUserCallOneParam pub fn GetSendMessageReceiver(dwThreadId: u32) -> HWND;

    #[doc = "<https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-windowfromdc>"]
    NtUserCallOneParam pub fn WindowFromDC(hdc: HDC) -> HWND;

    #[doc = "<https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-allowsetforegroundwindow>"]
    NtUserCallOneParam pub fn AllowSetForegroundWindow(dwProcessId: u32) -> u32;

    #[doc = ""]
    NtUserCallOneParam pub fn CreateEmptyCursorObject(param: BOOL) -> u32;

    #[doc = ""]
    NtUserCallOneParam pub fn CsDdeUninitialize(dde_object: usize) -> BOOL;

    #[doc = "NOP"]
    NtUserCallOneParam pub fn DirectedYield(param: usize) -> usize;

    #[doc = ""]
    NtUserCallOneParam pub fn KbdNlsFuncTypeDummy(param: usize) -> u32;

    #[doc = ""]
    NtUserCallOneParam pub fn EditionGetExecutionEvironment(param: usize) -> BOOL;

    #[doc = "<https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-enumclipboardformats>"]
    NtUserCallOneParam pub fn EnumClipboardFormats(format: u32) -> u32;

    #[doc = ""]
    NtUserCallOneParam pub fn GetInputEvent(wake_mask_and_flags: u32) -> HANDLE;

    #[doc = ""]
    NtUserCallOneParam pub fn GetKeyboardLayout(dwThread: u32) -> HKL;

    #[doc = "<https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-getkeyboardtype>"]
    NtUserCallOneParam pub fn GetKeyboardType(nTypeFlag: i32) -> i32;

    #[doc = ""]
    NtUserCallOneParam pub fn GetProcessDefaultLayout(pdwDefaultLayout: *mut u32) -> BOOL;

    #[doc = ""]
    NtUserCallOneParam pub fn GetQueueStatus(flags: u32) -> u32;

    #[doc = ""]
    NtUserCallOneParam pub fn GetWinStationInfo(ptr: *mut c_void) -> BOOL;

    #[doc = "<https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-locksetforegroundwindow>"]
    NtUserCallOneParam pub fn LockSetForegroundWindow(uLockCode: u32) -> BOOL;

    #[doc = ""]
    NtUserCallOneParam pub fn LW_LoadFonts(unknown: i32) -> BOOL;

    #[doc = ""]
    NtUserCallOneParam pub fn MapDesktopObject(handle: *mut c_void) -> *mut c_void;

    #[doc = "<https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-messagebeep>"]
    NtUserCallOneParam pub fn MessageBeep(uType: MESSAGEBOX_STYLE) -> BOOL;

    #[doc = ""]
    NtUserCallOneParam pub fn PlayEventSound(unknown: u32) -> BOOL;

    #[doc = "<https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-postquitmessage>"]
    NtUserCallOneParam pub fn PostQuitMessage(nExitCode: i32) -> ();

    #[doc = "<https://learn.microsoft.com/en-us/windows/win32/api/wingdi/nf-wingdi-realizepalette>"]
    NtUserCallOneParam pub fn RealizePalette(hdc: HDC) -> u32;

    #[doc = ""]
    NtUserCallOneParam pub fn RegisterLPK(unknown: u32) -> BOOL;

    #[doc = ""]
    NtUserCallOneParam pub fn RegisterSystemThread(unknown_flags: u32) -> BOOL;

    #[doc = ""]
    NtUserCallOneParam pub fn RemoteReconnect(unknown: *mut c_void) -> NTSTATUS;

    #[doc = "May only be called by CSRSS, returns STATUS_ACCESS_DENIED otherwise."]
    NtUserCallOneParam pub fn RemoteThinwireStats(stats: *mut c_void) -> NTSTATUS;

    #[doc = ""]
    NtUserCallOneParam pub fn ReleaseDC(hdc: HDC) -> BOOL;

    #[doc = "May only be called by CSRSS, returns STATUS_ACCESS_DENIED otherwise."]
    NtUserCallOneParam pub fn RemoteNotify(unknown: *const u32) -> NTSTATUS;

    #[doc = "<https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-replymessage>"]
    NtUserCallOneParam pub fn ReplyMessage(lResult: LRESULT) -> BOOL;

    #[doc = "<https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-setcaretblinktime>"]
    NtUserCallOneParam pub fn SetCaretBlinkTime(uMSeconds: u32) -> BOOL;

    #[doc = "<https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-setdoubleclicktime>"]
    NtUserCallOneParam pub fn SetDoubleClickTime(unnamedParam1: u32) -> BOOL;

    #[doc = "<https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-setmessageextrainfo>"]
    NtUserCallOneParam pub fn SetMessageExtraInfo(lParam: LPARAM) -> LPARAM;

    #[doc = "<https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-setprocessdefaultlayout>"]
    NtUserCallOneParam pub fn SetProcessDefaultLayout(dwDefaultLayout: u32) -> BOOL;

    #[doc = "May only be called by winlogon, returns FALSE otherwise."]
    NtUserCallOneParam pub fn SetWatermarkStrings(param: *const UNICODE_STRING) -> BOOL;

    #[doc = "<https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-showcursor>"]
    NtUserCallOneParam pub fn ShowCursor(bShow: BOOL) -> i32;

    #[doc = ""]
    NtUserCallOneParam pub fn ShowStartGlass(param: u32) -> BOOL;

    #[doc = "<https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-swapmousebutton>"]
    NtUserCallOneParam pub fn SwapMouseButton(fSwap: BOOL) -> BOOL;

    #[doc = ""]
    NtUserCallOneParam pub fn WOWModuleUnload(param: i16) -> BOOL;

    #[doc = "May only be called by winlogon."]
    NtUserCallOneParam pub fn DwmLockScreenUpdates(lock: BOOL) -> i32;

    #[doc = "May only be called by dwm, returns FALSE otherwise."]
    NtUserCallOneParam pub fn EnableSessionForMMCSS(enable: BOOL) -> BOOL;

    #[doc = ""]
    NtUserCallOneParam pub fn SetWaitForQueueAttach(wait: BOOL) -> BOOL;

    #[doc = ""]
    NtUserCallOneParam pub fn ThreadMessageQueueAttached(thread_id: u32) -> BOOL;

    #[doc = "May only be called by the immersive broker, otherwise returns 0 with GetLastError() == ERROR_ACCESS_DENIED."]
    NtUserCallOneParam pub fn PostUIActions(wparam: WPARAM) -> LRESULT;

    #[doc = ""]
    NtUserCallOneParam pub fn EnsureDpiDepSysMetCacheForPlateau(dpi: u32) -> BOOL;

    #[doc = ""]
    NtUserCallOneParam pub fn ForceEnableNumpadTranslation(param: u32) -> u32;

    #[doc = ""]
    NtUserCallOneParam pub fn SetTSFEventState(state: u32) -> BOOL;

    #[doc = ""]
    NtUserCallOneParam pub fn SetShellChangeNotifyHWND(hwnd: HWND) -> BOOL;

    #[doc = ""]
    NtUserCallHwnd pub fn DeregisterShellHookWindow(hwnd: HWND) -> BOOL;

    #[doc = ""]
    NtUserCallHwnd pub fn DWP_GetEnabledPopup(hwnd: HWND) -> usize;

    #[doc = ""]
    NtUserCallHwnd pub fn DWP_GetEnabledPopupOffset(hwnd: HWND) -> usize;

    #[doc = ""]
    NtUserCallHwnd pub fn GetModernAppWindow(hwnd: HWND) -> HWND;

    #[doc = ""]
    NtUserCallHwnd pub fn GetWindowContextHelpId(hwnd: HWND) -> ();

    #[doc = ""]
    NtUserCallHwnd pub fn RegisterShellHookWindow(hwnd: HWND) -> ();

    #[doc = ""]
    NtUserCallHwnd pub fn SetMsgBox(hwnd: HWND) -> BOOL;

    #[doc = ""]
    NtUserCallHwndSafe pub fn InitThreadCoreMessagingIocp(hwnd: HWND) -> HANDLE;

    #[doc = ""]
    NtUserCallHwndSafe pub fn ScheduleDispatchNotification(hwnd: HWND) -> i32;

    #[doc = ""]
    NtUserCallHwndSafe pub fn SetProgmanWindow(hwnd: HWND) -> BOOL;

    #[doc = ""]
    NtUserCallHwndOpt pub fn SetTaskmanWindow(hwnd: HWND) -> BOOL;

    #[doc = "See <https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-getclasslongptrw>. `index` may be GCLP_HCURSOR or GCLP_HICON."]
    NtUserCallHwndParam pub fn GetClassIcoCur(hwnd: HWND, index: GET_CLASS_LONG_INDEX) -> HICON;

    #[doc = ""]
    NtUserCallHwndParam pub fn ClearWindowState(hwnd: HWND, state: u32) -> BOOL;

    #[doc = ""]
    NtUserCallHwndParam pub fn KillSystemTimer(hwnd: HWND, timer_id: usize) -> BOOL;

    #[doc = ""]
    NtUserCallHwndParam pub fn NotifyOverlayWindow(hwnd: HWND, param: BOOL) -> BOOL;

    #[doc = "May only be called by the immersive broker, otherwise returns FALSE with GetLastError() == ERROR_ACCESS_DENIED."]
    NtUserCallHwndParam pub fn RegisterKeyboardCorrectionCallout(hwnd: HWND, param: u32) -> BOOL;

    #[doc = ""]
    NtUserCallHwndParam pub fn SetDialogPointer(hwnd: HWND, param: u32) -> BOOL;

    #[doc = ""]
    NtUserCallHwndParam pub fn SetVisible(hwnd: HWND, param: u32) -> BOOL;

    #[doc = "<https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-setwindowcontexthelpid>"]
    NtUserCallHwndParam pub fn SetWindowContextHelpId(hwnd: HWND, help_context_identifier: u32) -> BOOL;

    #[doc = ""]
    NtUserCallHwndParam pub fn SetWindowState(hwnd: HWND, state: u32) -> BOOL;

    #[doc = ""]
    NtUserCallHwndParam pub fn RegisterWindowArrangementCallout(hwnd: HWND, param: u32) -> BOOL;

    #[doc = "May only be called by the immersive broker, otherwise returns 0 with GetLastError() == ERROR_ACCESS_DENIED."]
    NtUserCallHwndParam pub fn EnableModernAppWindowKeyboardIntercept(hwnd: HWND, param: u32) -> BOOL;

    #[doc = "<https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-arrangeiconicwindows>"]
    NtUserCallHwndLock pub fn ArrangeIconicWindows(hwnd: HWND) -> u32;

    #[doc = "<https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-drawmenubar>"]
    NtUserCallHwndLock pub fn DrawMenuBar(hwnd: HWND) -> BOOL;

    #[doc = ""]
    NtUserCallHwndLock pub fn CheckImeShowStatusInThread(hwnd: HWND) -> BOOL;

    #[doc = ""]
    NtUserCallHwndLock pub fn GetSysMenuHandle(hwnd: HWND) -> HMENU;

    #[doc = ""]
    NtUserCallHwndLock pub fn GetSysMenuOffset(hwnd: HWND) -> usize;

    #[doc = "Equivalent to `SetWindowPos(hwnd, HWND::default(), 0, 0, 0, 0, SWP_DRAWFRAME | SWP_NOACTIVATE | SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER)`"]
    NtUserCallHwndLock pub fn RedrawFrame(hwnd: HWND) -> BOOL;

    #[doc = "Redraws and calls WH_SYSMSGFILTER hooks if a tray window"]
    NtUserCallHwndLock pub fn RedrawFrameAndHook(hwnd: HWND) -> BOOL;

    #[doc = ""]
    NtUserCallHwndLock pub fn SetDialogSystemMenu(hwnd: HWND) -> BOOL;

    #[doc = ""]
    NtUserCallHwndLock pub fn StubSetForegroundWindow(hwnd: HWND) -> BOOL;

    #[doc = ""]
    NtUserCallHwndLock pub fn SetSysMenu(hwnd: HWND) -> BOOL;

    #[doc = ""]
    NtUserCallHwndLock pub fn UpdateClientRect(hwnd: HWND) -> BOOL;

    #[doc = ""]
    NtUserCallHwndLock pub fn UpdateWindow(hwnd: HWND) -> BOOL;

    #[doc = "Needs IAM access."]
    NtUserCallHwndLock pub fn SetActiveImmersiveWindow(hwnd: HWND) -> BOOL;

    #[doc = ""]
    NtUserCallHwndLock pub fn SetCancelRotationDelayHintWindow(hwnd: HWND) -> BOOL;

    #[doc = "Needs IAM access."]
    NtUserCallHwndLock pub fn GetWindowTrackInfoAsync(hwnd: HWND) -> BOOL;

    #[doc = ""]
    NtUserCallHwndParamLock pub fn BroadcastImeShowStatusChange(hwnd: HWND, status: BOOL) -> BOOL;

    #[doc = ""]
    NtUserCallHwndParamLock pub fn SetModernAppWindow(hwnd: HWND, modern: HWND) -> BOOL;

    #[doc = ""]
    NtUserCallHwndParamLock pub fn RedrawTitle(hwnd: HWND, param: u32) -> BOOL;

    #[doc = ""]
    NtUserCallHwndParamLock pub fn ShowOwnedPopups(hwnd: HWND, show: BOOL) -> BOOL;

    #[doc = "<https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-switchtothiswindow>"]
    NtUserCallHwndParamLock pub fn SwitchToThisWindow(hwnd: HWND, unknown: BOOL) -> ();

    #[doc = ""]
    NtUserCallHwndParamLock pub fn UpdateWindows(first_hwnd: HWND, region: HRGN) -> BOOL;

    #[doc = "<https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-validatergn>"]
    NtUserCallHwndParamLock pub fn ValidateRgn(hwnd: HWND, hrgn: HRGN) -> BOOL;

    #[doc = "<https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-monitorfromwindow>"]
    NtUserCallHwndParamLock pub fn MonitorFromWindow(hwnd: HWND, dwFlags: u32) -> HMONITOR;

    #[doc = "<https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-enablewindow>"]
    NtUserCallHwndParamLockSafe pub fn EnableWindow(hwnd: HWND, fEnable: BOOL) -> BOOL;

    #[doc = ""]
    NtUserCallTwoParam pub fn ChangeWindowMessageFilter(message: u32, dwFlag: u32) -> BOOL;

    #[doc = "1 = regular, 2 = logical pos from dpi awareness context"]
    NtUserCallTwoParam pub fn GetCursorPos(point: *mut POINT, which: u32) -> BOOL;

    #[doc = ""]
    NtUserCallTwoParam pub fn GetHDevName(hdev: HDEV, buffer: *mut [u8; 64]) -> BOOL;

    #[doc = ""]
    NtUserCallTwoParam pub fn InitAnsiOem(param1: *mut c_void, param2: *mut c_void) -> BOOL;

    #[doc = ""]
    NtUserCallTwoParam pub fn NlsKbdSendIMENotification(param1: u32, param2: u32) -> ();

    #[doc = "May only be called by DWM, returns FALSE with GetLastError() == ERROR_ACCESS_DENIED otherwise."]
    NtUserCallTwoParam pub fn RegisterGhostWindow(hwnd: HWND, ghost: HWND) -> BOOL;

    #[doc = ""]
    NtUserCallTwoParam pub fn RegisterLogonProcess(process_id: u32, param2: usize) -> BOOL;

    #[doc = ""]
    NtUserCallTwoParam pub fn RegisterSiblingFrostWindow(hwnd: HWND, frost: HWND) -> BOOL;

    #[doc = ""]
    NtUserCallTwoParam pub fn RegisterUserHungAppHandlers(unknown: usize, event: HANDLE) -> BOOL;

    #[doc = "May only be called by CSRSS, returns STATUS_ACCESS_DENIED otherwise."]
    NtUserCallTwoParam pub fn RemoteShadowCleanup(buffer: *const c_void, size: usize) -> NTSTATUS;

    #[doc = "May only be called by CSRSS, returns STATUS_ACCESS_DENIED otherwise."]
    NtUserCallTwoParam pub fn RemoteShadowStart(buffer: *const c_void, size: usize) -> NTSTATUS;

    #[doc = "<https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-setcaretpos>"]
    NtUserCallTwoParam pub fn SetCaretPos(x: i32, y: i32) -> BOOL;

    #[doc = "<https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-setcursorpos>"]
    NtUserCallTwoParam pub fn SetCursorPos(x: i32, y: i32) -> BOOL;

    #[doc = "<https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-setphysicalcursorpos>"]
    NtUserCallTwoParam pub fn SetPhysicalCursorPos(x: i32, y: i32) -> BOOL;

    #[doc = ""]
    NtUserCallTwoParam pub fn SetThreadQueueMergeSetting(thread_id: u32, setting: BOOL) -> BOOL;

    #[doc = ""]
    NtUserCallTwoParam pub fn UnhookWindowsHook(hook: i32, param: i32) -> BOOL;

    #[doc = ""]
    NtUserCallTwoParam pub fn WOWCleanup(param1: usize, param2: u32) -> BOOL;

    #[doc = ""]
    NtUserCallTwoParam pub fn EnableShellWindowManagementBehavior(mask: u32, behavior: u32) -> BOOL;

    #[doc = ""]
    NtUserCallTwoParam pub fn CitSetInfo(which: u32, info: *mut c_void) -> NTSTATUS;

    #[doc = ""]
    NtUserCallTwoParam pub fn ScaleSystemMetricForDPIWithoutCache(metric: SYSTEM_METRICS_INDEX, dpi: u32) -> i32;
}

macro_rules! nt_user_call_syscall_fn {
        (($paramname:ident: $paramtype:ty) -> $rettype:ty) => {
            unsafe extern "system" fn syscall<const SYSCALL_NR: usize>(
                $paramname: $paramtype
            ) -> $rettype {
                use std::arch::asm;
                let result;

                asm!(
                    "mov eax, {syscall_nr}",
                    "syscall",
                    in("r10") $paramname,
                    lateout("rax") result,
                    syscall_nr = const(SYSCALL_NR),
                    options(nostack),
                    );

                result
            }
        };

        (($paramname:ident: $paramtype:ty, $param2name:ident: $param2type:ty) -> $rettype:ty) => {
            unsafe extern "system" fn syscall<const SYSCALL_NR: usize>(
                $paramname: $paramtype,
                $param2name: $param2type,
            ) -> $rettype {
                use std::arch::asm;
                let result;

                asm!(
                    "mov eax, {syscall_nr}",
                    "syscall",
                    in("r10") $paramname,
                    in("rdx") $param2name,
                    lateout("rax") result,
                    syscall_nr = const(SYSCALL_NR),
                    options(nostack),
                    );

                result
            }
        };

        (($paramname:ident: $paramtype:ty, $param2name:ident: $param2type:ty, $param3name:ident: $param3type:ty) -> $rettype:ty) => {
            unsafe extern "system" fn syscall<const SYSCALL_NR: usize>(
                $paramname: $paramtype,
                $param2name: $param2type,
                $param3name: $param3type,
            ) -> $rettype {
                use std::arch::asm;
                let result;

                asm!(
                    "mov eax, {syscall_nr}",
                    "syscall",
                    in("r10") $paramname,
                    in("rdx") $param2name,
                    in("r8") $param3name,
                    lateout("rax") result,
                    syscall_nr = const(SYSCALL_NR),
                    options(nostack),
                    );

                result
            }
        };
    }

macro_rules! nt_user_call_alternate {
        ($name:ident => => $rettype:ty => $($paramname:ident: $paramtype:ty),*) => {{
            _ = FUNCTION.compare_exchange(
                std::ptr::null_mut(),
                UserCallError::CallNotFound as _,
                Ordering::SeqCst,
                Ordering::Relaxed,
            );
            return Err(UserCallError::CallNotFound);
        }};

        ($name:ident => $($(#[$cfg:meta])? $os:ident = $syscall_nr:literal),+ => $rettype:ty => $($paramname:ident: $paramtype:ty),*) => {{
            println!(concat!("Function ", stringify!($name), " direct syscall"));

            nt_user_call_syscall_fn!(($($paramname: $paramtype),+) -> $rettype);

            let syscall: unsafe extern "system" fn($($paramtype),*) -> $rettype = match get_os_version() {
                $(
                    $(#[$cfg])?
                    Ok(OsVersion::$os) => syscall::<$syscall_nr>,
                )+
                Ok(_) => {
                    _ = FUNCTION.compare_exchange(
                        std::ptr::null_mut(),
                        UserCallError::OsNotSupported as usize as _,
                        Ordering::SeqCst,
                        Ordering::Relaxed,
                    );

                    return Err(UserCallError::OsNotSupported);
                },
                Err(err) => {
                    _ = FUNCTION.compare_exchange(
                        std::ptr::null_mut(),
                        err as usize as _,
                        Ordering::SeqCst,
                        Ordering::Relaxed,
                    );

                    return Err(err);
                },
            };

            syscall as _
        }};
    }

macro_rules! nt_user_call_syscall {
        (
            $vis:vis fn $name:ident  ($($paramname:ident: $paramtype:ty),*) -> $rettype:ty $(=> $($(#[$cfg:meta])? $os:ident = $syscall_nr:literal),+)?
        ) => {
            #[expect(non_snake_case, clippy::missing_safety_doc)]
            $vis unsafe fn $name($($paramname: $paramtype),*) -> Result<$rettype, UserCallError> {
                type Function = unsafe extern "system" fn($($paramtype),*) -> $rettype;

                static FUNCTION: AtomicPtr<c_void> = AtomicPtr::new(std::ptr::null_mut());

                let mut ptr = FUNCTION.load(Ordering::Relaxed);

                if ptr.is_null() {
                    // SAFETY:
                    let library = match unsafe { GetModuleHandleW(w!("win32u")).or_else(|_| GetModuleHandleW(w!("user32")))  } {
                        Ok(library) => library,
                        Err(_) => {
                            _ = FUNCTION.compare_exchange(std::ptr::null_mut(), 0x1 as _, Ordering::AcqRel, Ordering::Acquire);
                            return Err(UserCallError::LibraryNotFound);
                        }
                    };

                    // SAFETY: GetProcAddress returns a valid function pointer if the function exists.
                    ptr = match unsafe { GetProcAddress(library, PCSTR(concat!(stringify!($name), "\u{0}").as_ptr()))  } {
                        // SAFETY: All syscall signatures are set in stone and will not change.
                        Some(f) => f as _,
                        None => {
                            nt_user_call_alternate!($name =>  $($($(#[$cfg])? $os = $syscall_nr),+)? => $rettype => $($paramname: $paramtype),*)
                        }
                    };

                    ptr = FUNCTION.compare_exchange(std::ptr::null_mut(), ptr, Ordering::AcqRel, Ordering::Acquire).map_or_else(|p| p, |_| ptr);
                }

                if (ptr as usize) < u16::MAX as usize {
                    println!("{:?}", ptr as usize);
                    return Err(UserCallError::try_from(ptr as usize).unwrap());
                }

                // SAFETY: The function pointer has been validated and matches the syscall signature.
                let function: Function = unsafe {
                    std::mem::transmute(ptr)
                };

                // SAFETY: `function` is a valid function.
                Ok(unsafe { function($($paramname),*) })
            }
        };
    }

/// Direct access to the underlying NtUserCall* syscalls.
///
/// <div class="warning">Those syscalls were removed in Windows 11. This module does not provide a reverse mapping to the dedicated syscalls.</div>
pub mod user_call {
    use super::{
        c_void, get_os_version, w, AtomicPtr, GetModuleHandleW, GetProcAddress, Ordering,
        OsVersion, UserCallError, PCSTR,
    };

    nt_user_call_syscall!(pub fn NtUserCallNoParam(call: u32) -> usize => #[cfg(any(target_vendor = "win7", feature = "all_os_versions"))] Win7 = 4101, Win8 = 4102, Win81 = 4103);
    nt_user_call_syscall!(pub fn NtUserCallOneParam(param: usize, call: u32) -> usize => #[cfg(any(target_vendor = "win7", feature = "all_os_versions"))] Win7 = 4098, Win8 = 4099, Win81 = 4100);
    nt_user_call_syscall!(pub fn NtUserCallHwnd(hwnd: usize, call: u32) -> usize => #[cfg(any(target_vendor = "win7", feature = "all_os_versions"))] Win7 = 4364, Win8 = 4364, Win81 = 4365);
    nt_user_call_syscall!(pub fn NtUserCallHwndSafe(hwnd: usize, call: u32) -> usize => #[cfg(any(target_vendor = "win7", feature = "all_os_versions"))] Win7 = 4364, Win8 = 4364, Win81 = 4365);
    nt_user_call_syscall!(pub fn NtUserCallHwndOpt(hwnd: usize, call: u32) -> usize => #[cfg(any(target_vendor = "win7", feature = "all_os_versions"))] Win7 = 4743, Win8 = 4836, Win81 = 4869);
    nt_user_call_syscall!(pub fn NtUserCallHwndParam(hwnd: usize, param: usize, call: u32) -> usize => #[cfg(any(target_vendor = "win7", feature = "all_os_versions"))] Win7 = 4254, Win8 = 4254, Win81 = 4255);
    nt_user_call_syscall!(pub fn NtUserCallHwndLock(hwnd: usize, call: u32) -> usize => #[cfg(any(target_vendor = "win7", feature = "all_os_versions"))] Win7 = 4129, Win8 = 4130, Win81 = 4131);
    nt_user_call_syscall!(pub fn NtUserCallHwndParamLock(hwnd: usize, param: usize, call: u32) -> usize => #[cfg(any(target_vendor = "win7", feature = "all_os_versions"))] Win7 = 4135, Win8 = 4136, Win81 = 4137);
    nt_user_call_syscall!(pub fn NtUserCallHwndParamLockSafe(hwnd: usize, param: usize, call: u32) -> usize => #[cfg(any(target_vendor = "win7", feature = "all_os_versions"))] Win7 = 4135, Win8 = 4136, Win81 = 4137);
    nt_user_call_syscall!(pub fn NtUserCallTwoParam(param1: usize, param2: usize, call: u32) -> usize => #[cfg(any(target_vendor = "win7", feature = "all_os_versions"))] Win7 = 4138, Win8 = 4138, Win81 = 4139);
}
