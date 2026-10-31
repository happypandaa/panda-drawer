//! Resolve and encode an explicit Windows environment without mutating the host.
use super::wide;
use std::{cmp::Ordering, ffi::OsString, io};
use windows_sys::Win32::Globalization::{
    CompareStringOrdinal, CSTR_EQUAL, CSTR_GREATER_THAN, CSTR_LESS_THAN,
};

fn compare_keys(left: &[u16], right: &[u16]) -> io::Result<Ordering> {
    let left_len = i32::try_from(left.len())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "Environment key is too long"))?;
    let right_len = i32::try_from(right.len())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "Environment key is too long"))?;
    let compared =
        unsafe { CompareStringOrdinal(left.as_ptr(), left_len, right.as_ptr(), right_len, 1) };
    match compared {
        CSTR_LESS_THAN => Ok(Ordering::Less),
        CSTR_EQUAL => Ok(Ordering::Equal),
        CSTR_GREATER_THAN => Ok(Ordering::Greater),
        _ => Err(io::Error::last_os_error()),
    }
}

fn same_key(left: &OsString, right: &OsString) -> io::Result<bool> {
    let left = wide(left)?;
    let right = wide(right)?;
    compare_keys(&left[..left.len() - 1], &right[..right.len() - 1])
        .map(|order| order == Ordering::Equal)
}

/// Windows compares environment keys using its ordinal, case-insensitive
/// UTF-16 mapping. Unicode uppercase expansion and lossy decoding can merge
/// distinct names, so neither participates in sorting or duplicate detection.
pub(super) fn block(values: &[(OsString, OsString)]) -> io::Result<Vec<u16>> {
    let mut sorted: Vec<(Vec<u16>, Vec<u16>)> = Vec::with_capacity(values.len());
    for (name, value) in values {
        let mut name = wide(name)?;
        name.pop();
        // The leading '=' of Windows hidden entries (for example =C:) is
        // part of their name. Any subsequent '=' is the value separator.
        if name.is_empty() || name[1..].contains(&(b'=' as u16)) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "Invalid environment key",
            ));
        }
        let value = wide(value)?;
        let (mut low, mut high) = (0, sorted.len());
        while low < high {
            let middle = low + (high - low) / 2;
            match compare_keys(&sorted[middle].0, &name)? {
                Ordering::Less => low = middle + 1,
                Ordering::Greater => high = middle,
                Ordering::Equal => {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidInput,
                        "Duplicate environment key",
                    ));
                }
            }
        }
        sorted.insert(low, (name, value));
    }
    let mut result = Vec::new();
    for (name, value) in sorted {
        result.extend(name);
        result.push(b'=' as u16);
        result.extend(value);
    }
    if result.is_empty() {
        result.push(0);
    }
    result.push(0);
    Ok(result)
}

fn apply(
    mut values: Vec<(OsString, OsString)>,
    changes: &[(OsString, Option<OsString>)],
) -> io::Result<Vec<(OsString, OsString)>> {
    for (key, value) in changes {
        let mut matching = None;
        for (index, (existing, _)) in values.iter().enumerate() {
            if same_key(existing, key)? {
                matching = Some(index);
                break;
            }
        }
        match (matching, value) {
            (Some(index), Some(value)) => values[index] = (key.clone(), value.clone()),
            (Some(index), None) => {
                values.remove(index);
            }
            (None, Some(value)) => values.push((key.clone(), value.clone())),
            (None, None) => (),
        }
    }
    Ok(values)
}

pub(super) fn resolve(
    inherit: bool,
    changes: &[(OsString, Option<OsString>)],
) -> io::Result<Vec<(OsString, OsString)>> {
    let inherited = if inherit {
        std::env::vars_os().collect()
    } else {
        Vec::new()
    };
    apply(inherited, changes)
}

