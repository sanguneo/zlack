use std::path::{Path, PathBuf};

use base64::Engine as _;
use serde::Serialize;

/// Hard cap on a single decoded image. Slack images are far smaller; this only
/// guards against a pathological payload coming from the page.
const MAX_IMAGE_BYTES: usize = 64 * 1024 * 1024;
/// General attachments can be much larger than images, but the payload crosses
/// Tauri's JSON IPC as base64 (~1.33x the bytes held in memory at once), so an
/// explicit ceiling still applies instead of accepting unbounded files.
const MAX_FILE_BYTES: usize = 256 * 1024 * 1024;
const DEFAULT_STEM: &str = "slack-image";
const DEFAULT_FILE_STEM: &str = "slack-file";

#[derive(Debug, Serialize)]
#[serde(tag = "kind", content = "message")]
pub(crate) enum SaveError {
    EmptyData,
    InvalidData,
    TooLarge,
    NoDownloadDir,
    WriteFailed(String),
    OpenFailed(String),
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
) -> Result<String, SaveError> {
    let data = decode_base64_payload(&data_base64, MAX_IMAGE_BYTES)?;
    let dir = downloads_dir()?;

    let filename = filename.unwrap_or_default();
    let mime = mime.unwrap_or_default();
    let stem = sanitize_stem(&filename, DEFAULT_STEM);
    let ext = image_extension(&data, &mime, &filename);
    let target = unique_path(&dir, &stem, Some(ext));

    std::fs::write(&target, &data).map_err(|error| SaveError::WriteFailed(error.to_string()))?;

    notify_saved(&app_handle, "Image saved", &target, DEFAULT_STEM);
    Ok(target.to_string_lossy().into_owned())
}

/// Persist a non-image attachment (PDF, ZIP, ...) the page fetched with its own
/// cookies. Unlike `save_image`, the extension comes from the caller-supplied
/// filename alone — no image sniffing and no forced fallback extension.
#[tauri::command]
pub(crate) fn save_file(
    app_handle: tauri::AppHandle,
    filename: Option<String>,
    data_base64: String,
) -> Result<String, SaveError> {
    let data = decode_base64_payload(&data_base64, MAX_FILE_BYTES)?;
    let dir = downloads_dir()?;

    let filename = filename.unwrap_or_default();
    let stem = sanitize_stem(&filename, DEFAULT_FILE_STEM);
    let ext = file_extension(&filename);
    let target = unique_path(&dir, &stem, ext.as_deref());

    std::fs::write(&target, &data).map_err(|error| SaveError::WriteFailed(error.to_string()))?;

    notify_saved(&app_handle, "File saved", &target, DEFAULT_FILE_STEM);
    Ok(target.to_string_lossy().into_owned())
}

/// Reveal the Downloads folder in the OS file manager, creating it first if it
/// does not exist yet.
#[tauri::command]
pub(crate) fn open_downloads_folder() -> Result<(), SaveError> {
    let dir = downloads_dir()?;
    open::that(&dir).map_err(|error| SaveError::OpenFailed(error.to_string()))
}

fn decode_base64_payload(data_base64: &str, max_bytes: usize) -> Result<Vec<u8>, SaveError> {
    if data_base64.is_empty() {
        return Err(SaveError::EmptyData);
    }
    // base64 expands the payload ~4:3; reject obvious oversizes before decoding.
    if data_base64.len() / 4 * 3 > max_bytes {
        return Err(SaveError::TooLarge);
    }

    let data = base64::engine::general_purpose::STANDARD
        .decode(data_base64.as_bytes())
        .map_err(|_| SaveError::InvalidData)?;
    if data.is_empty() {
        return Err(SaveError::EmptyData);
    }
    if data.len() > max_bytes {
        return Err(SaveError::TooLarge);
    }
    Ok(data)
}

fn downloads_dir() -> Result<PathBuf, SaveError> {
    let dir = tauri::api::path::download_dir().ok_or(SaveError::NoDownloadDir)?;
    std::fs::create_dir_all(&dir).map_err(|error| SaveError::WriteFailed(error.to_string()))?;
    Ok(dir)
}

fn notify_saved(app_handle: &tauri::AppHandle, title: &str, target: &Path, fallback: &str) {
    let display_name = target
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(fallback)
        .to_string();
    crate::native_notifications::show_local_notification(
        app_handle,
        title.to_string(),
        display_name,
    );
}

/// Derive a safe file stem from a caller-supplied name, dropping any path parts
/// and characters that are illegal on common filesystems.
fn sanitize_stem(filename: &str, default: &str) -> String {
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
        default.to_string()
    } else {
        trimmed.chars().take(80).collect()
    }
}

