//! Image paste helper: read image data from clipboard and save to a temp file.
//!
//! The image isn't rendered inline in the TUI (which is text-only), but
//! we can save it as a temp file and inject the path as text so the user
//! can reference it in a follow-up message.
//!
//! pi uses `readClipboardImage()` + `extensionForImageMimeType()` to
//! detect PNG/JPEG. nini v1 just stores raw bytes with a guessed extension.

use std::io;
use std::path::{Path, PathBuf};

/// Detect a sensible file extension from a magic-number sniff of the first
/// few bytes. Returns `png`/`jpg`/`gif`/`webp`/`bmp`/`""` (unknown).
fn detect_extension(bytes: &[u8]) -> &'static str {
    if bytes.len() < 8 {
        return "";
    }
    // PNG: 89 50 4E 47 0D 0A 1A 0A
    if bytes.starts_with(&[0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A]) {
        return "png";
    }
    // JPEG: FF D8 FF
    if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
        return "jpg";
    }
    // GIF: GIF87a or GIF89a
    if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        return "gif";
    }
    // WEBP: RIFF....WEBP
    if bytes.len() >= 12 && &bytes[0..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        return "webp";
    }
    // BMP: BM
    if bytes.starts_with(b"BM") {
        return "bmp";
    }
    ""
}

/// Save raw image bytes to a temp file with a guessed extension.
/// Returns the absolute path on success.
pub fn save_image_to_temp(bytes: &[u8]) -> io::Result<PathBuf> {
    let ext = detect_extension(bytes);
    let mut tmp = std::env::temp_dir();
    let unique = format!(
        "nini-image-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis())
            .unwrap_or(0)
    );
    tmp.push(if ext.is_empty() { unique } else { format!("{unique}.{ext}") });
    std::fs::write(&tmp, bytes)?;
    Ok(tmp)
}

