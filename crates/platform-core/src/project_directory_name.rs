//! Readable names for newly allocated user project roots only. Identity remains
//! the complete project ID; this function neither selects nor touches a path.
use sha2::{Digest, Sha256};
use std::sync::OnceLock;

const MAX_SLUG_BYTES: usize = 48;
const DATA: &str = include_str!("project_directory_name_data/mapping.tsv");
struct Mapping {
    scalar: u32,
    flags: u8,
    fragment: &'static str,
}
fn mappings() -> &'static [Mapping] {
    static MAPPINGS: OnceLock<Vec<Mapping>> = OnceLock::new();
    MAPPINGS.get_or_init(|| {
        DATA.lines()
            .filter(|line| !line.starts_with('#'))
            .map(|line| {
                let mut fields = line.splitn(3, '\t');
                Mapping {
                    scalar: u32::from_str_radix(fields.next().expect("generated scalar"), 16)
                        .expect("generated scalar"),
                    flags: fields
                        .next()
                        .expect("generated flags")
                        .parse()
                        .expect("generated flags"),
                    fragment: fields.next().expect("generated ASCII fragment"),
                }
            })
            .collect()
    })
}
fn push(slug: &mut String, byte: u8) {
    if slug.len() < MAX_SLUG_BYTES && (byte != b'-' || (!slug.is_empty() && !slug.ends_with('-'))) {
        slug.push(char::from(byte));
    }
}
fn reserved(slug: &str) -> bool {
    matches!(slug, "con" | "prn" | "aux" | "nul")
        || (slug.len() == 4
            && (slug.starts_with("com") || slug.starts_with("lpt"))
            && matches!(slug.as_bytes()[3], b'1'..=b'9'))
}

/// Returns `<ASCII-readable-slug>-<8 hex SHA256(full_id) digits>`.
/// The suffix is a compact display aid, not a uniqueness proof. The caller must
/// atomically allocate without replacing an existing directory.
pub fn readable_directory_name(display_name: &str, full_id: &str) -> String {
    let table = mappings();
    let mut slug = String::with_capacity(MAX_SLUG_BYTES);
    let mut previous_chinese = false;
    for character in display_name.chars() {
        let mapping = table
            .binary_search_by_key(&u32::from(character), |entry| entry.scalar)
            .ok()
            .map(|index| &table[index]);
        let boundary = previous_chinese && mapping.is_none_or(|entry| entry.flags & 2 != 0);
        if boundary {
            push(&mut slug, b'-');
        }
        if let Some(entry) = mapping {
            for byte in entry.fragment.bytes() {
                push(&mut slug, byte);
            }
            previous_chinese = entry.flags & 1 != 0 && (!entry.fragment.is_empty() || boundary);
        } else {
            previous_chinese = false;
        }
        if slug.len() >= MAX_SLUG_BYTES {
            break;
        }
    }
    while slug.ends_with('-') {
        slug.pop();
    }
    if slug.is_empty() {
        slug.push_str("project");
    }
    if reserved(&slug) {
        slug.insert_str(0, "project-");
    }
    let digest = format!("{:x}", Sha256::digest(full_id.as_bytes()));
    format!("{slug}-{}", &digest[..8])
}

#[cfg(test)]
#[path = "project_directory_name_tests.rs"]
mod tests;
