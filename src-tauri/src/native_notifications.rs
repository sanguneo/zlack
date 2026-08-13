use std::{
    cmp::Reverse,
    collections::{BinaryHeap, HashMap},
    sync::{
        mpsc::{self, Receiver, RecvTimeoutError, Sender},
        Mutex, OnceLock,
    },
    thread,
    time::{Duration, Instant},
};

use crate::notifications::{activate_native_notification, NotificationActivation};

#[cfg(all(unix, not(target_os = "macos")))]
mod linux;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "windows")]
mod windows;

#[cfg(all(unix, not(target_os = "macos")))]
use linux as backend;
#[cfg(target_os = "macos")]
use macos as backend;
#[cfg(target_os = "windows")]
use windows as backend;

struct PendingNativeNotification {
    app_handle: tauri::AppHandle,
    origin_label: String,
    activation: NotificationActivation,
    created_at: Instant,
}

const PENDING_RETENTION: Duration = Duration::from_secs(10 * 60);
type CleanupSchedule = Reverse<(Instant, String, Instant)>;

fn pending_notifications() -> &'static Mutex<HashMap<String, PendingNativeNotification>> {
    static PENDING: OnceLock<Mutex<HashMap<String, PendingNativeNotification>>> = OnceLock::new();
    PENDING.get_or_init(|| Mutex::new(HashMap::new()))
}

fn cleanup_sender() -> &'static Sender<CleanupSchedule> {
    static CLEANUP_SENDER: OnceLock<Sender<CleanupSchedule>> = OnceLock::new();
    CLEANUP_SENDER.get_or_init(|| {
        let (sender, receiver) = mpsc::channel();
        thread::Builder::new()
            .name("zlack-notification-cleanup".to_string())
            .spawn(move || cleanup_pending_notifications(receiver))
            .expect("notification cleanup worker starts");
        sender
    })
}

fn cleanup_pending_notifications(receiver: Receiver<CleanupSchedule>) {
    let mut deadlines: BinaryHeap<CleanupSchedule> = BinaryHeap::new();
    loop {
        let received = match deadlines.peek() {
            Some(Reverse((deadline, _, _))) => {
                receiver.recv_timeout(deadline.saturating_duration_since(Instant::now()))
            }
            None => match receiver.recv() {
                Ok(schedule) => {
                    deadlines.push(schedule);
                    continue;
                }
                Err(_) => return,
            },
        };

        match received {
            Ok(schedule) => deadlines.push(schedule),
            Err(RecvTimeoutError::Timeout) => remove_due_notifications(&mut deadlines),
            Err(RecvTimeoutError::Disconnected) => return,
        }
    }
}

fn remove_due_notifications(deadlines: &mut BinaryHeap<CleanupSchedule>) {
    let now = Instant::now();
    while let Some(Reverse((deadline, notification_id, created_at))) = deadlines.peek() {
        if *deadline > now {
            break;
        }
        let notification_id = notification_id.clone();
        let created_at = *created_at;
        deadlines.pop();

        let mut notifications = pending_notifications().lock().unwrap();
        if notifications
            .get(&notification_id)
            .is_some_and(|pending| pending.created_at == created_at)
        {
            notifications.remove(&notification_id);
        }
    }
}

fn remove_expired_notifications(notifications: &mut HashMap<String, PendingNativeNotification>) {
    notifications.retain(|_, pending| pending.created_at.elapsed() < PENDING_RETENTION);
}

fn register_pending_notification(
    app_handle: tauri::AppHandle,
    origin_label: String,
    activation: NotificationActivation,
) -> String {
    let notification_id = activation.notification_id().to_string();
    let created_at = Instant::now();
    let mut notifications = pending_notifications().lock().unwrap();
    remove_expired_notifications(&mut notifications);
    notifications.insert(
        notification_id.clone(),
        PendingNativeNotification {
            app_handle,
            origin_label,
            activation,
            created_at,
        },
    );
    drop(notifications);

    if let Err(error) = cleanup_sender().send(Reverse((
        created_at + PENDING_RETENTION,
        notification_id.clone(),
        created_at,
    ))) {
        eprintln!("Zlack: Failed to schedule notification cleanup: {error}");
    }
    notification_id
}

pub(super) fn remove_pending_notification(notification_id: &str) {
    pending_notifications()
        .lock()
        .unwrap()
        .remove(notification_id);
}

pub(super) fn dispatch_pending_notification(notification_id: &str) {
    let pending = {
        let mut notifications = pending_notifications().lock().unwrap();
        remove_expired_notifications(&mut notifications);
        notifications.remove(notification_id)
    };
    if let Some(pending) = pending {
        dispatch_activation(pending.app_handle, pending.origin_label, pending.activation);
    }
}

fn dispatch_activation(
    app_handle: tauri::AppHandle,
    origin_label: String,
    activation: NotificationActivation,
) {
    let dispatcher = app_handle.clone();
    if let Err(error) = dispatcher.run_on_main_thread(move || {
        activate_native_notification(&app_handle, &origin_label, activation);
    }) {
        eprintln!("Zlack: Failed to dispatch notification activation: {error}");
    }
}

#[tauri::command]
pub(crate) fn update_notification_context(
    notification_id: String,
    team_id: Option<String>,
    channel_id: Option<String>,
) {
    let mut notifications = pending_notifications().lock().unwrap();
    remove_expired_notifications(&mut notifications);
    if let Some(pending) = notifications.get_mut(&notification_id) {
        pending.activation.update_context(team_id, channel_id);
    }
}

#[tauri::command]
pub(crate) fn notify(
    window: tauri::Window,
    app_handle: tauri::AppHandle,
    notification_id: String,
    title: String,
    body: String,
    team_id: Option<String>,
    channel_id: Option<String>,
) {
    let activation = NotificationActivation::new(notification_id, team_id, channel_id);
    let notification_id =
        register_pending_notification(app_handle.clone(), window.label().to_string(), activation);
    backend::show(&app_handle, notification_id, title, body);
}

/// Show a fire-and-forget local toast (for example "Image saved") that reuses
/// the platform notification backend. Its id is never registered for
/// activation, so clicking the toast is a harmless no-op.
pub(crate) fn show_local_notification(app_handle: &tauri::AppHandle, title: String, body: String) {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let notification_id = format!("zlack-local-{}", COUNTER.fetch_add(1, Ordering::Relaxed));
    backend::show(app_handle, notification_id, title, body);
}
