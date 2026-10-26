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
pub(crate) fn set_default_download_folder(window: &tauri::WebviewWindow) {
    use std::os::windows::ffi::OsStrExt;
    use tauri::Manager;
    use webview2_com::Microsoft::Web::WebView2::Win32::ICoreWebView2_13;
    use windows::core::{Interface, PCWSTR};

    let Ok(download_dir) = window.path().download_dir() else {
        return;
    };
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
pub(crate) fn set_default_download_folder(_window: &tauri::WebviewWindow) {}
