use std::{
    ffi::{c_char, CStr, CString, NulError},
    panic::{catch_unwind, AssertUnwindSafe},
    sync::Once,
};

// SAFETY: These declarations match the C functions compiled from
// macos/notification.m. The callback is invoked synchronously with a
// non-null, NUL-terminated UTF-8 identifier valid for the callback duration.
unsafe extern "C" {
    fn zlack_notification_initialize(callback: extern "C" fn(*const c_char, bool));
    fn zlack_notification_show(
        notification_id: *const c_char,
        title: *const c_char,
        body: *const c_char,
    );
}

extern "C" fn handle_notification_response(notification_id: *const c_char, activated: bool) {
    let result = catch_unwind(AssertUnwindSafe(|| {
        handle_notification_response_inner(notification_id, activated);
    }));
    if result.is_err() {
        eprintln!("Zlack: macOS notification callback panicked");
    }
}

fn handle_notification_response_inner(notification_id: *const c_char, activated: bool) {
    if notification_id.is_null() {
        return;
    }
    // SAFETY: notification.m passes NSString.UTF8String directly into this
    // callback and keeps the NSString alive until the callback returns.
    let Ok(notification_id) = unsafe { CStr::from_ptr(notification_id) }.to_str() else {
        return;
    };

    if activated {
        super::dispatch_pending_notification(notification_id);
    } else {
        super::remove_pending_notification(notification_id);
    }
}

pub(super) fn show(
    _app_handle: &tauri::AppHandle,
    notification_id: String,
    title: String,
    body: String,
) {
    if let Err(error) = show_inner(&notification_id, &title, &body) {
        super::remove_pending_notification(&notification_id);
        eprintln!("Zlack: Failed to show macOS notification: {error}");
    }
}

fn show_inner(notification_id: &str, title: &str, body: &str) -> Result<(), NulError> {
    static INITIALIZE: Once = Once::new();
    // SAFETY: The callback is an extern "C" function with static lifetime
    // and the Objective-C side stores exactly that function pointer.
    INITIALIZE.call_once(|| unsafe {
        zlack_notification_initialize(handle_notification_response);
    });

    let notification_id = CString::new(notification_id)?;
    let title = CString::new(title)?;
    let body = CString::new(body)?;
    // SAFETY: All pointers are valid NUL-terminated C strings for this call.
    // notification.m copies them into NSString values before returning and
    // before dispatching the asynchronous delivery block.
    unsafe {
        zlack_notification_show(notification_id.as_ptr(), title.as_ptr(), body.as_ptr());
    }
    Ok(())
}
