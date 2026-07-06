#[cfg(target_os = "windows")]
use std::path::PathBuf;

#[cfg(not(target_os = "windows"))]
use tauri::api::notification::Notification;

#[cfg(target_os = "windows")]
use tauri_winrt_notification::{Duration, Sound, Toast};

#[cfg(target_os = "windows")]
const WINDOWS_DEFAULT_DOWNLOAD_FOLDER_NAME: &str = "Desktop";

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
fn windows_desktop_dir() -> Option<PathBuf> {
    use windows::Win32::{
        Foundation::HANDLE,
        System::Com::CoTaskMemFree,
        UI::Shell::{FOLDERID_Desktop, SHGetKnownFolderPath, KF_FLAG_DEFAULT},
    };

    unsafe {
        let path = SHGetKnownFolderPath(&FOLDERID_Desktop, KF_FLAG_DEFAULT, HANDLE(0)).ok()?;
        let desktop = path.to_string().ok().map(PathBuf::from);
        CoTaskMemFree(Some(path.as_ptr() as _));
        desktop
    }
}

#[cfg(target_os = "windows")]
fn windows_default_download_dir() -> PathBuf {
    windows_desktop_dir()
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

#[cfg(target_os = "windows")]
fn notification_target_url(
    team_id: &Option<String>,
    channel_id: &Option<String>,
) -> Option<String> {
    if let (Some(tid), Some(cid)) = (team_id, channel_id) {
        if tid != "unknown" && cid != "unknown" {
            return Some(format!("https://app.slack.com/client/{}/{}", tid, cid));
        } else if tid != "unknown" {
            return Some(format!("https://app.slack.com/client/{}", tid));
        }
    }
    None
}

#[tauri::command]
pub(crate) fn notify(
    app_handle: tauri::AppHandle,
    title: String,
    body: String,
    team_id: Option<String>,
    channel_id: Option<String>,
) {
    #[cfg(target_os = "windows")]
    {
        let app_handle_clone = app_handle.clone();
        let identifier = app_handle.config().tauri.bundle.identifier.clone();
        let target_url = notification_target_url(&team_id, &channel_id);

        // Create the toast on the main thread so the COM apartment/listener stays
        // alive for the duration of the app, rather than dying with a background thread.
        let _ = app_handle.run_on_main_thread(move || {
            let res = Toast::new(&identifier)
                .title(&title)
                .text1(&body)
                .sound(Some(Sound::SMS))
                .duration(Duration::Short)
                .on_activated(move |_| {
                    let app_dispatcher = app_handle_clone.clone();
                    let app_worker = app_handle_clone.clone();
                    let url_to_open = target_url.clone();
                    let team_to_open = team_id.clone();

                    let _ = app_dispatcher.run_on_main_thread(move || {
                        if let Some(team) = team_to_open.filter(|team| team != "unknown") {
                            crate::switch_to_workspace(&app_worker, &team, url_to_open);
                        } else if let Some(window) = crate::active_window(&app_worker) {
                            crate::restore_window(&window);
                        }
                    });
                    Ok(())
                })
                .show();

            if let Err(e) = res {
                eprintln!("Zlack: Failed to show toast: {}", e);
            }
        });
    }

    #[cfg(not(target_os = "windows"))]
    {
        let _ = (&team_id, &channel_id);
        let identifier = app_handle.config().tauri.bundle.identifier.clone();

        let _ = Notification::new(&identifier)
            .title(title)
            .body(body)
            .show();
    }
}
