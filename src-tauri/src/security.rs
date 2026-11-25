use serde::Serialize;
use url::Url;

#[derive(Debug, Serialize)]
#[serde(tag = "kind", content = "message")]
pub(crate) enum OpenExternalUrlError {
    InvalidUrl,
    OpenFailed(String),
}

pub(crate) fn parse_slack_url(value: &str) -> Option<Url> {
    let url = Url::parse(value).ok()?;
    let host = url.host_str()?;
    let allowed_host = host == "slack.com" || host.ends_with(".slack.com");
    let credentials_absent = url.username().is_empty() && url.password().is_none();

    (url.scheme() == "https" && allowed_host && credentials_absent).then_some(url)
}

pub(crate) fn parse_external_url(value: &str) -> Option<Url> {
    let url = Url::parse(value).ok()?;
    let allowed_scheme = matches!(url.scheme(), "http" | "https");
    let credentials_absent = url.username().is_empty() && url.password().is_none();

    (allowed_scheme && credentials_absent).then_some(url)
}

#[tauri::command]
pub(crate) fn open_external_url(url: String) -> Result<(), OpenExternalUrlError> {
    let url = parse_external_url(&url).ok_or(OpenExternalUrlError::InvalidUrl)?;
    open::that(url.as_str()).map_err(|error| OpenExternalUrlError::OpenFailed(error.to_string()))
}

#[cfg(test)]
mod tests {
    use super::{parse_external_url, parse_slack_url};

    #[test]
    fn accepts_official_slack_https_urls() {
        let app = parse_slack_url("https://app.slack.com/client/T123");
        let workspace = parse_slack_url("https://example.slack.com/client");

        assert_eq!(
            app.and_then(|url| url.host_str().map(str::to_owned)),
            Some("app.slack.com".to_owned())
        );
        assert_eq!(
            workspace.and_then(|url| url.host_str().map(str::to_owned)),
            Some("example.slack.com".to_owned())
        );
    }

    #[test]
    fn rejects_lookalike_and_non_https_slack_urls() {
        assert!(parse_slack_url("https://evilslack.com/client").is_none());
        assert!(parse_slack_url("http://app.slack.com/client/T123").is_none());
        assert!(parse_slack_url("https://slack.com.evil.example/client").is_none());
    }

    #[test]
    fn rejects_urls_with_credentials() {
        assert!(parse_slack_url("https://user:pass@app.slack.com/client").is_none());
        assert!(parse_external_url("https://user:pass@example.com").is_none());
    }

    #[test]
    fn switch_workspace_url_validation_regression() {
        // Test cases for switch_workspace URL validation
        // Valid Slack URLs
        assert!(parse_slack_url("https://app.slack.com/client/T123/C456").is_some());
        assert!(parse_slack_url("https://team.slack.com/client").is_some());

        // Invalid URLs that should be rejected
        assert!(parse_slack_url("http://app.slack.com/client").is_none()); // HTTP not HTTPS
        assert!(parse_slack_url("https://evil.com/client").is_none()); // Not slack.com
        assert!(parse_slack_url("https://slack.com.evil.com/client").is_none()); // Subdomain trick
        assert!(parse_slack_url("file:///etc/passwd").is_none()); // File protocol
        assert!(parse_slack_url("javascript:alert(1)").is_none()); // JavaScript
    }
}
