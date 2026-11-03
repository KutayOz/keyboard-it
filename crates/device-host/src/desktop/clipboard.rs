//! Unicode text only. Clipboard ownership is a hidden native window, not NULL.
//! OpenClipboard(NULL) cannot safely be followed by Empty/SetClipboardData.
use windows::core::w;
use windows::Win32::{
    Foundation::{HANDLE, HGLOBAL, HWND},
    System::{DataExchange::*, Memory::*},
    UI::WindowsAndMessaging::*,
};
pub struct Clipboard {
    window: HWND,
}
impl Clipboard {
    pub fn new() -> windows::core::Result<Self> {
        unsafe {
            let window = CreateWindowExW(
                WINDOW_EX_STYLE(0),
                w!("STATIC"),
                w!("keyboard-it clipboard"),
                WINDOW_STYLE(0),
                0,
                0,
                0,
                0,
                Some(HWND_MESSAGE),
                None,
                None,
                None,
            )?;
            Ok(Self { window })
        }
    }
    pub fn pump(&self) {
        unsafe {
            let mut msg = MSG::default();
            while PeekMessageW(&mut msg, Some(self.window), 0, 0, PM_REMOVE).as_bool() {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
    }
    pub fn read(&self) -> Option<String> {
        unsafe {
            OpenClipboard(Some(self.window)).ok()?;
            let result = (|| {
                let h = GetClipboardData(13).ok()?;
                let global = HGLOBAL(h.0);
                let size = GlobalSize(global);
                if size > 2 * (protocol::desktop::MAX_CLIPBOARD + 1) {
                    return None;
                }
                let p = GlobalLock(global) as *const u16;
                if p.is_null() {
                    return None;
                }
                let all = std::slice::from_raw_parts(p, size / 2);
                let n = all.iter().position(|v| *v == 0).unwrap_or(all.len());
                let s = String::from_utf16(&all[..n]).ok();
                let _ = GlobalUnlock(global);
                s.filter(|v| v.len() <= protocol::desktop::MAX_CLIPBOARD)
            })();
            let _ = CloseClipboard();
            result
        }
    }
    pub fn write(&self, text: &str) -> bool {
        unsafe {
            if text.len() > protocol::desktop::MAX_CLIPBOARD || text.contains('\0') {
                return false;
            }
            if OpenClipboard(Some(self.window)).is_err() {
                return false;
            }
            let result = (|| -> windows::core::Result<()> {
                let data: Vec<u16> = text.encode_utf16().chain(Some(0)).collect();
                let mem = GlobalAlloc(GMEM_MOVEABLE, data.len() * 2)?;
                let p = GlobalLock(mem) as *mut u16;
                if p.is_null() {
                    let _ = windows::Win32::Foundation::GlobalFree(Some(mem));
                    return Err(windows::core::Error::from_thread());
                }
                std::ptr::copy_nonoverlapping(data.as_ptr(), p, data.len());
                let _ = GlobalUnlock(mem);
                if EmptyClipboard()
                    .and_then(|_| SetClipboardData(13, Some(HANDLE(mem.0))).map(|_| ()))
                    .is_err()
                {
                    let _ = windows::Win32::Foundation::GlobalFree(Some(mem));
                    return Err(windows::core::Error::from_thread());
                }
                Ok(())
            })()
            .is_ok();
            let _ = CloseClipboard();
            result
        }
    }
}
impl Drop for Clipboard {
    fn drop(&mut self) {
        unsafe {
            let _ = DestroyWindow(self.window);
        }
    }
}
