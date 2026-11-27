use crate::os::{PlatformEvent, PlatformSender};
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows_sys::Win32::UI::Shell::{DefSubclassProc, RemoveWindowSubclass, SetWindowSubclass};
use windows_sys::Win32::UI::WindowsAndMessaging::{SPI_GETCLIENTAREAANIMATION, SystemParametersInfoW, WM_SETTINGCHANGE};
use winit::window::Window;

pub(super) fn reduced_motion() -> Option<bool> {
    let mut animations_enabled = 0i32;
    // SAFETY: SPI_GETCLIENTAREAANIMATION writes one Win32 BOOL into the supplied pointer.
    let success = unsafe { SystemParametersInfoW(SPI_GETCLIENTAREAANIMATION, 0, (&raw mut animations_enabled).cast(), 0) };
    (success != 0).then_some(animations_enabled == 0)
}

pub(super) struct Observer {
    hwnd: HWND,
    sender: Box<PlatformSender>,
}

impl Observer {
    pub(super) fn new(window: &Window, sender: PlatformSender) -> Option<Self> {
        let RawWindowHandle::Win32(handle) = window.window_handle().ok()?.as_raw() else { return None };
        let sender = Box::new(sender);
        let data = std::ptr::from_ref(sender.as_ref()) as usize;
        let hwnd = handle.hwnd.get() as HWND;
        // SAFETY: Installation and removal run on the owning window thread.
        // The boxed sender remains live for as long as the subclass is installed.
        let installed = unsafe { SetWindowSubclass(hwnd, Some(window_proc), data, data) };
        (installed != 0).then_some(Self { hwnd, sender })
    }
}

impl Drop for Observer {
    fn drop(&mut self) {
        let id = std::ptr::from_ref(self.sender.as_ref()) as usize;
        // SAFETY: RemoveWindowSubclass detaches this lifetime's callback before the sender is freed.
        unsafe { RemoveWindowSubclass(self.hwnd, Some(window_proc), id) };
    }
}

unsafe extern "system" fn window_proc(hwnd: HWND, message: u32, wparam: WPARAM, lparam: LPARAM, _: usize, data: usize) -> LRESULT {
    if message == WM_SETTINGCHANGE {
        // SAFETY: The installed subclass data references the live boxed sender.
        let sender = unsafe { &*(data as *const PlatformSender) };
        sender.send_event(PlatformEvent::RefreshSystemPreferences);
    }
    // SAFETY: Forward this callback's parameters along the window's subclass chain.
    unsafe { DefSubclassProc(hwnd, message, wparam, lparam) }
}
