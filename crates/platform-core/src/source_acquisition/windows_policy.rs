//! Windows filename and case comparison for a Git tree before any blob is
//! materialized. Mac's existing normalization and tree budgets remain separate.
use super::*;
use std::cmp::Ordering;

#[link(name = "kernel32")]
unsafe extern "system" {
    fn CompareStringOrdinal(
        first: *const u16,
        first_length: i32,
        second: *const u16,
        second_length: i32,
        ignore_case: i32,
    ) -> i32;
}

pub(super) fn needs_adaptation(path: &str) -> ApiError {
    error(
        "SOURCE_NEEDS_ADAPTATION",
        "仓库包含 Windows 无法使用的文件名或大小写冲突，请修改源码后重新添加。",
    )
    .detail("path", path)
    .detail("nextStep", "review-and-adapt-source")
    .detail("executionState", "not-executed")
}

pub(super) fn validate_names(path: &str) -> Result<(), ApiError> {
    for part in path.split('/') {
        let base = part.split('.').next().unwrap_or("").to_ascii_uppercase();
        let port_device = ["COM", "LPT"].iter().any(|prefix| {
            base.strip_prefix(prefix).is_some_and(|suffix| {
                matches!(
                    suffix,
                    "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9" | "¹" | "²" | "³"
                )
            })
        });
        if part.chars().any(|c| c < ' ' || "<>:\"\\|?*".contains(c))
            || part.ends_with([' ', '.'])
            || part.encode_utf16().count() > 255
            || matches!(base.as_str(), "CON" | "PRN" | "AUX" | "NUL")
            || port_device
        {
            return Err(needs_adaptation(path));
        }
    }
    Ok(())
}

fn compare(first: &[u16], second: &[u16]) -> Result<Ordering, ApiError> {
    let compared = unsafe {
        CompareStringOrdinal(
            first.as_ptr(),
            first.len() as i32,
            second.as_ptr(),
            second.len() as i32,
            1,
        )
    };
    match compared {
        1 => Ok(Ordering::Less),
        2 => Ok(Ordering::Equal),
        3 => Ok(Ordering::Greater),
        _ => Err(invalid("无法核对本机文件名的大小写等价关系。")),
    }
}

pub(super) fn validate_tree(entries: &[TreeEntry]) -> Result<(), ApiError> {
    let mut prefixes = HashSet::new();
    for entry in entries {
        let mut prefix = String::new();
        for part in entry.path.split('/') {
            if !prefix.is_empty() {
                prefix.push('/');
            }
            prefix.push_str(part);
            prefixes.insert(prefix.clone());
        }
    }
    let mut prefixes = prefixes
        .into_iter()
        .map(|name| {
            let key = name.encode_utf16().collect::<Vec<_>>();
            (name, key)
        })
        .collect::<Vec<_>>();
    let mut failed = false;
    prefixes.sort_by(|first, second| {
        compare(&first.1, &second.1).unwrap_or_else(|_| {
            failed = true;
            first.1.cmp(&second.1)
        })
    });
    if failed {
        return Err(invalid("无法核对本机文件名的大小写等价关系。"));
    }
    for pair in prefixes.windows(2) {
        if compare(&pair[0].1, &pair[1].1)? == Ordering::Equal && pair[0].0 != pair[1].0 {
            return Err(needs_adaptation(&pair[1].0));
        }
    }
    Ok(())
}