/// Extension for a general file, taken from the caller-supplied name only.
/// Restricted to short ASCII alphanumerics so a hostile name cannot smuggle in
/// separators; a missing or unusable extension yields `None` (no forced one).
fn file_extension(filename: &str) -> Option<String> {
    let ext = Path::new(filename)
        .extension()
        .and_then(|ext| ext.to_str())?;
    let cleaned: String = ext
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .take(16)
        .collect::<String>()
        .to_ascii_lowercase();
    if cleaned.is_empty() {
        None
    } else {
        Some(cleaned)
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
/// Files without a usable extension get no trailing dot.
fn unique_path(dir: &Path, stem: &str, ext: Option<&str>) -> PathBuf {
    let name = |suffix: &str| match ext {
        Some(ext) => format!("{stem}{suffix}.{ext}"),
        None => format!("{stem}{suffix}"),
    };
    let first = dir.join(name(""));
    if !first.exists() {
        return first;
    }
    for n in 1..=9999u32 {
        let candidate = dir.join(name(&format!(" ({n})")));
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
        assert_eq!(
            image_extension(b"????unknown", "image/png; charset=binary", "x"),
            "png"
        );
        // Nothing usable defaults to png.
        assert_eq!(image_extension(b"????unknown", "", "noext"), "png");
    }

    #[test]
    fn sanitizes_stems_and_falls_back() {
        assert_eq!(sanitize_stem("my photo.png", DEFAULT_STEM), "my photo");
        assert_eq!(sanitize_stem("passwd", DEFAULT_STEM), "passwd");
        assert_eq!(sanitize_stem("", DEFAULT_STEM), DEFAULT_STEM);
        assert_eq!(sanitize_stem("....", DEFAULT_FILE_STEM), DEFAULT_FILE_STEM);
        let sanitized = sanitize_stem("a?b<c>d", DEFAULT_STEM);
        assert!(!sanitized.contains('?') && !sanitized.contains('<') && !sanitized.contains('>'));
        // Unicode letters (e.g. Korean) are preserved.
        assert_eq!(sanitize_stem("사진.png", DEFAULT_STEM), "사진");
        // Multi-dot names keep the inner dots in the stem.
        assert_eq!(
            sanitize_stem("archive.tar.gz", DEFAULT_FILE_STEM),
            "archive.tar"
        );
    }

    #[test]
    fn file_extension_is_sanitized_or_absent() {
        assert_eq!(file_extension("report.PDF"), Some("pdf".to_string()));
        assert_eq!(file_extension("archive.tar.gz"), Some("gz".to_string()));
        assert_eq!(file_extension("noext"), None);
        assert_eq!(file_extension(""), None);
        // Illegal characters are stripped rather than written to disk.
        assert_eq!(file_extension("evil.p?d"), Some("pd".to_string()));
        // An extension with nothing usable left disappears entirely.
        assert_eq!(file_extension("weird.???"), None);
    }

    #[test]
    fn decode_base64_payload_validates_input() {
        assert!(matches!(
            decode_base64_payload("", 16),
            Err(SaveError::EmptyData)
        ));
        assert!(matches!(
            decode_base64_payload("not base64!!", 1024),
            Err(SaveError::InvalidData)
        ));
        let encoded = base64::engine::general_purpose::STANDARD.encode(b"hello");
        assert_eq!(decode_base64_payload(&encoded, 1024).unwrap(), b"hello");
        assert!(matches!(
            decode_base64_payload(&encoded, 4),
            Err(SaveError::TooLarge)
        ));
    }

    #[test]
    fn unique_path_avoids_overwrite() {
        let dir = std::env::temp_dir().join(format!("zlack-save-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let first = unique_path(&dir, "img", Some("png"));
        std::fs::write(&first, b"x").unwrap();
        let second = unique_path(&dir, "img", Some("png"));
        assert_ne!(first, second);
        assert!(second
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap()
            .contains("(1)"));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn unique_path_handles_missing_extension() {
        let dir = std::env::temp_dir().join(format!("zlack-save-noext-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let first = unique_path(&dir, "README", None);
        assert_eq!(first.file_name().and_then(|n| n.to_str()), Some("README"));
        std::fs::write(&first, b"x").unwrap();
        let second = unique_path(&dir, "README", None);
        assert_eq!(
            second.file_name().and_then(|n| n.to_str()),
            Some("README (1)")
        );
        std::fs::remove_dir_all(&dir).ok();
    }
}
