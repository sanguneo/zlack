// Google and SSO sign-in through the default browser (#13).
//
// Google refuses to sign in inside embedded webviews ("This browser or app may
// not be secure"), whatever user agent the webview reports, and SSO identity
// providers often do the same. Like the official Slack desktop app, Zlack sends
// those sign-ins to the default browser at slack.com/ssb/signin. A sign-in
// there ends with Slack opening `slack://<TEAM_ID>/magic-login/<token>`, which
// Zlack redeems with auth.loginMagicBulk inside its own webview, so the session
// cookie lands next to the other workspaces' cookies.

use crate::security;
use serde::Deserialize;
use std::{
    collections::BTreeMap,
    sync::Mutex,
    time::{Duration, Instant},
};
use tauri::{
    webview::{PageLoadEvent, PageLoadPayload},
    AppHandle, Manager, WebviewUrl, WebviewWindow, WebviewWindowBuilder,
};
use tauri_plugin_dialog::{
    DialogExt, MessageDialogButtons, MessageDialogKind, MessageDialogResult,
};
use url::Url;

mod session;

pub(crate) use session::SignInState;
use session::{AttemptId, AttemptOrigin, CallbackDecision, Exchange};

// Slack's sign-in page for desktop apps: signing in there ends with a
// `slack://` magic link instead of the web client.
const BROWSER_SIGN_IN_URL: &str = "https://slack.com/ssb/signin";
const CLIENT_URL: &str = "https://app.slack.com/client";
const MAGIC_LOGIN_PATH: &str = "/api/auth.loginMagicBulk";
const MAGIC_LOGIN_TIMEOUT: Duration = Duration::from_secs(60);
const BROWSER_ATTEMPT_LIFETIME: Duration = Duration::from_secs(10 * 60);
const CALLBACK_DEDUP_LIFETIME: Duration = Duration::from_secs(10 * 60);
const SIGN_IN_HERE_LIFETIME: Duration = Duration::from_secs(2 * 60);
const BROWSER_SIGN_IN_TOAST_JS: &str = "typeof zlackPageToast === 'function' && zlackPageToast('Sign-in continues in your browser. Zlack signs you in when you finish there.');";
// The API answers with JSON, which the webview shows as text in a <pre>.
const READ_RESPONSE_JS: &str = "(function () { var pre = document.querySelector('body > pre') || document.querySelector('pre'); return (pre ? pre.textContent : (document.body ? document.body.innerText : '')) || ''; })()";

