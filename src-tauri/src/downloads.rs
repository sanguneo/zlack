use std::path::{Path, PathBuf};

use base64::Engine as _;
use serde::Serialize;

/// Hard cap on a single decoded image. Slack images are far smaller; this only
/// guards against a pathological payload coming from the page.
const MAX_IMAGE_BYTES: usize = 64 * 1024 * 1024;
const DEFAULT_STEM: &str = "slack-image";

#[derive(Debug, Serialize)]
#[serde(tag = "kind", content = "message")]
pub(crate) enum SaveImageError {
    EmptyData,
    InvalidData,
    TooLarge,
    NoDownloadDir,
    WriteFailed(String),
}

/// Persist an image the page already fetched (with its own cookies) into the
/// user's Downloads folder. The bytes arrive base64-encoded because the WebView
/// is the only context that can read authenticated `files.slack.com` images,
/// and Tauri v1 IPC is JSON so a compact string beats a raw byte array.
#[tauri::command]
pub(crate) fn save_image(
    app_handle: tauri::AppHandle,
    filename: Option<String>,
    mime: Option<String>,
    data_base64: String,
) -> Result<String, SaveImageError> {
    if data_base64.is_empty() {
        return Err(SaveImageError::EmptyData);
    }
    // base64 expands the payload ~4:3; reject obvious oversizes before decoding.
    if data_base64.len() / 4 * 3 > MAX_IMAGE_BYTES {
        return Err(SaveImageError::TooLarge);
    }

    let data = base64::engine::general_purpose::STANDARD
        .decode(data_base64.as_bytes())
        .map_err(|_| SaveImageError::InvalidData)?;
    if data.is_empty() {
        return Err(SaveImageError::EmptyData);
    }
    if data.len() > MAX_IMAGE_BYTES {
        return Err(SaveImageError::TooLarge);
    }

    let dir = tauri::api::path::download_dir().ok_or(SaveImageError::NoDownloadDir)?;
    std::fs::create_dir_all(&dir).map_err(|error| SaveImageError::WriteFailed(error.to_string()))?;

    let filename = filename.unwrap_or_default();
    let mime = mime.unwrap_or_default();
    let stem = image_stem(&filename);
    let ext = image_extension(&data, &mime, &filename);
    let target = unique_path(&dir, &stem, ext);

    std::fs::write(&target, &data).map_err(|error| SaveImageError::WriteFailed(error.to_string()))?;

    let display_name = target
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(DEFAULT_STEM)
        .to_string();
    crate::native_notifications::show_local_notification(
        &app_handle,
        "Image saved".to_string(),
        display_name,
    );

    Ok(target.to_string_lossy().into_owned())
}

/// Derive a safe file stem from a caller-supplied name, dropping any path parts
/// and characters that are illegal on common filesystems.
fn image_stem(filename: &str) -> String {
    let raw = Path::new(filename)
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or("");
    let cleaned: String = raw
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || matches!(c, '-' | '_' | ' ' | '.') {
                c
            } else {
                '_'
            }
        })
        .collect();
    let collapsed = cleaned.split_whitespace().collect::<Vec<_>>().join(" ");
    let trimmed = collapsed.trim_matches(|c| c == '.' || c == '_' || c == ' ');
    if trimmed.is_empty() {
        DEFAULT_STEM.to_string()
    } else {
        trimmed.chars().take(80).collect()
    }
}

/// Pick an extension from the actual bytes first (most reliable), then the
/// response MIME type, then the URL-derived name, defaulting to `png`.
fn image_extension(data: &[u8], mime: &str, filename: &str) -> &'static str {
    sniff_extension(data)
        .or_else(|| extension_for_mime(mime))
        .or_else(|| extension_from_name(filename))
        .unwrap_or("png")
}

fn sniff_extension(data: &[u8]) -> Option<&'static str> {
    if data.starts_with(&[0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A]) {
        return Some("png");
    }
    if data.starts_with(&[0xFF, 0xD8, 0xFF]) {
        return Some("jpg");
    }
    if data.starts_with(b"GIF87a") || data.starts_with(b"GIF89a") {
        return Some("gif");
    }
    if data.len() >= 12 && &data[0..4] == b"RIFF" && &data[8..12] == b"WEBP" {
        return Some("webp");
    }
    if data.starts_with(b"BM") {
        return Some("bmp");
    }
    if data.starts_with(&[0x00, 0x00, 0x01, 0x00]) {
        return Some("ico");
    }
    None
}

