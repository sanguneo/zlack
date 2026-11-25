// allow: SIZE_OK - download persistence is one boundary with shared naming and cleanup invariants.
use std::{
    collections::HashMap,
    fs::{File, OpenOptions},
    io::{Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    sync::Mutex,
};

use base64::Engine as _;
use serde::Serialize;
use tauri::{Manager, WebviewWindow};
use tauri_plugin_dialog::{DialogExt, MessageDialogButtons};

/// Hard cap on a single decoded image. Slack images are far smaller; this only
/// guards against a pathological payload coming from the page.
const MAX_IMAGE_BYTES: usize = 64 * 1024 * 1024;
const MAX_FILE_BYTES: u64 = 256 * 1024 * 1024;
const FILE_CHUNK_BYTES: usize = 256 * 1024;
const MAX_COLLISION_SUFFIX: u32 = 9999;
const DEFAULT_STEM: &str = "slack-image";
const DEFAULT_FILE_STEM: &str = "slack-file";

#[derive(Debug, Serialize)]
#[serde(tag = "kind", content = "message")]
pub(crate) enum SaveError {
    EmptyData,
    InvalidData,
    TooLarge,
    ApprovalDeclined,
    Unauthorized,
    TransferNotFound,
    ChunkOutOfOrder,
    NameExhausted,
    NoDownloadDir,
    WriteFailed(String),
    OpenFailed(String),
    StateFailed,
}

struct Approval {
    owner: String,
    filename: String,
}

struct PendingTransfer {
    owner: String,
    file: Option<File>,
    temp_path: Option<PathBuf>,
    stem: String,
    extension: Option<String>,
    next_chunk: u32,
    bytes_written: u64,
}

impl Drop for PendingTransfer {
    fn drop(&mut self) {
        self.file.take();
        if let Some(path) = self.temp_path.take() {
            if let Err(error) = std::fs::remove_file(&path) {
                if error.kind() != std::io::ErrorKind::NotFound {
                    eprintln!("Zlack: failed to remove partial download: {error}");
                }
            }
        }
    }
}

#[derive(Default)]
struct DownloadSessions {
    approvals: HashMap<String, Approval>,
    transfers: HashMap<String, PendingTransfer>,
}

pub(crate) struct DownloadState {
    sessions: Mutex<DownloadSessions>,
    max_file_bytes: u64,
}

impl Default for DownloadState {
    fn default() -> Self {
        Self {
            sessions: Mutex::new(DownloadSessions::default()),
            max_file_bytes: MAX_FILE_BYTES,
        }
    }
}

impl DownloadState {
    fn approve(&self, owner: &str, filename: &str) -> Result<String, SaveError> {
        let mut random = [0u8; 16];
        getrandom::fill(&mut random).map_err(|_| SaveError::StateFailed)?;
        let token = random
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();
        self.sessions
            .lock()
            .map_err(|_| SaveError::StateFailed)?
            .approvals
            .insert(
                token.clone(),
                Approval {
                    owner: owner.to_string(),
                    filename: filename.to_string(),
                },
            );
        Ok(token)
    }

    fn consume_approval(
        sessions: &mut DownloadSessions,
        owner: &str,
        token: &str,
        filename: &str,
    ) -> Result<(), SaveError> {
        let approval = sessions
            .approvals
            .get(token)
            .ok_or(SaveError::Unauthorized)?;
        if approval.owner != owner || approval.filename != filename {
            return Err(SaveError::Unauthorized);
        }
        sessions.approvals.remove(token);
        Ok(())
    }

    fn begin(
        &self,
        dir: &Path,
        owner: &str,
        token: &str,
        filename: &str,
    ) -> Result<String, SaveError> {
        let mut sessions = self.sessions.lock().map_err(|_| SaveError::StateFailed)?;
        Self::consume_approval(&mut sessions, owner, token, filename)?;
        let temp_path = dir.join(format!(".zlack-download-{token}.part"));
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .open(&temp_path)
            .map_err(|error| SaveError::WriteFailed(error.to_string()))?;
        let stem = sanitize_stem(filename, DEFAULT_FILE_STEM);
        let extension = file_extension(filename);
        sessions.transfers.insert(
            token.to_string(),
            PendingTransfer {
                owner: owner.to_string(),
                file: Some(file),
                temp_path: Some(temp_path),
                stem,
                extension,
                next_chunk: 0,
                bytes_written: 0,
            },
        );
        Ok(token.to_string())
    }

    fn write_chunk(
        &self,
        owner: &str,
        transfer_id: &str,
        chunk_index: u32,
        data_base64: &str,
    ) -> Result<(), SaveError> {
        let mut sessions = self.sessions.lock().map_err(|_| SaveError::StateFailed)?;
        if sessions
            .transfers
            .get(transfer_id)
            .is_none_or(|transfer| transfer.owner != owner)
        {
            return Err(SaveError::Unauthorized);
        }
        let mut transfer = sessions
            .transfers
            .remove(transfer_id)
            .ok_or(SaveError::TransferNotFound)?;
        if transfer.next_chunk != chunk_index {
            return Err(SaveError::ChunkOutOfOrder);
        }
        let data = decode_base64_payload(data_base64, FILE_CHUNK_BYTES)?;
        let chunk_len = u64::try_from(data.len()).map_err(|_| SaveError::TooLarge)?;
        let next_size = transfer
            .bytes_written
            .checked_add(chunk_len)
            .ok_or(SaveError::TooLarge)?;
        if next_size > self.max_file_bytes {
            return Err(SaveError::TooLarge);
        }
        transfer
            .file
            .as_mut()
            .ok_or(SaveError::StateFailed)?
            .write_all(&data)
            .map_err(|error| SaveError::WriteFailed(error.to_string()))?;
        transfer.bytes_written = next_size;
        transfer.next_chunk = transfer
            .next_chunk
            .checked_add(1)
            .ok_or(SaveError::ChunkOutOfOrder)?;
        sessions.transfers.insert(transfer_id.to_string(), transfer);
        Ok(())
    }

    fn finish(&self, dir: &Path, owner: &str, transfer_id: &str) -> Result<PathBuf, SaveError> {
        let mut sessions = self.sessions.lock().map_err(|_| SaveError::StateFailed)?;
        if sessions
            .transfers
            .get(transfer_id)
            .is_none_or(|transfer| transfer.owner != owner)
        {
            return Err(SaveError::Unauthorized);
        }
        let mut transfer = sessions
            .transfers
            .remove(transfer_id)
            .ok_or(SaveError::TransferNotFound)?;
        drop(sessions);
        if transfer.bytes_written == 0 {
            return Err(SaveError::EmptyData);
        }
        let mut source = transfer.file.take().ok_or(SaveError::StateFailed)?;
        source
            .flush()
            .and_then(|()| source.seek(SeekFrom::Start(0)).map(|_| ()))
            .map_err(|error| SaveError::WriteFailed(error.to_string()))?;
        let (target, mut destination) = create_unique_file(
            dir,
            &transfer.stem,
            transfer.extension.as_deref(),
            MAX_COLLISION_SUFFIX,
        )?;
        if let Err(error) =
            std::io::copy(&mut source, &mut destination).and_then(|_| destination.flush())
        {
            drop(destination);
            remove_partial_file(&target);
            return Err(SaveError::WriteFailed(error.to_string()));
        }
        drop(destination);
        drop(source);
        if let Some(temp_path) = transfer.temp_path.take() {
            std::fs::remove_file(temp_path)
                .map_err(|error| SaveError::WriteFailed(error.to_string()))?;
        }
        Ok(target)
    }

    fn cancel(&self, owner: &str, token: &str) -> Result<(), SaveError> {
        let mut sessions = self.sessions.lock().map_err(|_| SaveError::StateFailed)?;
        if sessions
            .approvals
            .get(token)
            .is_some_and(|approval| approval.owner == owner)
        {
            sessions.approvals.remove(token);
            return Ok(());
        }
        if sessions
            .transfers
            .get(token)
            .is_some_and(|transfer| transfer.owner == owner)
        {
            sessions.transfers.remove(token);
            return Ok(());
        }
        Err(SaveError::TransferNotFound)
    }
}

// Off the main thread: blocking_show waits for a dialog the plugin drives from
// the main thread, so a sync command here would deadlock or freeze every window.
#[tauri::command(async)]
pub(crate) fn authorize_download(
    window: WebviewWindow,
    state: tauri::State<'_, DownloadState>,
    filename: Option<String>,
) -> Result<String, SaveError> {
    let filename = filename.unwrap_or_default();
    let display_name = if filename.trim().is_empty() {
        DEFAULT_FILE_STEM
    } else {
        &filename
    };
    let approved = window
        .app_handle()
        .dialog()
        .message(format!("Save {display_name} to Downloads?"))
        .title("Download file")
        .buttons(MessageDialogButtons::OkCancelCustom(
            "Save".into(),
            "Cancel".into(),
        ))
        .blocking_show();
    if !approved {
        return Err(SaveError::ApprovalDeclined);
    }
    state.approve(window.label(), &filename)
}

/// Persist an image the page already fetched (with its own cookies) into the
/// user's Downloads folder. The bytes arrive base64-encoded because the WebView
/// is the only context that can read authenticated `files.slack.com` images,
/// and JSON IPC makes a compact string cheaper than a raw byte array.
#[tauri::command(async)]
pub(crate) fn save_image(
    window: WebviewWindow,
    state: tauri::State<'_, DownloadState>,
    app_handle: tauri::AppHandle,
    authorization: String,
    filename: Option<String>,
    mime: Option<String>,
    data_base64: String,
) -> Result<String, SaveError> {
    let filename = filename.unwrap_or_default();
    state.consume_for_save(window.label(), &authorization, &filename)?;
    let data = decode_base64_payload(&data_base64, MAX_IMAGE_BYTES)?;
    let dir = downloads_dir(&app_handle)?;
    let mime = mime.unwrap_or_default();
    let stem = sanitize_stem(&filename, DEFAULT_STEM);
    let ext = image_extension(&data, &mime, &filename);
    let target = write_unique_file(&dir, &stem, Some(ext), &data)?;

    notify_saved(&app_handle, "Image saved", &target, DEFAULT_STEM);
    Ok(target.to_string_lossy().into_owned())
}

#[tauri::command(async)]
pub(crate) fn start_file_download(
    window: WebviewWindow,
    state: tauri::State<'_, DownloadState>,
    app_handle: tauri::AppHandle,
    authorization: String,
    filename: Option<String>,
) -> Result<String, SaveError> {
    let dir = downloads_dir(&app_handle)?;
    let filename = filename.unwrap_or_default();
    state.begin(&dir, window.label(), &authorization, &filename)
}

#[tauri::command(async)]
pub(crate) fn write_file_chunk(
    window: WebviewWindow,
    state: tauri::State<'_, DownloadState>,
    transfer_id: String,
    chunk_index: u32,
    data_base64: String,
) -> Result<(), SaveError> {
    state.write_chunk(window.label(), &transfer_id, chunk_index, &data_base64)
}

#[tauri::command(async)]
pub(crate) fn finish_file_download(
    window: WebviewWindow,
    state: tauri::State<'_, DownloadState>,
    app_handle: tauri::AppHandle,
    transfer_id: String,
) -> Result<String, SaveError> {
    let dir = downloads_dir(&app_handle)?;
    let target = state.finish(&dir, window.label(), &transfer_id)?;
    notify_saved(&app_handle, "File saved", &target, DEFAULT_FILE_STEM);
    Ok(target.to_string_lossy().into_owned())
}

#[tauri::command]
pub(crate) fn cancel_file_download(
    window: WebviewWindow,
    state: tauri::State<'_, DownloadState>,
    transfer_id: String,
) -> Result<(), SaveError> {
    state.cancel(window.label(), &transfer_id)
}

/// Reveal the Downloads folder in the OS file manager, creating it first if it
/// does not exist yet.
#[tauri::command]
pub(crate) fn open_downloads_folder(app_handle: tauri::AppHandle) -> Result<(), SaveError> {
    let dir = downloads_dir(&app_handle)?;
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

impl DownloadState {
    fn consume_for_save(&self, owner: &str, token: &str, filename: &str) -> Result<(), SaveError> {
        let mut sessions = self.sessions.lock().map_err(|_| SaveError::StateFailed)?;
        Self::consume_approval(&mut sessions, owner, token, filename)
    }
}

fn create_unique_file(
    dir: &Path,
    stem: &str,
    extension: Option<&str>,
    max_suffix: u32,
) -> Result<(PathBuf, File), SaveError> {
    for suffix in 0..=max_suffix {
        let suffix = if suffix == 0 {
            String::new()
        } else {
            format!(" ({suffix})")
        };
        let name = match extension {
            Some(extension) => format!("{stem}{suffix}.{extension}"),
            None => format!("{stem}{suffix}"),
        };
        let path = dir.join(name);
        match OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(file) => return Ok((path, file)),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(SaveError::WriteFailed(error.to_string())),
        }
    }
    Err(SaveError::NameExhausted)
}

fn write_unique_file(
    dir: &Path,
    stem: &str,
    extension: Option<&str>,
    data: &[u8],
) -> Result<PathBuf, SaveError> {
    let (target, mut file) = create_unique_file(dir, stem, extension, MAX_COLLISION_SUFFIX)?;
    if let Err(error) = file.write_all(data).and_then(|()| file.flush()) {
        drop(file);
        remove_partial_file(&target);
        return Err(SaveError::WriteFailed(error.to_string()));
    }
    Ok(target)
}

fn remove_partial_file(path: &Path) {
    if let Err(error) = std::fs::remove_file(path) {
        if error.kind() != std::io::ErrorKind::NotFound {
            eprintln!("Zlack: failed to remove partial download: {error}");
        }
    }
}

fn downloads_dir(app_handle: &tauri::AppHandle) -> Result<PathBuf, SaveError> {
    let dir = app_handle
        .path()
        .download_dir()
        .map_err(|_| SaveError::NoDownloadDir)?;
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
/// and characters that are illegal on common filesystems. Everything else is
/// kept so the saved file carries the name the user actually sees in Slack.
fn sanitize_stem(filename: &str, default: &str) -> String {
    let raw = Path::new(filename)
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or("");
    let cleaned: String = raw
        .chars()
        .map(|c| {
            if c.is_control() || matches!(c, '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*') {
                '_'
            } else {
                c
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

#[cfg(test)]
mod tests {
    use super::*;

    fn test_dir(name: &str) -> PathBuf {
        let mut random = [0u8; 8];
        getrandom::fill(&mut random).unwrap();
        let suffix = random
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();
        let path = std::env::temp_dir().join(format!("zlack-{name}-{suffix}"));
        std::fs::create_dir_all(&path).unwrap();
        path
    }

    fn test_state(max_file_bytes: u64) -> DownloadState {
        DownloadState {
            sessions: Mutex::new(DownloadSessions::default()),
            max_file_bytes,
        }
    }

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
        // Characters that are legal on disk stay, so the saved name matches Slack.
        assert_eq!(
            sanitize_stem("Q3 report (final)+v2!.pdf", DEFAULT_FILE_STEM),
            "Q3 report (final)+v2!"
        );
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
    fn exclusive_file_creation_uses_the_next_available_name() {
        let dir = test_dir("collision");
        std::fs::write(dir.join("img.png"), b"original").unwrap();

        let target = write_unique_file(&dir, "img", Some("png"), b"new").unwrap();

        assert_eq!(
            target.file_name().and_then(|name| name.to_str()),
            Some("img (1).png")
        );
        assert_eq!(std::fs::read(dir.join("img.png")).unwrap(), b"original");
        assert_eq!(std::fs::read(target).unwrap(), b"new");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn exclusive_file_creation_errors_when_names_are_exhausted() {
        let dir = test_dir("exhausted");
        std::fs::write(dir.join("README"), b"0").unwrap();
        std::fs::write(dir.join("README (1)"), b"1").unwrap();

        let result = create_unique_file(&dir, "README", None, 1);

        assert!(matches!(result, Err(SaveError::NameExhausted)));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn chunk_transfer_rejects_out_of_order_data_and_removes_temp_file() {
        let dir = test_dir("chunk-order");
        let state = test_state(MAX_FILE_BYTES);
        let token = state.approve("main", "report.pdf").unwrap();
        let transfer_id = state.begin(&dir, "main", &token, "report.pdf").unwrap();

        let result = state.write_chunk("main", &transfer_id, 1, "aGVsbG8=");

        assert!(matches!(result, Err(SaveError::ChunkOutOfOrder)));
        assert!(!dir.join(format!(".zlack-download-{token}.part")).exists());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn chunk_transfer_writes_in_order_and_finishes_at_a_collision_safe_path() {
        let dir = test_dir("chunk-finish");
        std::fs::write(dir.join("report.pdf"), b"existing").unwrap();
        let state = test_state(MAX_FILE_BYTES);
        let token = state.approve("main", "report.pdf").unwrap();
        let transfer_id = state.begin(&dir, "main", &token, "report.pdf").unwrap();
        state
            .write_chunk("main", &transfer_id, 0, "aGVsbG8g")
            .unwrap();
        state
            .write_chunk("main", &transfer_id, 1, "d29ybGQ=")
            .unwrap();

        let target = state.finish(&dir, "main", &transfer_id).unwrap();

        assert_eq!(
            target.file_name().and_then(|name| name.to_str()),
            Some("report (1).pdf")
        );
        assert_eq!(std::fs::read(target).unwrap(), b"hello world");
        assert_eq!(std::fs::read(dir.join("report.pdf")).unwrap(), b"existing");
        assert!(!dir.join(format!(".zlack-download-{token}.part")).exists());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn chunk_transfer_rejects_cumulative_overflow_and_removes_temp_file() {
        let dir = test_dir("chunk-limit");
        let state = test_state(4);
        let token = state.approve("main", "report.pdf").unwrap();
        let transfer_id = state.begin(&dir, "main", &token, "report.pdf").unwrap();

        let result = state.write_chunk("main", &transfer_id, 0, "aGVsbG8=");

        assert!(matches!(result, Err(SaveError::TooLarge)));
        assert!(!dir.join(format!(".zlack-download-{token}.part")).exists());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn cancelling_transfer_removes_temp_file() {
        let dir = test_dir("cancel");
        let state = test_state(MAX_FILE_BYTES);
        let token = state.approve("main", "report.pdf").unwrap();
        let transfer_id = state.begin(&dir, "main", &token, "report.pdf").unwrap();
        let temp_path = dir.join(format!(".zlack-download-{token}.part"));
        assert!(temp_path.exists());

        state.cancel("main", &transfer_id).unwrap();

        assert!(!temp_path.exists());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn approval_is_one_time_and_bound_to_the_requesting_window() {
        let dir = test_dir("approval");
        let state = test_state(MAX_FILE_BYTES);
        let token = state.approve("main", "report.pdf").unwrap();

        assert!(matches!(
            state.begin(&dir, "other", &token, "report.pdf"),
            Err(SaveError::Unauthorized)
        ));
        assert!(matches!(
            state.begin(&dir, "main", &token, "other.pdf"),
            Err(SaveError::Unauthorized)
        ));
        let transfer_id = state.begin(&dir, "main", &token, "report.pdf").unwrap();
        state.cancel("main", &transfer_id).unwrap();
        assert!(matches!(
            state.begin(&dir, "main", &token, "report.pdf"),
            Err(SaveError::Unauthorized)
        ));
        std::fs::remove_dir_all(dir).unwrap();
    }
}
