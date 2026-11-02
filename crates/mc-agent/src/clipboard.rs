//! Clipboard access, both directions.
//!
//! Only text is synchronised in v1. The clipboard is polled rather than hooked,
//! which keeps the implementation small and works identically on both systems.

#[cfg(windows)]
pub fn get_text() -> Option<String> {
    use windows::Win32::Foundation::HGLOBAL;
    use windows::Win32::System::DataExchange::{CloseClipboard, GetClipboardData};
    use windows::Win32::System::Memory::{GlobalLock, GlobalSize, GlobalUnlock};
    const CF_UNICODETEXT: u32 = 13;
    unsafe {
        if !open_clipboard_with_retry() {
            return None;
        }
        let result = (|| {
            let handle = GetClipboardData(CF_UNICODETEXT).ok()?;
            let ptr = GlobalLock(HGLOBAL(handle.0)) as *const u16;
            if ptr.is_null() {
                return None;
            }
            // Bound the scan by the real allocation; never walk off the block.
            let capacity = (GlobalSize(HGLOBAL(handle.0)) / 2) as usize;
            let mut len = 0usize;
            while len < capacity && *ptr.add(len) != 0 {
                len += 1;
            }
            let slice = std::slice::from_raw_parts(ptr, len);
            let text = String::from_utf16_lossy(slice);
            let _ = GlobalUnlock(HGLOBAL(handle.0));
            Some(text)
        })();
        let _ = CloseClipboard();
        result
    }
}

/// Another application may hold the clipboard open for a few milliseconds;
/// retry briefly instead of dropping an update.
#[cfg(windows)]
unsafe fn open_clipboard_with_retry() -> bool {
    use windows::Win32::System::DataExchange::OpenClipboard;
    for _ in 0..8 {
        if OpenClipboard(None).is_ok() {
            return true;
        }
        std::thread::sleep(std::time::Duration::from_millis(15));
    }
    false
}

#[cfg(windows)]
pub fn set_text(text: &str) {
    use windows::Win32::Foundation::{GlobalFree, HANDLE};
    use windows::Win32::System::DataExchange::{CloseClipboard, EmptyClipboard, SetClipboardData};
    use windows::Win32::System::Memory::{GlobalAlloc, GlobalLock, GlobalUnlock, GMEM_MOVEABLE};
    const CF_UNICODETEXT: u32 = 13;
    let wide: Vec<u16> = text.encode_utf16().chain(std::iter::once(0)).collect();
    let bytes = wide.len() * 2;
    unsafe {
        let Ok(handle) = GlobalAlloc(GMEM_MOVEABLE, bytes) else {
            return;
        };
        let ptr = GlobalLock(handle) as *mut u16;
        if ptr.is_null() {
            let _ = GlobalFree(handle);
            return;
        }
        std::ptr::copy_nonoverlapping(wide.as_ptr(), ptr, wide.len());
        let _ = GlobalUnlock(handle);
        if !open_clipboard_with_retry() {
            let _ = GlobalFree(handle);
            return;
        }
        let _ = EmptyClipboard();
        // Ownership of the block transfers to the clipboard on success; on
        // failure it is ours to free.
        if SetClipboardData(CF_UNICODETEXT, HANDLE(handle.0)).is_err() {
            let _ = GlobalFree(handle);
        }
        let _ = CloseClipboard();
    }
}

#[cfg(target_os = "macos")]
pub fn get_text() -> Option<String> {
    use objc2_app_kit::{NSPasteboard, NSPasteboardTypeString};
    unsafe {
        let pasteboard = NSPasteboard::generalPasteboard();
        let value = pasteboard.stringForType(NSPasteboardTypeString)?;
        Some(value.to_string())
    }
}

#[cfg(target_os = "macos")]
pub fn set_text(text: &str) {
    use objc2_app_kit::{NSPasteboard, NSPasteboardTypeString};
    use objc2_foundation::NSString;
    unsafe {
        let pasteboard = NSPasteboard::generalPasteboard();
        pasteboard.clearContents();
        let value = NSString::from_str(text);
        pasteboard.setString_forType(&value, NSPasteboardTypeString);
    }
}

#[cfg(not(any(windows, target_os = "macos")))]
pub fn get_text() -> Option<String> {
    None
}

#[cfg(not(any(windows, target_os = "macos")))]
pub fn set_text(_text: &str) {}
