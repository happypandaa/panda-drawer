//! Read the user's preferred interface language without changing OS preferences.

pub fn default_ui_locale() -> &'static str {
    locale_for_language(preferred_ui_language().as_deref().unwrap_or(""))
}

pub fn locale_for_language(language: &str) -> &'static str {
    if language.to_ascii_lowercase().starts_with("zh") {
        "zh-CN"
    } else {
        "en"
    }
}

#[cfg(target_os = "macos")]
pub fn preferred_ui_language() -> Option<String> {
    use std::ffi::{c_char, c_void, CStr};
    #[link(name = "CoreFoundation", kind = "framework")]
    unsafe extern "C" {
        fn CFLocaleCopyPreferredLanguages() -> *const c_void;
        fn CFArrayGetCount(array: *const c_void) -> isize;
        fn CFArrayGetValueAtIndex(array: *const c_void, index: isize) -> *const c_void;
        fn CFStringGetCString(
            string: *const c_void,
            buffer: *mut c_char,
            size: isize,
            encoding: u32,
        ) -> u8;
        fn CFRelease(value: *const c_void);
    }
    // Copy returns an owned array; its first CFString is borrowed until release.
    unsafe {
        let languages = CFLocaleCopyPreferredLanguages();
        if languages.is_null() {
            return None;
        }
        let result = if CFArrayGetCount(languages) > 0 {
            let language = CFArrayGetValueAtIndex(languages, 0);
            let mut buffer = [0 as c_char; 256];
            if !language.is_null()
                && CFStringGetCString(
                    language,
                    buffer.as_mut_ptr(),
                    buffer.len() as isize,
                    0x0800_0100,
                ) != 0
            {
                CStr::from_ptr(buffer.as_ptr())
                    .to_str()
                    .ok()
                    .map(str::to_owned)
            } else {
                None
            }
        } else {
            None
        };
        CFRelease(languages);
        result
    }
}

#[cfg(windows)]
pub fn preferred_ui_language() -> Option<String> {
    use windows_sys::Win32::Globalization::{GetUserPreferredUILanguages, MUI_LANGUAGE_NAME};
    let mut count = 0;
    let mut length = 0;
    unsafe {
        if GetUserPreferredUILanguages(
            MUI_LANGUAGE_NAME,
            &mut count,
            std::ptr::null_mut(),
            &mut length,
        ) == 0
            || length == 0
            || length > 65536
        {
            return None;
        }
        let mut buffer = vec![0u16; length as usize];
        if GetUserPreferredUILanguages(
            MUI_LANGUAGE_NAME,
            &mut count,
            buffer.as_mut_ptr(),
            &mut length,
        ) == 0
            || count == 0
        {
            return None;
        }
        let end = buffer.iter().position(|&unit| unit == 0)?;
        String::from_utf16(&buffer[..end]).ok()
    }
}

#[cfg(not(any(target_os = "macos", windows)))]
pub fn preferred_ui_language() -> Option<String> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn maps_chinese_variants_and_other_languages() {
        for language in ["zh", "zh-CN", "zh-Hant-TW", "ZH-HK"] {
            assert_eq!(locale_for_language(language), "zh-CN");
        }
        for language in ["en-US", "ja-JP", "fr", ""] {
            assert_eq!(locale_for_language(language), "en");
        }
    }
    #[test]
    #[cfg(any(target_os = "macos", windows))]
    fn reads_the_actual_users_preferred_language() {
        let language = preferred_ui_language().expect("OS preferred UI language");
        assert!(!language.is_empty());
        assert_eq!(default_ui_locale(), locale_for_language(&language));
    }
}
