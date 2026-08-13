#[cfg(target_os = "windows")]
use std::path::PathBuf;

#[cfg(target_os = "windows")]
const WINDOWS_DEFAULT_DOWNLOAD_FOLDER_NAME: &str = "Downloads";

#[cfg(target_os = "windows")]
pub(crate) fn prefer_private_webview2_runtime() {
    if std::env::var_os("WEBVIEW2_BROWSER_EXECUTABLE_FOLDER").is_some() {
        return;
    }
    if let Some(runtime) = crate::exe_sibling("webview2-runtime") {
        if runtime.join("msedgewebview2.exe").is_file() {
            std::env::set_var("WEBVIEW2_BROWSER_EXECUTABLE_FOLDER", runtime);
        }
    }
}

#[cfg(not(target_os = "windows"))]
pub(crate) fn prefer_private_webview2_runtime() {}

#[cfg(target_os = "windows")]
fn windows_downloads_dir() -> Option<PathBuf> {
    use windows::Win32::{
        Foundation::HANDLE,
        System::Com::CoTaskMemFree,
        UI::Shell::{FOLDERID_Downloads, SHGetKnownFolderPath, KF_FLAG_DEFAULT},
    };

    unsafe {
        let path = SHGetKnownFolderPath(&FOLDERID_Downloads, KF_FLAG_DEFAULT, HANDLE(0)).ok()?;
        let downloads = path.to_string().ok().map(PathBuf::from);
        CoTaskMemFree(Some(path.as_ptr() as _));
        downloads
    }
}

#[cfg(target_os = "windows")]
fn windows_default_download_dir() -> PathBuf {
    windows_downloads_dir()
        .or_else(|| {
            std::env::var_os("USERPROFILE")
                .map(PathBuf::from)
                .map(|home| home.join(WINDOWS_DEFAULT_DOWNLOAD_FOLDER_NAME))
        })
        .unwrap_or_else(|| PathBuf::from(WINDOWS_DEFAULT_DOWNLOAD_FOLDER_NAME))
}

#[cfg(target_os = "windows")]
pub(crate) fn set_default_download_folder(window: &tauri::Window) {
    use std::os::windows::ffi::OsStrExt;
    use webview2_com::Microsoft::Web::WebView2::Win32::ICoreWebView2_13;
    use windows_webview2::core::{Interface, PCWSTR};

    let download_dir = windows_default_download_dir();
    let _ = std::fs::create_dir_all(&download_dir);
    let _ = window.with_webview(move |webview| {
        let download_dir: Vec<u16> = download_dir
            .as_os_str()
            .encode_wide()
            .chain(std::iter::once(0))
            .collect();

        unsafe {
            let _ = webview
                .controller()
                .CoreWebView2()
                .and_then(|webview| webview.cast::<ICoreWebView2_13>())
                .and_then(|webview| webview.Profile())
                .and_then(|profile| {
                    profile.SetDefaultDownloadFolderPath(PCWSTR::from_raw(download_dir.as_ptr()))
                });
        }
    });
}

#[cfg(not(target_os = "windows"))]
pub(crate) fn set_default_download_folder(_window: &tauri::Window) {}