pub(crate) fn is_auth_window(label: &str) -> bool {
    AttemptId::from_auth_window_label(label).is_some()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BrowserSignIn {
    Google,
    Sso,
}

// Whether a navigation starts a sign-in that has to happen in a real browser:
// "Sign in with Google" on slack.com or on a workspace's own sign-in page,
// SAML single sign-on, or Google's OAuth page when Slack sent the user there
// to sign in. Apple, Microsoft and email sign-in keep working in the webview.
pub(crate) fn browser_sign_in(url: &Url) -> Option<BrowserSignIn> {
    if security::parse_slack_url(url.as_str()).is_some() {
        let path = url.path().to_ascii_lowercase();
        if path.starts_with("/signin/oauth/google/")
            || path.starts_with("/workspace-signin/oauth/google/")
        {
            return Some(BrowserSignIn::Google);
        }
        if path.trim_end_matches('/') == "/sso/saml/start" {
            return Some(BrowserSignIn::Sso);
        }
        return None;
    }
    is_google_oauth_for_slack_sign_in(url).then_some(BrowserSignIn::Google)
}

// Google's OAuth page, when its redirect_uri is a Slack sign-in or SSO
// callback (https://oauth2.slack.com/signin/oauth/google/end). Google Drive and
// other integrations come back elsewhere and are left alone.
fn is_google_oauth_for_slack_sign_in(url: &Url) -> bool {
    if url.scheme() != "https"
        || url.host_str() != Some("accounts.google.com")
        || !url.path().starts_with("/o/oauth2/")
    {
        return false;
    }
    url.query_pairs()
        .filter(|(key, _)| key == "redirect_uri")
        .filter_map(|(_, value)| security::parse_slack_url(&value))
        .any(|redirect| {
            let path = redirect.path();
            path.starts_with("/signin/")
                || path.starts_with("/workspace-signin/")
                || path.starts_with("/sso/")
        })
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum MagicLink {
    Login(MagicLogin),
    UnsupportedHost(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct MagicLogin {
    api_base: Url,
    tokens: Vec<String>,
}

impl MagicLogin {
    pub(crate) fn request_url(&self) -> Url {
        let mut url = self.api_base.clone();
        url.set_path(MAGIC_LOGIN_PATH);
        // Tokens only hold [A-Za-z0-9_-], so they need no escaping.
        url.set_query(Some(&format!(
            "magic_tokens={}&ssb=1",
            self.tokens.join(",")
        )));
        url
    }

    // Logs mask each token's secret, as the official app's logs do.
    fn redacted_request_url(&self) -> String {
        self.tokens
            .iter()
            .fold(self.request_url().to_string(), |url, token| {
                url.replace(token, &redact_token(token))
            })
    }

    fn workspace_description(&self) -> String {
        let teams = self
            .tokens
            .iter()
            .filter_map(|token| {
                token
                    .strip_prefix("z-app-")
                    .and_then(|rest| rest.split_once('-'))
                    .map(|(team, _)| team)
            })
            .collect::<Vec<_>>()
            .join(", ");
        let host = self.api_base.host_str().unwrap_or("Slack");
        if teams.is_empty() {
            host.to_owned()
        } else {
            format!("{teams} on {host}")
        }
    }
}

fn redact_token(token: &str) -> String {
    match token
        .strip_prefix("z-app-")
        .and_then(|rest| rest.split_once('-'))
    {
        Some((team, _)) => format!("z-app-{team}-<redacted>"),
        None => "<redacted>".to_owned(),
    }
}

struct TokenSet {
    host: Option<String>,
    tokens: Vec<String>,
}

// Parses the sign-in links Slack hands to desktop apps, the way the official
// app reads them:
// - `slack://<TEAM_ID>/magic-login/<token>[?host=<domain>]`, which a browser
//   sign-in ends with (more `<TEAM_ID>/magic-login/<token>` pairs may follow),
// - `slack://login-v2?0.host=<domain>&0.tokens=<token>_<token>[&1.host=...]`.
// `host` names the Slack domain to sign in on: none or slack.com means
// app.slack.com. Returns None for other `slack://` links (channels, profiles).
pub(crate) fn parse_magic_link(url: &Url) -> Option<MagicLink> {
    if url.scheme() != "slack" {
        return None;
    }
    let sets = if url.host_str() == Some("login-v2") {
        login_v2_sets(url)
    } else {
        magic_login_set(url).into_iter().collect()
    };

    let mut api_base: Option<Url> = None;
    let mut tokens = Vec::new();
    let mut unsupported = None;
    for set in sets.into_iter().filter(|set| !set.tokens.is_empty()) {
        match magic_login_api_base(set.host.as_deref()) {
            Some(base) if api_base.as_ref().is_none_or(|first| *first == base) => {
                api_base = Some(base);
                tokens.extend(set.tokens);
            }
            // A second Slack domain in one link; the first one wins.
            Some(_) => {}
            None => {
                unsupported.get_or_insert(set.host.unwrap_or_default());
            }
        }
    }
    match api_base {
        Some(api_base) => Some(MagicLink::Login(MagicLogin { api_base, tokens })),
        None => unsupported.map(MagicLink::UnsupportedHost),
    }
}

fn magic_login_set(url: &Url) -> Option<TokenSet> {
    if !url.path().to_ascii_lowercase().contains("/magic-login/") {
        return None;
    }
    // slack://T123/magic-login/abc puts the team in the host and the rest in
    // the path. Some Linux URL openers lowercase the host, so the team ID is
    // uppercased again, as the official app does.
    let segments: Vec<&str> = url
        .host_str()
        .into_iter()
        .chain(url.path().split('/'))
        .filter(|segment| !segment.is_empty())
        .collect();
    let tokens = segments
        .windows(3)
        .filter_map(|pair| {
            let team = pair[0].to_ascii_uppercase();
            (pair[1].eq_ignore_ascii_case("magic-login") && is_team_id(&team) && is_token(pair[2]))
                .then(|| format!("z-app-{team}-{}", pair[2]))
        })
        .collect();
    let host = url
        .query_pairs()
        .find(|(key, _)| key == "host")
        .map(|(_, value)| value.into_owned());
    Some(TokenSet { host, tokens })
}

fn login_v2_sets(url: &Url) -> Vec<TokenSet> {
    let params: Vec<(String, String)> = url.query_pairs().into_owned().collect();
    let param = |name: &str| {
        params
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.as_str())
    };
    (0..)
        .map_while(|index| {
            let host = param(&format!("{index}.host"))?;
            let tokens = param(&format!("{index}.tokens"))?;
            Some(TokenSet {
                host: Some(host.to_owned()),
                tokens: tokens
                    .split('_')
                    .filter(|token| is_token(token))
                    .map(str::to_owned)
                    .collect(),
            })
        })
        .collect()
}

// Where to call auth.loginMagicBulk. The official app uses app.<domain> for the
// domain a link names, so slack.com (or no domain) means app.slack.com. A
// workspace host (acme.slack.com) serves the same Web API and is used as is.
// Anything outside slack.com is a Slack environment Zlack does not run on.
fn magic_login_api_base(host: Option<&str>) -> Option<Url> {
    let host = host
        .map(|host| host.trim().to_ascii_lowercase())
        .filter(|host| !host.is_empty() && host != "slack.com")
        .unwrap_or_else(|| "app.slack.com".to_owned());
    security::parse_slack_url(&format!("https://{host}/"))
        .filter(|url| url.host_str() == Some(host.as_str()) && url.port().is_none())
}

// Slack team and enterprise IDs: T or E and 8-50 more characters.
fn is_team_id(value: &str) -> bool {
    let bytes = value.as_bytes();
    matches!(bytes.first(), Some(b'T' | b'E'))
        && (9..=51).contains(&bytes.len())
        && bytes[1..]
            .iter()
            .all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit())
}

fn is_token(value: &str) -> bool {
    !value.is_empty()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum MagicLoginOutcome {
    SignedIn,
    ContinueInBrowser(Url),
    Failed(String),
}

#[derive(Deserialize)]
struct MagicLoginResponse {
    ok: bool,
    error: Option<String>,
    #[serde(default)]
    token_results: BTreeMap<String, TokenResult>,
}

#[derive(Deserialize)]
struct TokenResult {
    #[serde(default)]
    ok: bool,
    error: Option<String>,
    auth_redir: Option<String>,
}

// Reads the auth.loginMagicBulk response. Slack answers `ok: true` even when
// every token was rejected ({"ok":true,"token_results":{"z-app-…":{"ok":false,
// "error":"invalid_token"}}}), so the per-token results decide.
pub(crate) fn magic_login_outcome(body: &str) -> MagicLoginOutcome {
    let Ok(response) = serde_json::from_str::<MagicLoginResponse>(body.trim()) else {
        return MagicLoginOutcome::Failed("unexpected_response".to_owned());
    };
    if !response.ok {
        return MagicLoginOutcome::Failed(
            response.error.unwrap_or_else(|| "unknown_error".to_owned()),
        );
    }
    let results = &response.token_results;
    if let Some(redirect) = results
        .values()
        .find_map(|result| result.auth_redir.as_deref())
    {
        return match security::parse_auth_redirect(redirect) {
            Some(redirect) => MagicLoginOutcome::ContinueInBrowser(redirect),
            None => MagicLoginOutcome::Failed("invalid_auth_redirect".to_owned()),
        };
    }
    if results.is_empty() || results.values().any(|result| result.ok) {
        return MagicLoginOutcome::SignedIn;
    }
    MagicLoginOutcome::Failed(
        results
            .values()
            .find_map(|result| result.error.clone())
            .unwrap_or_else(|| "unknown_error".to_owned()),
    )
}

pub(crate) fn allow_navigation(app: &AppHandle, label: &str, url: &Url) -> bool {
    let Some(kind) = browser_sign_in(url) else {
        return true;
    };
    {
        let state = app.state::<Mutex<SignInState>>();
        let mut state = state.lock().unwrap();
        if state.allow_sign_in_here(label, url, Instant::now()) {
            return true;
        }
    }
    eprintln!(
        "Zlack: {kind:?} sign-in at {}{} continues in the browser",
        url.host_str().unwrap_or_default(),
        url.path()
    );
    let app = app.clone();
    let label = label.to_owned();
    let url = url.clone();
    std::thread::spawn(move || match kind {
        BrowserSignIn::Google => open_browser_sign_in(&app, &label),
        BrowserSignIn::Sso => ask_where_to_sign_in(app, label, url),
    });
    false
}

fn attempt_origin(app: &AppHandle, label: &str) -> AttemptOrigin {
    let return_url = app
        .get_webview_window(label)
        .and_then(|window| window.url().ok())
        .and_then(|url| security::parse_slack_url(url.as_str()))
        .or_else(|| Url::parse(CLIENT_URL).ok())
        .unwrap_or_else(|| CLIENT_URL.parse().unwrap());
    AttemptOrigin {
        window_label: label.to_owned(),
        return_url,
    }
}

fn open_browser_sign_in(app: &AppHandle, label: &str) {
    let attempt_id = app
        .state::<Mutex<SignInState>>()
        .lock()
        .unwrap()
        .record_browser_attempt(
            attempt_origin(app, label),
            Instant::now(),
            BROWSER_ATTEMPT_LIFETIME,
        );
    match open::that(BROWSER_SIGN_IN_URL) {
        Ok(()) => {
            if let Some(window) = app.get_webview_window(label) {
                let _ = window.eval(BROWSER_SIGN_IN_TOAST_JS);
            }
        }
        Err(error) => {
            app.state::<Mutex<SignInState>>()
                .lock()
                .unwrap()
                .remove_browser_attempt(attempt_id);
            eprintln!("Zlack: Failed to open the browser for sign-in: {error}");
            show_error(
                app,
                &format!(
                    "Zlack could not open your browser ({error}). Open {BROWSER_SIGN_IN_URL} in a browser to sign in."
                ),
            );
        }
    }
}

// SSO used to work inside Zlack for many identity providers, and the browser
// route depends on the slack:// link coming back to Zlack (another Slack app
// may claim it). So SSO offers both, browser first.
fn ask_where_to_sign_in(app: AppHandle, label: String, url: Url) {
    let dialog_app = app.clone();
    dialog_app
        .dialog()
        .message(
            "Single sign-on continues in your default browser, as in the Slack desktop app. Zlack signs you in when you finish there.\n\nIf your browser hands the sign-in to another Slack app instead, choose Sign In Here.",
        )
        .title("Sign in with SSO")
        .buttons(MessageDialogButtons::YesNoCancelCustom(
            "Open Browser".into(),
            "Sign In Here".into(),
            "Cancel".into(),
        ))
        .show_with_result(move |result| match result {
            MessageDialogResult::Custom(choice) if choice == "Open Browser" => {
                open_browser_sign_in(&app, &label);
            }
            MessageDialogResult::Custom(choice) if choice == "Sign In Here" => {
                if let Some(window) = app.get_webview_window(&label) {
                    app.state::<Mutex<SignInState>>()
                        .lock()
                        .unwrap()
                        .allow_next_sign_in_here(
                            label.clone(),
                            url.clone(),
                            Instant::now(),
                            SIGN_IN_HERE_LIFETIME,
                        );
                    if let Err(error) = window.navigate(url) {
                        app.state::<Mutex<SignInState>>()
                            .lock()
                            .unwrap()
                            .clear_sign_in_here(&label);
                        eprintln!("Zlack: SSO navigation failed: {error}");
                    }
                }
            }
            MessageDialogResult::Cancel
            | MessageDialogResult::Ok
            | MessageDialogResult::Yes
            | MessageDialogResult::No
            | MessageDialogResult::Custom(_) => {}
        });
}

// `slack://` links from the OS: on macOS through the open-URL event, on Windows
// and Linux as the argument of a second instance, which the single-instance
// plugin forwards.
pub(crate) fn handle_deep_links(app: &AppHandle, urls: Vec<Url>) {
    for url in urls {
        match parse_magic_link(&url) {
            Some(MagicLink::Login(login)) => submit_magic_login(app, login),
            Some(MagicLink::UnsupportedHost(host)) => {
                eprintln!("Zlack: Magic login: unsupported Slack domain {host:?}");
                show_error(
                    app,
                    &format!("This sign-in link is for {host}, which Zlack does not support."),
                );
            }
            None => {
                eprintln!(
                    "Zlack: Ignoring {}://{} link",
                    url.scheme(),
                    url.host_str().unwrap_or_default()
                );
                if let Some(window) = crate::active_window(app) {
                    crate::restore_window(&window);
                }
            }
        }
    }
}

fn submit_magic_login(app: &AppHandle, login: MagicLogin) {
    let Some(window) = crate::active_window(app) else {
        eprintln!("Zlack: Magic login: no window to sign in with");
        return;
    };
    let fallback_origin = AttemptOrigin {
        window_label: window.label().to_owned(),
        return_url: window
            .url()
            .ok()
            .and_then(|url| security::parse_slack_url(url.as_str()))
            .or_else(|| Url::parse(CLIENT_URL).ok())
            .unwrap_or_else(|| CLIENT_URL.parse().unwrap()),
    };
    let decision = app
        .state::<Mutex<SignInState>>()
        .lock()
        .unwrap()
        .submit_callback(
            login,
            fallback_origin,
            Instant::now(),
            CALLBACK_DEDUP_LIFETIME,
        );
    match decision {
        CallbackDecision::Duplicate => {
            eprintln!("Zlack: Magic login: ignored duplicate callback");
        }
        CallbackDecision::Queued => {
            eprintln!("Zlack: Magic login: queued behind the active exchange");
        }
        CallbackDecision::Start(exchange) => start_magic_login(app, exchange),
        CallbackDecision::NeedsConfirmation(exchange) => confirm_unstarted_login(app, exchange),
    }
}

fn confirm_unstarted_login(app: &AppHandle, exchange: Exchange) {
    let workspace = exchange.login.workspace_description();
    let dialog_app = app.clone();
    app.dialog()
        .message(format!(
            "Zlack did not start this sign-in, or it restarted before the sign-in finished. Sign in to {workspace}?"
        ))
        .title("Confirm Slack sign-in")
        .buttons(MessageDialogButtons::OkCancelCustom(
            "Sign In".into(),
            "Cancel".into(),
        ))
        .show(move |approved| {
            if !approved {
                dialog_app
                    .state::<Mutex<SignInState>>()
                    .lock()
                    .unwrap()
                    .reject_callback(&exchange);
                return;
            }
            let decision = dialog_app
                .state::<Mutex<SignInState>>()
                .lock()
                .unwrap()
                .confirm_callback(exchange);
            match decision {
                CallbackDecision::Start(exchange) => start_magic_login(&dialog_app, exchange),
                CallbackDecision::Queued => {
                    eprintln!("Zlack: Magic login: queued confirmed callback");
                }
                CallbackDecision::Duplicate => {}
                CallbackDecision::NeedsConfirmation(exchange) => {
                    dialog_app
                        .state::<Mutex<SignInState>>()
                        .lock()
                        .unwrap()
                        .reject_callback(&exchange);
                }
            }
        });
}

fn start_magic_login(app: &AppHandle, exchange: Exchange) {
    let id = exchange.id;
    let label = id.auth_window_label();
    let request_url = exchange.login.request_url();
    eprintln!(
        "Zlack: Magic login: loading {} in isolated window {}",
        exchange.login.redacted_request_url(),
        label
    );
    let built = WebviewWindowBuilder::new(app, &label, WebviewUrl::External(request_url))
        .additional_browser_args(crate::BROWSER_ARGS)
        .user_agent(crate::user_agent())
        .visible(false)
        .skip_taskbar(true)
        .on_page_load(on_page_load)
        .build();
    if let Err(error) = built {
        finish_magic_login(app, id, None);
        eprintln!("Zlack: Magic login: auth window failed: {error}");
        show_error(
            app,
            "Zlack could not finish signing in to Slack. Sign in again.",
        );
        return;
    }

    let app = app.clone();
    std::thread::spawn(move || {
        std::thread::sleep(MAGIC_LOGIN_TIMEOUT);
        if finish_magic_login(&app, id, None) {
            eprintln!("Zlack: Magic login: no response");
            show_error(
                &app,
                "Signing in to Slack took too long. Check your connection and sign in again.",
            );
        }
    });
}

fn is_magic_login_response(url: &Url) -> bool {
    url.path() == MAGIC_LOGIN_PATH && security::parse_slack_url(url.as_str()).is_some()
}

pub(crate) fn on_page_load(window: WebviewWindow, payload: PageLoadPayload<'_>) {
    if payload.event() != PageLoadEvent::Finished || !is_magic_login_response(payload.url()) {
        return;
    }
    let Some(id) = AttemptId::from_auth_window_label(window.label()) else {
        return;
    };
    let app = window.app_handle().clone();
    if !app
        .state::<Mutex<SignInState>>()
        .lock()
        .unwrap()
        .response_matches(id, payload.url())
    {
        return;
    }
    let callback_app = app.clone();
    let read = window.eval_with_callback(READ_RESPONSE_JS, move |result| {
        finish_magic_login(&callback_app, id, Some(&result));
    });
    if let Err(error) = read {
        eprintln!("Zlack: Magic login: could not read the response: {error}");
        finish_magic_login(&app, id, Some(""));
    }
}

fn finish_magic_login(app: &AppHandle, id: AttemptId, result: Option<&str>) -> bool {
    let completed = app
        .state::<Mutex<SignInState>>()
        .lock()
        .unwrap()
        .complete(id);
    let Some((completed, next)) = completed else {
        return false;
    };
    if let Some(result) = result {
        // The script's return value arrives JSON-encoded.
        let body = serde_json::from_str::<String>(result).unwrap_or_else(|_| result.to_owned());
        match magic_login_outcome(&body) {
            MagicLoginOutcome::SignedIn => eprintln!("Zlack: Magic login: signed in"),
            MagicLoginOutcome::ContinueInBrowser(redirect) => {
                eprintln!("Zlack: Magic login: Slack asked to continue in the browser");
                if let Err(error) = open::that(redirect.as_str()) {
                    eprintln!("Zlack: Failed to open the browser for sign-in: {error}");
                }
            }
            MagicLoginOutcome::Failed(error) => {
                eprintln!("Zlack: Magic login: failed: {error}");
                show_error(
                    app,
                    &format!(
                        "Zlack could not finish signing in to Slack ({error}). Sign in again."
                    ),
                );
            }
        }
    }
    if let Some(auth_window) = app.get_webview_window(&id.auth_window_label()) {
        let _ = auth_window.close();
    }
    if let Some(window) = app.get_webview_window(&completed.origin.window_label) {
        let return_url = if completed.origin.return_url.path().starts_with("/client") {
            completed.origin.return_url
        } else {
            Url::parse(CLIENT_URL).unwrap()
        };
        if window.url().ok().as_ref() != Some(&return_url) {
            let _ = window.navigate(return_url);
        }
        crate::restore_window(&window);
    }
    if let Some(next) = next {
        start_magic_login(app, next);
    }
    true
}

pub(crate) fn on_window_destroyed(app: &AppHandle, label: &str) {
    app.state::<Mutex<SignInState>>()
        .lock()
        .unwrap()
        .clear_sign_in_here(label);
    if let Some(id) = AttemptId::from_auth_window_label(label) {
        finish_magic_login(app, id, None);
    }
}

fn show_error(app: &AppHandle, message: &str) {
    app.dialog()
        .message(message)
        .title("Slack sign-in")
        .kind(MessageDialogKind::Error)
        .show(|_| {});
}

#[cfg(test)]
mod tests {
    use super::{
        browser_sign_in, magic_login_outcome, parse_magic_link, BrowserSignIn, MagicLink,
        MagicLoginOutcome,
    };
    use url::Url;

    fn url(value: &str) -> Url {
        Url::parse(value).unwrap()
    }

    fn sign_in(value: &str) -> Option<BrowserSignIn> {
        browser_sign_in(&url(value))
    }

    fn request_url(link: &str) -> Option<String> {
        match parse_magic_link(&url(link)) {
            Some(MagicLink::Login(login)) => Some(login.request_url().to_string()),
            _ => None,
        }
    }

    #[test]
    fn slack_google_sign_in_goes_to_the_browser() {
        assert_eq!(
            sign_in("https://slack.com/signin/oauth/google/start?email_first=1&entry_point=redirect_flow"),
            Some(BrowserSignIn::Google)
        );
        assert_eq!(
            sign_in("https://acme.slack.com/workspace-signin/oauth/google/start?redir=&is_ssb_browser_signin="),
            Some(BrowserSignIn::Google)
        );
        assert_eq!(
            sign_in("https://accounts.google.com/o/oauth2/v2/auth?client_id=606092904014-s1u3idjanlbhr4ns5b1hcjgfn63cr9nh.apps.googleusercontent.com&redirect_uri=https%3A%2F%2Foauth2.slack.com%2Fsignin%2Foauth%2Fgoogle%2Fend&scope=openid+email+profile&response_type=code"),
            Some(BrowserSignIn::Google)
        );
    }

    #[test]
    fn saml_sign_in_goes_to_the_browser() {
        assert_eq!(
            sign_in("https://acme.slack.com/sso/saml/start?redir=%2F"),
            Some(BrowserSignIn::Sso)
        );
        assert_eq!(
            sign_in("https://acme.enterprise.slack.com/sso/saml/start"),
            Some(BrowserSignIn::Sso)
        );
    }

    #[test]
    fn other_sign_in_and_app_pages_stay_in_the_webview() {
        for page in [
            "https://slack.com/signin",
            "https://slack.com/ssb/signin",
            "https://acme.slack.com/",
            "https://acme.slack.com/workspace-signin/oauth/apple/start?redir=",
            "https://acme.slack.com/workspace-signin/oauth/microsoft/start?redir=",
            "https://slack.com/signin/oauth/apple/start?email_first=1",
            // The SAML response coming back from the identity provider.
            "https://acme.slack.com/sso/saml",
            "https://app.slack.com/client/T0123456789",
            "https://accounts.google.com/gsi/iframe/select?client_id=1",
            "https://accounts.google.com/o/oauth2/v2/auth?redirect_uri=https%3A%2F%2Fdocs.google.com%2Fpicker",
            "https://accounts.google.com/o/oauth2/v2/auth?redirect_uri=https%3A%2F%2Fslack.com%2Fservices%2Fauth",
        ] {
            assert_eq!(sign_in(page), None, "{page}");
        }
    }

    #[test]
    fn look_alike_google_sign_in_urls_are_not_intercepted() {
        for page in [
            "http://slack.com/signin/oauth/google/start",
            "https://slack.com.evil.example/signin/oauth/google/start",
            "https://evilslack.com/sso/saml/start",
            "https://accounts.google.com.evil.example/o/oauth2/v2/auth?redirect_uri=https%3A%2F%2Foauth2.slack.com%2Fsignin%2Foauth%2Fgoogle%2Fend",
            "https://accounts.google.com/o/oauth2/v2/auth?redirect_uri=https%3A%2F%2Fslack.com.evil.example%2Fsignin%2Foauth%2Fgoogle%2Fend",
        ] {
            assert_eq!(sign_in(page), None, "{page}");
        }
    }

    #[test]
    fn magic_login_link_becomes_a_login_magic_bulk_request() {
        let expected = "https://app.slack.com/api/auth.loginMagicBulk?magic_tokens=z-app-T00000000-dummytoken&ssb=1";
        assert_eq!(
            request_url("slack://T00000000/magic-login/dummytoken").as_deref(),
            Some(expected)
        );
        assert_eq!(
            request_url("slack://T00000000/magic-login/dummytoken?host=slack.com").as_deref(),
            Some(expected)
        );
        assert_eq!(
            request_url("slack://T00000000/magic-login/dummytoken/?host=").as_deref(),
            Some(expected)
        );
        // Real links carry tokens like this one.
        assert_eq!(
            request_url("slack://T02PXBHER/magic-login/4044442274161-4c258fb079cdd838e7318365d6305619d1e7392a5840699dbe1d2e84918220c1").as_deref(),
            Some("https://app.slack.com/api/auth.loginMagicBulk?magic_tokens=z-app-T02PXBHER-4044442274161-4c258fb079cdd838e7318365d6305619d1e7392a5840699dbe1d2e84918220c1&ssb=1")
        );
    }

    #[test]
    fn magic_login_link_restores_a_lowercased_team_id() {
        assert_eq!(
            request_url("slack://t00000000/magic-login/dummytoken").as_deref(),
            Some("https://app.slack.com/api/auth.loginMagicBulk?magic_tokens=z-app-T00000000-dummytoken&ssb=1")
        );
    }

    #[test]
    fn magic_login_link_with_a_workspace_host_calls_that_host() {
        assert_eq!(
            request_url("slack://T00000000/magic-login/dummytoken?host=example.slack.com").as_deref(),
            Some("https://example.slack.com/api/auth.loginMagicBulk?magic_tokens=z-app-T00000000-dummytoken&ssb=1")
        );
    }

    #[test]
    fn magic_login_link_redeems_every_team_token_pair() {
        assert_eq!(
            request_url("slack://T00000001/magic-login/first-token/T00000002/magic-login/second-token").as_deref(),
            Some("https://app.slack.com/api/auth.loginMagicBulk?magic_tokens=z-app-T00000001-first-token,z-app-T00000002-second-token&ssb=1")
        );
    }

    #[test]
    fn login_v2_link_redeems_its_token_sets() {
        assert_eq!(
            request_url("slack://login-v2?0.host=slack.com&0.tokens=z-app-T00000001-a_z-app-T00000002-b&1.host=slack.com&1.tokens=z-app-E00000003-c").as_deref(),
            Some("https://app.slack.com/api/auth.loginMagicBulk?magic_tokens=z-app-T00000001-a,z-app-T00000002-b,z-app-E00000003-c&ssb=1")
        );
        // Sets are read up to the first incomplete one.
        assert_eq!(
            request_url("slack://login-v2?0.host=slack.com&0.tokens=z-app-T00000001-a&1.tokens=z-app-T00000002-b").as_deref(),
            Some("https://app.slack.com/api/auth.loginMagicBulk?magic_tokens=z-app-T00000001-a&ssb=1")
        );
    }

    #[test]
    fn magic_login_link_for_another_slack_environment_is_unsupported() {
        for (link, host) in [
            (
                "slack://T00000000/magic-login/dummytoken?host=slack-gov.com",
                "slack-gov.com",
            ),
            (
                "slack://T00000000/magic-login/dummytoken?host=evil.example",
                "evil.example",
            ),
            (
                "slack://T00000000/magic-login/dummytoken?host=example.slack.com:8443",
                "example.slack.com:8443",
            ),
            (
                "slack://T00000000/magic-login/dummytoken?host=user@example.slack.com",
                "user@example.slack.com",
            ),
            (
                "slack://login-v2?0.host=slack-gov.com&0.tokens=z-app-T00000001-a",
                "slack-gov.com",
            ),
        ] {
            assert_eq!(
                parse_magic_link(&url(link)),
                Some(MagicLink::UnsupportedHost(host.to_owned())),
                "{link}"
            );
        }
    }

    #[test]
    fn other_links_are_not_magic_logins() {
        for link in [
            "slack://channel?team=T00000000&id=C00000000",
            "slack://open",
            "slack://T00000000/magic-login/",
            "slack://X00000000/magic-login/dummytoken",
            "slack://T0000/magic-login/dummytoken",
            "slack://T00000000/magic-login/bad%20token",
            "slack://login-v2?0.host=slack.com",
            "https://app.slack.com/T00000000/magic-login/dummytoken",
        ] {
            assert_eq!(parse_magic_link(&url(link)), None, "{link}");
        }
    }

    #[test]
    fn logged_request_masks_token_secrets() {
        let Some(MagicLink::Login(login)) =
            parse_magic_link(&url("slack://T00000000/magic-login/dummytoken"))
        else {
            panic!("expected a magic login");
        };
        assert_eq!(
            login.redacted_request_url(),
            "https://app.slack.com/api/auth.loginMagicBulk?magic_tokens=z-app-T00000000-<redacted>&ssb=1"
        );
    }

    #[test]
    fn rejected_tokens_fail_even_though_the_response_is_ok() {
        assert_eq!(
            magic_login_outcome(
                r#"{"ok":true,"token_results":{"z-app-T00000000-dummytoken":{"ok":false,"error":"invalid_token"}}}"#
            ),
            MagicLoginOutcome::Failed("invalid_token".to_owned())
        );
    }

    #[test]
    fn accepted_token_signs_in() {
        assert_eq!(
            magic_login_outcome(
                r#"{"ok":true,"token_results":{"z-app-T1-a":{"ok":false,"error":"invalid_token"},"z-app-T2-b":{"ok":true,"team":{"id":"T2","url":"https://acme.slack.com/"}}}}"#
            ),
            MagicLoginOutcome::SignedIn
        );
        assert_eq!(
            magic_login_outcome(r#"{"ok":true}"#),
            MagicLoginOutcome::SignedIn
        );
    }

    #[test]
    fn auth_redirect_continues_in_the_browser() {
        assert_eq!(
            magic_login_outcome(
                r#"{"ok":true,"token_results":{"z-app-T1-a":{"ok":true,"auth_redir":"https://acme.slack.com/sso/saml/start?redir=%2F"}}}"#
            ),
            MagicLoginOutcome::ContinueInBrowser(
                Url::parse("https://acme.slack.com/sso/saml/start?redir=%2F").unwrap()
            )
        );
    }

    #[test]
    fn unreviewed_auth_redirect_fails_the_login() {
        for redirect in [
            "http://acme.slack.com/sso/saml/start",
            "https://id.example.com/saml/start",
            "https://acme.slack.com/client/T123",
        ] {
            assert_eq!(
                magic_login_outcome(&format!(
                    r#"{{"ok":true,"token_results":{{"z-app-T1-a":{{"ok":true,"auth_redir":"{redirect}"}}}}}}"#
                )),
                MagicLoginOutcome::Failed("invalid_auth_redirect".to_owned()),
                "{redirect}"
            );
        }
    }

    #[test]
    fn failed_or_unreadable_responses_fail() {
        assert_eq!(
            magic_login_outcome(r#"{"ok":false,"error":"invalid_arguments"}"#),
            MagicLoginOutcome::Failed("invalid_arguments".to_owned())
        );
        assert_eq!(
            magic_login_outcome("<html>Bad Gateway</html>"),
            MagicLoginOutcome::Failed("unexpected_response".to_owned())
        );
        assert_eq!(
            magic_login_outcome(""),
            MagicLoginOutcome::Failed("unexpected_response".to_owned())
        );
    }
}