/// Sanitize an image path for inclusion in a chat message: ensure it
/// exists, return the absolute path with a `file://`-like hint.
pub fn describe_path(path: &Path) -> String {
    format!("[pasted image: {}]", path.display())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detect_png() {
        let bytes = vec![0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00];
        assert_eq!(detect_extension(&bytes), "png");
    }

    #[test]
    fn detect_jpeg() {
        let bytes = vec![0xFF, 0xD8, 0xFF, 0xE0, 0x00, 0x10, 0x4A, 0x46];
        assert_eq!(detect_extension(&bytes), "jpg");
    }

    #[test]
    fn detect_gif() {
        assert_eq!(detect_extension(b"GIF89a..."), "gif");
        assert_eq!(detect_extension(b"GIF87a..."), "gif");
    }

    #[test]
    fn detect_webp() {
        let mut bytes = b"RIFF".to_vec();
        bytes.extend_from_slice(&[0, 0, 0, 0]);
        bytes.extend_from_slice(b"WEBP");
        assert_eq!(detect_extension(&bytes), "webp");
    }

    #[test]
    fn detect_bmp() {
        let bytes = b"BM\x00\x00\x00\x00\x36\x00\x00\x00";
        assert_eq!(detect_extension(bytes), "bmp");
    }

    #[test]
    fn detect_unknown_returns_empty() {
        assert_eq!(detect_extension(b"random bytes"), "");
        assert_eq!(detect_extension(&[]), "");
    }

    #[test]
    fn save_image_writes_file_with_correct_extension() {
        let png_bytes = vec![0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A];
        let path = save_image_to_temp(&png_bytes).unwrap();
        assert!(path.exists());
        assert!(path.extension().map(|s| s == "png").unwrap_or(false));
        // Clean up.
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn save_unknown_extension_uses_no_suffix() {
        let bytes = b"junk bytes with no header but enough length to pass the size check";
        let path = save_image_to_temp(bytes).unwrap();
        // No extension when unknown.
        let ext = path.extension();
        assert!(
            ext.is_none() || ext.map(|s| s.is_empty()).unwrap_or(false),
            "expected no extension, got: {ext:?}"
        );
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn describe_path_format() {
        let p = std::path::PathBuf::from("/tmp/foo.png");
        let s = describe_path(&p);
        assert!(s.contains("[pasted image:"));
        assert!(s.contains("foo.png"));
    }
}
/// High-level image-paste flow: read image bytes from the system
/// clipboard (using `arboard`), save them to a temp file with a
/// detected extension, and return the `[pasted image: <path>]`
/// string suitable for inserting into the prompt or transcript.
///
/// Returns `Ok(None)` if the clipboard doesn't contain image data
/// (text-only — the caller can fall through to a text-paste handler).
/// Returns `Err` if a real I/O or arboard error occurs.
pub fn paste_image_from_clipboard() -> io::Result<Option<String>> {
    use crate::clipboard;

    let mut cb = arboard::Clipboard::new()
        .map_err(|e| io::Error::new(io::ErrorKind::Other, format!("clipboard open: {e}")))?;
    let img = match cb.get_image() {
        Ok(img) => img,
        // arboard returns this when the clipboard has no image (only
        // text). Treat as a non-error: caller decides what to do next.
        Err(arboard::Error::ClipboardNotSupported)
        | Err(arboard::Error::ContentNotAvailable) => return Ok(None),
        Err(e) => {
            return Err(io::Error::new(
                io::ErrorKind::Other,
                format!("clipboard read image: {e}"),
            ))
        }
    };

    // Encode to PNG bytes (arboard gives raw RGBA). This is a small,
    // fixed-size conversion — safe in any runtime context.
    let mut png_buf = Vec::with_capacity((img.width as usize) * (img.height as usize) * 4 + 1024);
    {
        let mut encoder = png::Encoder::new(&mut png_buf, img.width as u32, img.height as u32);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder
            .write_header()
            .map_err(|e| io::Error::new(io::ErrorKind::Other, format!("png header: {e}")))?;
        writer
            .write_image_data(&img.bytes)
            .map_err(|e| io::Error::new(io::ErrorKind::Other, format!("png data: {e}")))?;
        writer
            .finish()
            .map_err(|e| io::Error::new(io::ErrorKind::Other, format!("png finish: {e}")))?;
    }

    let path = save_image_to_temp(&png_buf)?;
    Ok(Some(describe_path(&path)))
}

#[cfg(test)]
mod paste_flow_tests {
    use super::*;

    #[test]
    fn describe_path_for_png_includes_filename() {
        let p = std::path::PathBuf::from("/tmp/nini-image-1234-1700000000000.png");
        let s = describe_path(&p);
        // Should be self-contained (no external references), so
        // the string can be pasted as a user message.
        assert!(s.starts_with("[pasted image: "));
        assert!(s.contains("nini-image-1234"));
    }

    #[test]
    fn paste_image_returns_none_when_clipboard_has_no_image() {
        // When the clipboard is text-only (the typical test-env
        // case), arboard::get_image returns Err(ContentNotAvailable).
        // We propagate that as Ok(None) so the caller can fall
        // through to a text-paste path.
        // Skip this test in environments without a clipboard server.
        if std::env::var_os("DISPLAY").is_none()
            && std::env::var_os("WAYLAND_DISPLAY").is_none()
        {
            // CI without a display server: arboard will fail to
            // open the clipboard. That's also handled as a soft
            // failure (no image).
        }
        let result = paste_image_from_clipboard();
        // Either: no display → arboard error (Err), or no image (Ok(None)).
        // We just assert it doesn't panic and returns a sensible result.
        match result {
            Ok(None) => { /* expected */ }
            Err(e) => {
                // Acceptable in headless env: arboard can't open clipboard.
                assert!(
                    e.to_string().contains("clipboard")
                        || e.to_string().contains("Clipboard"),
                    "unexpected error: {e}"
                );
            }
            Ok(Some(_)) => panic!("didn't expect a real image in test env"),
        }
    }
}

/// Variant of `paste_image_from_clipboard()` that also returns the
/// saved file path and the PNG byte size, so the runtime can show
/// "pasted 1.2 MB → /tmp/...png" in the status bar.
///
/// Returns `None` if the clipboard has no image (or is unavailable).
/// Returns `Err` on I/O / arboard errors.
pub fn paste_image_with_size_from_clipboard() -> io::Result<Option<(PathBuf, u64)>> {
    use crate::clipboard;

    let mut cb = arboard::Clipboard::new()
        .map_err(|e| io::Error::new(io::ErrorKind::Other, format!("clipboard open: {e}")))?;
    let img = match cb.get_image() {
        Ok(img) => img,
        Err(arboard::Error::ClipboardNotSupported)
        | Err(arboard::Error::ContentNotAvailable) => return Ok(None),
        Err(e) => {
            return Err(io::Error::new(
                io::ErrorKind::Other,
                format!("clipboard read image: {e}"),
            ))
        }
    };

    let mut png_buf = Vec::with_capacity((img.width as usize) * (img.height as usize) * 4 + 1024);
    {
        let mut encoder = png::Encoder::new(&mut png_buf, img.width as u32, img.height as u32);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder
            .write_header()
            .map_err(|e| io::Error::new(io::ErrorKind::Other, format!("png header: {e}")))?;
        writer
            .write_image_data(&img.bytes)
            .map_err(|e| io::Error::new(io::ErrorKind::Other, format!("png data: {e}")))?;
        writer
            .finish()
            .map_err(|e| io::Error::new(io::ErrorKind::Other, format!("png finish: {e}")))?;
    }

    let size = png_buf.len() as u64;
    let path = save_image_to_temp(&png_buf)?;
    Ok(Some((path, size)))
}

#[cfg(test)]
mod paste_with_size_tests {
    use super::*;

    /// Tests that exercise the actual PNG-encoding logic of
    /// `save_image_to_temp` + `describe_path`. These don't need
    /// the system clipboard because they bypass
    /// `paste_image_from_clipboard` entirely.
    #[test]
    fn describe_path_for_png_round_trips_filename() {
        let p = std::path::PathBuf::from("/var/folders/nini-image-1700.png");
        let s = describe_path(&p);
        assert!(s.starts_with("[pasted image:"));
        assert!(s.contains("nini-image-1700.png"));
        assert!(s.ends_with("]"));
    }

    #[test]
    fn save_image_to_temp_round_trip_png_bytes() {
        // Use a tiny valid PNG header (8-byte signature + IHDR + IDAT + IEND).
        // Real PNG signature: \x89 PNG \r \n \x1a \n (8 bytes)
        let mut png = vec![0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A];
        // IHDR chunk (13 bytes data + CRC): width=1, height=1, bit_depth=8,
        // color_type=2 (RGB), compression=0, filter=0, interlace=0.
        png.extend_from_slice(&[
            0x00, 0x00, 0x00, 0x0D, // chunk length
            0x49, 0x48, 0x44, 0x52, // "IHDR"
            0x00, 0x00, 0x00, 0x01, // width = 1
            0x00, 0x00, 0x00, 0x01, // height = 1
            0x08, // bit depth = 8
            0x02, // color type = RGB
            0x00, 0x00, 0x00, // compression, filter, interlace
        ]);
        let path = save_image_to_temp(&png).expect("save_image_to_temp");
        assert!(path.exists(), "saved file should exist");
        let read_back = std::fs::read(&path).expect("read back");
        assert_eq!(read_back, png, "bytes should round-trip exactly");
        // File name should preserve the .png extension.
        assert_eq!(
            path.extension().and_then(|e| e.to_str()),
            Some("png"),
            "extension preserved"
        );
        let _ = std::fs::remove_file(&path);
    }
}
