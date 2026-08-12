use serde::Serialize;

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct NotificationActivation {
    notification_id: String,
    team_id: Option<String>,
    channel_id: Option<String>,
}

impl NotificationActivation {
    pub(crate) fn new(
        notification_id: String,
        team_id: Option<String>,
        channel_id: Option<String>,
    ) -> Self {
        Self {
            notification_id,
            team_id: usable_context_value(team_id),
            channel_id: usable_context_value(channel_id),
        }
    }

    pub(crate) fn target_url(&self) -> Option<String> {
        self.team_id.as_ref().map(|team_id| match &self.channel_id {
            Some(channel_id) => {
                format!("https://app.slack.com/client/{team_id}/{channel_id}")
            }
            None => format!("https://app.slack.com/client/{team_id}"),
        })
    }

    pub(crate) fn notification_id(&self) -> &str {
        &self.notification_id
    }

    pub(crate) fn update_context(&mut self, team_id: Option<String>, channel_id: Option<String>) {
        self.team_id = usable_context_value(team_id);
        self.channel_id = usable_context_value(channel_id);
    }
}

fn usable_context_value(value: Option<String>) -> Option<String> {
    value.and_then(|value| {
        let value = value.trim();
        (!value.is_empty() && value != "unknown").then(|| value.to_string())
    })
}

pub(crate) fn activation_script(
    activation: &NotificationActivation,
) -> Result<String, serde_json::Error> {
    let payload = serde_json::to_string(activation)?;
    let fallback_url = serde_json::to_string(&activation.target_url())?;
    Ok(format!(
        r#"(function(payload, fallbackUrl) {{
            try {{
                if (typeof window.__ZlackActivateNotification === "function") {{
                    window.__ZlackActivateNotification(payload);
                    return;
                }}
            }} catch (error) {{
                console.error("Zlack: Notification activation bridge failed", error);
            }}
            if (fallbackUrl && window.location.href !== fallbackUrl) {{
                window.location.href = fallbackUrl;
            }}
        }})({payload}, {fallback_url});"#
    ))
}

pub(crate) fn activate_native_notification(
    app: &tauri::AppHandle,
    origin_label: &str,
    activation: NotificationActivation,
) {
    if let Some(window) = crate::focus_workspace_label(app, origin_label) {
        match activation_script(&activation) {
            Ok(script) => match window.eval(&script) {
                Ok(()) => return,
                Err(error) => {
                    eprintln!(
                        "Zlack: Failed to deliver notification activation to {origin_label}: {error}"
                    );
                }
            },
            Err(error) => {
                eprintln!("Zlack: Failed to serialize notification activation: {error}");
            }
        }
    }

    if let Some(team_id) = activation.team_id.as_deref() {
        crate::switch_to_workspace(app, team_id, activation.target_url());
    } else if let Some(window) = crate::active_window(app) {
        crate::restore_window(&window);
    }
}

#[cfg(test)]
mod tests {
    use super::{activation_script, NotificationActivation};

    #[test]
    fn builds_channel_fallback_only_with_valid_context() {
        let channel = NotificationActivation::new(
            "notification-1".to_string(),
            Some("T123".to_string()),
            Some("C456".to_string()),
        );
        let workspace = NotificationActivation::new(
            "notification-2".to_string(),
            Some("T123".to_string()),
            None,
        );
        let unknown = NotificationActivation::new("notification-3".to_string(), None, None);

        assert_eq!(
            channel.target_url().as_deref(),
            Some("https://app.slack.com/client/T123/C456")
        );
        assert_eq!(
            workspace.target_url().as_deref(),
            Some("https://app.slack.com/client/T123")
        );
        assert_eq!(unknown.target_url(), None);
    }

    #[test]
    fn serializes_activation_for_the_originating_preload() {
        let activation = NotificationActivation::new(
            "notification-\"quoted".to_string(),
            Some("T123".to_string()),
            Some("D456".to_string()),
        );

        let script = activation_script(&activation).expect("activation serializes");

        assert!(script.contains("window.__ZlackActivateNotification"));
        assert!(script.contains(r#""notificationId":"notification-\"quoted""#));
        assert!(script.contains(r#""teamId":"T123""#));
        assert!(script.contains(r#""channelId":"D456""#));
        assert!(script.contains("window.location.href"));
        assert!(script.contains("https://app.slack.com/client/T123/D456"));
    }

    #[test]
    fn updates_a_pending_activation_with_late_context() {
        let mut activation = NotificationActivation::new("notification-1".to_string(), None, None);

        activation.update_context(Some("T123".to_string()), Some("D456".to_string()));

        assert_eq!(
            activation.target_url().as_deref(),
            Some("https://app.slack.com/client/T123/D456")
        );
    }
}
