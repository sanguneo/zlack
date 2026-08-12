use notify_rust::Notification;

pub(super) fn show(
    _app_handle: &tauri::AppHandle,
    notification_id: String,
    title: String,
    body: String,
) {
    std::thread::spawn(move || {
        let handle = Notification::new()
            .summary(&title)
            .body(&body)
            .action("default", "Open")
            .show();

        match handle {
            Ok(handle) => handle.wait_for_action(|action| {
                if action == "default" {
                    super::dispatch_pending_notification(&notification_id);
                } else {
                    super::remove_pending_notification(&notification_id);
                }
            }),
            Err(error) => {
                super::remove_pending_notification(&notification_id);
                eprintln!("Zlack: Failed to show Linux notification: {error}");
            }
        }
    });
}
