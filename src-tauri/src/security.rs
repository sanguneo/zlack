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

/// Destination for `switch_workspace`: a Slack URL whose `/client/<team>` path
/// belongs to the workspace being switched to. Anything else keeps the current
/// screen, whether the target window already exists or not.
pub(crate) fn workspace_switch_url(team: &str, value: &str) -> Option<Url> {
    let url = parse_slack_url(value)?;
    let mut segments = url.path_segments()?;
    (segments.next() == Some("client") && segments.next() == Some(team)).then_some(url)
}

pub(crate) fn parse_external_url(value: &str) -> Option<Url> {
    let url = Url::parse(value).ok()?;
    let allowed_scheme = matches!(url.scheme(), "http" | "https");
    let credentials_absent = url.username().is_empty() && url.password().is_none();

    (allowed_scheme && credentials_absent).then_some(url)
}

pub(crate) fn parse_auth_redirect(value: &str) -> Option<Url> {
    let url = parse_slack_url(value)?;
    let path = url.path().to_ascii_lowercase();
    (path.starts_with("/signin/")
        || path.starts_with("/workspace-signin/")
        || path.starts_with("/sso/"))
    .then_some(url)
}

#[tauri::command]
pub(crate) fn open_external_url(url: String) -> Result<(), OpenExternalUrlError> {
    let url = parse_external_url(&url).ok_or(OpenExternalUrlError::InvalidUrl)?;
    open::that(url.as_str()).map_err(|error| OpenExternalUrlError::OpenFailed(error.to_string()))
}

#[cfg(test)]
mod tests {
    use super::{parse_auth_redirect, parse_external_url, parse_slack_url, workspace_switch_url};

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
    fn external_urls_allow_only_http_and_https() {
        assert!(parse_external_url("https://example.com/docs").is_some());
        assert!(parse_external_url("http://example.com/docs").is_some());
        assert!(parse_external_url("file:///C:/Windows/System32/calc.exe").is_none());
        assert!(parse_external_url("custom-protocol:payload").is_none());
        assert!(parse_external_url("https://user@example.com/docs").is_none());
    }

    #[test]
    fn rejects_urls_with_credentials() {
        assert!(parse_slack_url("https://user:pass@app.slack.com/client").is_none());
        assert!(parse_external_url("https://user:pass@example.com").is_none());
    }

    #[test]
    fn workspace_switch_accepts_only_the_teams_slack_client() {
        for url in [
            "https://app.slack.com/client/T123",
            "https://app.slack.com/client/T123/C456",
            "https://acme.slack.com/client/T123/C456?thread=1",
        ] {
            assert!(workspace_switch_url("T123", url).is_some(), "{url}");
        }

        for url in [
            "https://app.slack.com/client/T999/C456",
            "https://app.slack.com/client/T1234",
            "https://app.slack.com/T123",
            "https://app.slack.com/",
            "http://app.slack.com/client/T123",
            "https://evil.example/client/T123",
            "https://slack.com.evil.example/client/T123",
            "https://user:pass@app.slack.com/client/T123",
            "javascript:location='https://evil.example'//app.slack.com/client/T123",
            "file:///client/T123",
            "not a url",
        ] {
            assert!(workspace_switch_url("T123", url).is_none(), "{url}");
        }
    }

    #[test]
    fn auth_redirects_require_credential_free_https() {
        assert!(parse_auth_redirect("https://acme.slack.com/sso/saml/start").is_some());
        assert!(
            parse_auth_redirect("https://slack.com/workspace-signin/oauth/google/start").is_some()
        );
        assert!(parse_auth_redirect("http://acme.slack.com/sso/saml/start").is_none());
        assert!(parse_auth_redirect("https://user@acme.slack.com/sso/saml/start").is_none());
        assert!(parse_auth_redirect("https://id.example.com/saml/start").is_none());
        assert!(parse_auth_redirect("https://acme.slack.com/client/T123").is_none());
        assert!(parse_auth_redirect("custom-protocol:payload").is_none());
    }
}
