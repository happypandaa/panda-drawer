//! Read-only, bounded decoding of author-provided PNG icons.
use crate::ApiError;
use std::io::Cursor;

pub(crate) const ICON_MAX: u64 = 8 * 1024 * 1024;
const ICON_PIXELS: u64 = 4 * 1024 * 1024;
const ICON_DIMENSION: u32 = 4096;
const DECODE_MAX: usize = 32 * 1024 * 1024;
fn invalid(message: impl Into<String>) -> ApiError {
    ApiError::new("INVALID_ICON", message)
}

pub(crate) fn decode_png(bytes: &[u8]) -> Result<(u32, u32), ApiError> {
    if !bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        return Err(invalid("Icon must be a decodable PNG"));
    }
    let mut offset = 8usize;
    let mut ended = false;
    while offset + 12 <= bytes.len() {
        let length = u32::from_be_bytes(bytes[offset..offset + 4].try_into().unwrap()) as usize;
        let end = offset
            .checked_add(length)
            .and_then(|v| v.checked_add(12))
            .ok_or_else(|| invalid("Invalid PNG chunk size"))?;
        if end > bytes.len() {
            return Err(invalid("Truncated PNG chunk"));
        }
        let kind = &bytes[offset + 4..offset + 8];
        if matches!(kind, b"acTL" | b"fcTL" | b"fdAT") {
            return Err(invalid("Animated PNG icons are not supported"));
        }
        offset = end;
        if kind == b"IEND" {
            ended = true;
            break;
        }
    }
    if !ended || offset != bytes.len() {
        return Err(invalid(
            "PNG must have a complete IEND and no trailing content",
        ));
    }
    let mut decoder = png::Decoder::new(Cursor::new(bytes));
    decoder.set_limits(png::Limits { bytes: DECODE_MAX });
    let mut reader = decoder
        .read_info()
        .map_err(|e| invalid(format!("PNG decode failed: {e}")))?;
    let (width, height) = (reader.info().width, reader.info().height);
    if width == 0
        || height == 0
        || width > ICON_DIMENSION
        || height > ICON_DIMENSION
        || u64::from(width) * u64::from(height) > ICON_PIXELS
    {
        return Err(invalid(
            "PNG dimensions exceed the internal 4096-side / 4-megapixel limit",
        ));
    }
    let size = reader
        .output_buffer_size()
        .filter(|v| *v <= DECODE_MAX)
        .ok_or_else(|| invalid("PNG decoded image exceeds memory limit"))?;
    let mut output = vec![0; size];
    reader
        .next_frame(&mut output)
        .map_err(|e| invalid(format!("PNG decode failed: {e}")))?;
    reader
        .finish()
        .map_err(|e| invalid(format!("PNG decode failed: {e}")))?;
    Ok((width, height))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn png_bytes(width: u32, height: u32) -> Vec<u8> {
        let mut bytes = Vec::new();
        {
            let mut encoder = png::Encoder::new(&mut bytes, width, height);
            encoder.set_color(png::ColorType::Rgba);
            encoder.set_depth(png::BitDepth::Eight);
            let mut writer = encoder.write_header().unwrap();
            writer
                .write_image_data(&vec![127; (width * height * 4) as usize])
                .unwrap();
        }
        bytes
    }
    #[test]
    fn png_validation_decodes_content_and_rejects_truncation_animation_and_oversized_dimensions() {
        let valid = png_bytes(2, 3);
        assert_eq!(decode_png(&valid).unwrap(), (2, 3));
        assert!(decode_png(&valid[..valid.len() - 8]).is_err());
        let mut animated = valid.clone();
        animated.splice(8..8, [0, 0, 0, 0, b'a', b'c', b'T', b'L', 0, 0, 0, 0]);
        assert!(decode_png(&animated).is_err());
        assert!(decode_png(&png_bytes(4097, 1)).is_err());
        let mut trailing = valid;
        trailing.push(0);
        assert!(decode_png(&trailing).is_err());
    }
}
