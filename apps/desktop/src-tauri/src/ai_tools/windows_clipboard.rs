//! A manager-initiated clipboard write can finish after focus has moved to
//! the external editor. WebView2's focused-document clipboard API cannot.
use super::{ApiError, MANAGER_TEXT_LIMIT};
use std::{mem, ptr, thread, time::Duration};
use windows::{
    core::Owned,
    Win32::{
        Foundation::{HANDLE, HWND},
        System::{
            DataExchange::{CloseClipboard, EmptyClipboard, OpenClipboard, SetClipboardData},
            Memory::{GlobalAlloc, GlobalLock, GlobalUnlock, GMEM_MOVEABLE},
            Ole::CF_UNICODETEXT,
        },
    },
};

pub(super) fn unavailable() -> ApiError {
    ApiError::new("CLIPBOARD_WRITE_FAILED", "无法写入剪贴板，请重试。")
}

pub(super) fn unicode_text(text: &str) -> Result<Vec<u16>, ApiError> {
    if text.chars().nth(MANAGER_TEXT_LIMIT).is_some()
        || text.trim().is_empty()
        || text.contains('\0')
    {
        return Err(ApiError::new(
            "HANDOFF_TEXT_INVALID",
            "无法复制这段提示词。",
        ));
    }
    // CF_UNICODETEXT requires CRLF line breaks and one terminal NUL.
    let normalized = text.replace("\r\n", "\n").replace(['\r', '\n'], "\r\n");
    Ok(normalized.encode_utf16().chain(Some(0)).collect())
}

struct Clipboard;

impl Clipboard {
    fn open(owner: HWND) -> Result<Self, ApiError> {
        for attempt in 0..6 {
            if attempt != 0 {
                thread::sleep(Duration::from_millis(20));
            }
            if unsafe { OpenClipboard(Some(owner)) }.is_ok() {
                return Ok(Self);
            }
        }
        Err(unavailable())
    }
}

impl Drop for Clipboard {
    fn drop(&mut self) {
        let _ = unsafe { CloseClipboard() };
    }
}

pub(super) fn write(window: &tauri::WebviewWindow, value: &[u16]) -> Result<(), ApiError> {
    let owner = HWND(window.hwnd().map_err(|_| unavailable())?.0);
    // Allocate and fill before taking the clipboard, so failures here leave
    // its current contents intact. The block is freed on every error path.
    let memory = unsafe {
        Owned::new(GlobalAlloc(GMEM_MOVEABLE, mem::size_of_val(value)).map_err(|_| unavailable())?)
    };
    let destination = unsafe { GlobalLock(*memory) }.cast::<u16>();
    if destination.is_null() {
        return Err(unavailable());
    }
    unsafe { ptr::copy_nonoverlapping(value.as_ptr(), destination, value.len()) };
    // The final unlock returns FALSE with NO_ERROR, which the bindings
    // expose as Err with a successful HRESULT.
    if unsafe { GlobalUnlock(*memory) }.is_err_and(|error| error.code().is_err()) {
        return Err(unavailable());
    }
    let _clipboard = Clipboard::open(owner)?;
    unsafe {
        EmptyClipboard().map_err(|_| unavailable())?;
        SetClipboardData(CF_UNICODETEXT.0.into(), Some(HANDLE((*memory).0)))
            .map_err(|_| unavailable())?;
    }
    mem::forget(memory); // Windows now owns the HGLOBAL.
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clipboard_text_preserves_unicode_and_encodes_windows_line_breaks() {
        let expected: Vec<_> = "中文 🐼\r\nline\r\nend\r\n\0".encode_utf16().collect();
        assert_eq!(unicode_text("中文 🐼\nline\r\nend\r").unwrap(), expected);
    }

    #[test]
    fn clipboard_text_rejects_nul_and_obeys_the_manager_prompt_limit_in_scalars() {
        assert!(unicode_text("  ").is_err());
        assert!(unicode_text("first\0second").is_err());
        let limit = "🐼".repeat(MANAGER_TEXT_LIMIT);
        assert_eq!(
            unicode_text(&limit).unwrap().len(),
            2 * MANAGER_TEXT_LIMIT + 1
        );
        assert!(unicode_text(&(limit + "x")).is_err());
    }
}