fn extension_for_mime(mime: &str) -> Option<&'static str> {
    let mime = mime
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase();
    match mime.as_str() {
        "image/png" => Some("png"),
        "image/jpeg" | "image/jpg" | "image/pjpeg" => Some("jpg"),
        "image/gif" => Some("gif"),
        "image/webp" => Some("webp"),
        "image/bmp" | "image/x-ms-bmp" => Some("bmp"),
        "image/svg+xml" => Some("svg"),
        "image/tiff" => Some("tiff"),
        "image/avif" => Some("avif"),
        "image/x-icon" | "image/vnd.microsoft.icon" => Some("ico"),
        _ => None,
    }
}

fn extension_from_name(filename: &str) -> Option<&'static str> {
    let ext = Path::new(filename)
        .extension()
        .and_then(|ext| ext.to_str())?
        .to_ascii_lowercase();
    match ext.as_str() {
        "png" => Some("png"),
        "jpg" | "jpeg" => Some("jpg"),
        "gif" => Some("gif"),
        "webp" => Some("webp"),
        "bmp" => Some("bmp"),
        "svg" => Some("svg"),
        "tif" | "tiff" => Some("tiff"),
        "avif" => Some("avif"),
        "ico" => Some("ico"),
        _ => None,
    }
}

/// Never clobber an existing file: fall back to `name (1).ext`, `name (2).ext`...
fn unique_path(dir: &Path, stem: &str, ext: &str) -> PathBuf {
    let first = dir.join(format!("{stem}.{ext}"));
    if !first.exists() {
        return first;
    }
    for n in 1..=9999u32 {
        let candidate = dir.join(format!("{stem} ({n}).{ext}"));
        if !candidate.exists() {
            return candidate;
        }
    }
    first
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sniffs_known_image_signatures() {
        assert_eq!(
            sniff_extension(&[0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0, 0]),
            Some("png")
        );
        assert_eq!(sniff_extension(&[0xFF, 0xD8, 0xFF, 0xE0]), Some("jpg"));
        assert_eq!(sniff_extension(b"GIF89a and more"), Some("gif"));
        let mut webp = Vec::from(&b"RIFF"[..]);
        webp.extend_from_slice(&[0, 0, 0, 0]);
        webp.extend_from_slice(b"WEBPVP8 ");
        assert_eq!(sniff_extension(&webp), Some("webp"));
        assert_eq!(sniff_extension(b"not an image"), None);
    }

    #[test]
    fn extension_prefers_bytes_then_mime_then_name() {
        let png = [0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A];
        // Real PNG bytes beat a misleading name and empty MIME.
        assert_eq!(image_extension(&png, "", "photo.jpg"), "png");
        // Unknown bytes fall back to MIME.
        assert_eq!(image_extension(b"????unknown", "image/webp", "x"), "webp");
        // Unknown bytes and MIME fall back to the URL name.
        assert_eq!(image_extension(b"????unknown", "", "clip.GIF"), "gif");
        // MIME parameters are ignored.
        assert_eq!(image_extension(b"????unknown", "image/png; charset=binary", "x"), "png");
        // Nothing usable defaults to png.
        assert_eq!(image_extension(b"????unknown", "", "noext"), "png");
    }

    #[test]
    fn sanitizes_stems_and_falls_back() {
        assert_eq!(image_stem("my photo.png"), "my photo");
        assert_eq!(image_stem("passwd"), "passwd");
        assert_eq!(image_stem(""), DEFAULT_STEM);
        assert_eq!(image_stem("...."), DEFAULT_STEM);
        let sanitized = image_stem("a?b<c>d");
        assert!(!sanitized.contains('?') && !sanitized.contains('<') && !sanitized.contains('>'));
        // Unicode letters (e.g. Korean) are preserved.
        assert_eq!(image_stem("사진.png"), "사진");
    }

    #[test]
    fn unique_path_avoids_overwrite() {
        let dir = std::env::temp_dir().join(format!("zlack-save-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let first = unique_path(&dir, "img", "png");
        std::fs::write(&first, b"x").unwrap();
        let second = unique_path(&dir, "img", "png");
        assert_ne!(first, second);
        assert!(second
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap()
            .contains("(1)"));
        std::fs::remove_dir_all(&dir).ok();
    }
}