#[cfg(test)]
pub(super) fn non_ascii_fixture_values() -> Vec<(OsString, OsString)> {
    use std::os::windows::ffi::OsStringExt;
    vec![
        ("ß".into(), "sharp-s".into()),
        (
            OsString::from_wide(&[b'P' as u16, 0xd801]),
            OsString::from_wide(&[0xd800]),
        ),
        ("=Z:".into(), r"Z:\panda-disposable-only".into()),
        ("SS".into(), "two-letters".into()),
        (
            OsString::from_wide(&[b'P' as u16, 0xd800]),
            OsString::from_wide(&[0xd801]),
        ),
        ("=C:".into(), r"C:\panda-disposable-only".into()),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::windows::ffi::{OsStrExt, OsStringExt};

    fn entries(block: &[u16]) -> Vec<&[u16]> {
        assert!(block.ends_with(&[0, 0]));
        block
            .split(|unit| *unit == 0)
            .filter(|item| !item.is_empty())
            .collect()
    }

    #[test]
    fn encoded_keys_follow_native_ordinal_order_without_unicode_or_utf16_loss() {
        let encoded = block(&non_ascii_fixture_values()).unwrap();
        let entries = entries(&encoded);
        let keys = entries
            .iter()
            .map(|item| {
                let separator = item[1..]
                    .iter()
                    .position(|unit| *unit == b'=' as u16)
                    .unwrap()
                    + 1;
                &item[..separator]
            })
            .collect::<Vec<_>>();
        let expected = [
            vec![61, 67, 58],
            vec![61, 90, 58],
            vec![80, 0xd800],
            vec![80, 0xd801],
            vec![83, 83],
            vec![223],
        ];
        assert_eq!(keys, expected.iter().map(Vec::as_slice).collect::<Vec<_>>());
        for pair in keys.windows(2) {
            assert_eq!(compare_keys(pair[0], pair[1]).unwrap(), Ordering::Less);
        }
        assert_eq!(entries[2], &[80, 0xd800, 61, 0xd801]);
        assert_eq!(entries[3], &[80, 0xd801, 61, 0xd800]);
    }

    #[test]
    fn ascii_encoding_and_invalid_input_rules_remain_explicit() {
        assert_eq!(block(&[]).unwrap(), [0, 0]);
        assert_eq!(
            block(&[("z".into(), "last".into()), ("a".into(), "first".into())]).unwrap(),
            "a=first\0z=last\0\0".encode_utf16().collect::<Vec<_>>()
        );
        for values in [
            vec![("Path".into(), "one".into()), ("PATH".into(), "two".into())],
            vec![("Äkey".into(), "one".into()), ("äKEY".into(), "two".into())],
            vec![("".into(), "value".into())],
            vec![("ordinary=key".into(), "value".into())],
            vec![("=C:=extra".into(), "value".into())],
            vec![("nul\0key".into(), "value".into())],
            vec![("key".into(), "nul\0value".into())],
        ] {
            assert_eq!(
                block(&values).unwrap_err().kind(),
                io::ErrorKind::InvalidInput
            );
        }
    }

    #[test]
    fn explicit_changes_use_native_case_insensitive_keys_and_preserve_utf16() {
        let mut raw_value: Vec<u16> = "中文-".encode_utf16().collect();
        raw_value.push(0xd800);
        let value = OsString::from_wide(&raw_value);
        let changes = vec![
            ("PATH".into(), Some("owned-node".into())),
            ("äKEY".into(), None),
            ("PANDA_RAW".into(), Some(value)),
        ];
        let values = apply(
            vec![
                ("Path".into(), "host-path".into()),
                ("Äkey".into(), "removed".into()),
                ("KEEP".into(), "kept".into()),
            ],
            &changes,
        )
        .unwrap();
        assert_eq!(values.len(), 3);
        assert!(values.contains(&("PATH".into(), "owned-node".into())));
        assert!(values.contains(&("KEEP".into(), "kept".into())));
        assert_eq!(
            values
                .iter()
                .find(|(key, _)| key == "PANDA_RAW")
                .unwrap()
                .1
                .encode_wide()
                .collect::<Vec<_>>(),
            raw_value
        );
    }

    #[test]
    fn cleared_policy_does_not_inherit_even_when_changes_are_empty() {
        assert!(resolve(false, &[]).unwrap().is_empty());
        assert_eq!(
            resolve(false, &[("PANDA_OWNED".into(), Some("only".into()))]).unwrap(),
            vec![("PANDA_OWNED".into(), "only".into())]
        );
    }
}
