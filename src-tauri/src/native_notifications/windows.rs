use tauri_winrt_notification::{Duration, Sound, Toast};

pub(super) fn show(
    app_handle: &tauri::AppHandle,
    notification_id: String,
    title: String,
    body: String,
) {
    let identifier = app_handle.config().tauri.bundle.identifier.clone();
    let activation_id = notification_id.clone();
    let failed_id = notification_id.clone();
    let scheduling_failed_id = notification_id;

    let result = app_handle.run_on_main_thread(move || {
        let result = Toast::new(&identifier)
            .title(&title)
            .text1(&body)
            .sound(Some(Sound::SMS))
            .duration(Duration::Short)
            .on_activated(move |_| {
                super::dispatch_pending_notification(&activation_id);
                Ok(())
            })
            .show();

        if let Err(error) = result {
            super::remove_pending_notification(&failed_id);
            eprintln!("Zlack: Failed to show toast: {error}");
        }
    });
    if let Err(error) = result {
        super::remove_pending_notification(&scheduling_failed_id);
        eprintln!("Zlack: Failed to schedule toast creation: {error}");
    }
}
