use super::{AttemptId, AttemptOrigin, CallbackDecision, SignInState};
use crate::sign_in::{parse_magic_link, MagicLink};
use std::time::{Duration, Instant};
use url::Url;

fn url(value: &str) -> Url {
    Url::parse(value).unwrap()
}

fn login(token: &str) -> crate::sign_in::MagicLogin {
    let link = url(&format!(
        "slack://T00000000/magic-login/{token}?host=slack.com"
    ));
    match parse_magic_link(&link) {
        Some(MagicLink::Login(login)) => login,
        other => panic!("expected magic login, got {other:?}"),
    }
}

fn origin(label: &str) -> AttemptOrigin {
    AttemptOrigin {
        window_label: label.to_owned(),
        return_url: url("https://app.slack.com/client/T00000000/C00000000"),
    }
}

fn confirm(state: &mut SignInState, decision: CallbackDecision) -> CallbackDecision {
    let CallbackDecision::NeedsConfirmation(exchange) = decision else {
        panic!("callback should require confirmation");
    };
    state.confirm_callback(exchange)
}

#[test]
fn duplicate_callback_is_admitted_once() {
    let now = Instant::now();
    let mut state = SignInState::default();
    state.record_browser_attempt(origin("main"), now, Duration::from_secs(60));

    let first = state.submit_callback(
        login("same-token"),
        origin("main"),
        now,
        Duration::from_secs(60),
    );
    let duplicate = state.submit_callback(
        login("same-token"),
        origin("main"),
        now,
        Duration::from_secs(60),
    );

    assert!(matches!(
        confirm(&mut state, first),
        CallbackDecision::Start(_)
    ));
    assert!(matches!(duplicate, CallbackDecision::Duplicate));
}

#[test]
fn consecutive_attempts_are_exchanged_in_order() {
    let now = Instant::now();
    let mut state = SignInState::default();
    let first_id = state.record_browser_attempt(origin("main"), now, Duration::from_secs(60));
    let second_id =
        state.record_browser_attempt(origin("workspace-2"), now, Duration::from_secs(60));

    let first = state.submit_callback(
        login("first-token"),
        origin("main"),
        now,
        Duration::from_secs(60),
    );
    let first = confirm(&mut state, first);
    let second = state.submit_callback(
        login("second-token"),
        origin("workspace-2"),
        now,
        Duration::from_secs(60),
    );
    let second = confirm(&mut state, second);
    let Some((completed, next)) = state.complete(first_id) else {
        panic!("first exchange should be active");
    };

    assert!(matches!(first, CallbackDecision::Start(exchange) if exchange.id == first_id));
    assert!(matches!(second, CallbackDecision::Queued));
    assert_eq!(completed.id, first_id);
    assert_eq!(next.map(|exchange| exchange.id), Some(second_id));
}

#[test]
fn late_response_after_timeout_cannot_finish_next_attempt() {
    let now = Instant::now();
    let mut state = SignInState::default();
    let first_id = state.record_browser_attempt(origin("main"), now, Duration::from_secs(60));
    let second_id = state.record_browser_attempt(origin("main"), now, Duration::from_secs(60));
    let first_login = login("first-token");
    let first_url = first_login.request_url();
    let second_login = login("second-token");

    let first = state.submit_callback(first_login, origin("main"), now, Duration::from_secs(60));
    confirm(&mut state, first);
    let second = state.submit_callback(second_login, origin("main"), now, Duration::from_secs(60));
    confirm(&mut state, second);
    let Some((_, next)) = state.complete(first_id) else {
        panic!("timed out exchange should be active");
    };

    assert_eq!(next.map(|exchange| exchange.id), Some(second_id));
    assert!(!state.response_matches(first_id, &first_url));
    assert!(state.complete(first_id).is_none());
}

#[test]
fn unstarted_callback_requires_confirmation() {
    let now = Instant::now();
    let mut state = SignInState::default();

    let decision = state.submit_callback(
        login("unstarted-token"),
        origin("main"),
        now,
        Duration::from_secs(60),
    );

    assert!(matches!(decision, CallbackDecision::NeedsConfirmation(_)));
}

#[test]
fn expired_attempt_uses_the_current_window_for_confirmation() {
    let now = Instant::now();
    let mut state = SignInState::default();
    let attempt_id = state.record_browser_attempt(origin("main"), now, Duration::from_secs(1));

    let decision = state.submit_callback(
        login("expired-token"),
        origin("workspace-2"),
        now + Duration::from_secs(1),
        Duration::from_secs(60),
    );

    assert!(matches!(
        decision,
        CallbackDecision::NeedsConfirmation(exchange)
            if exchange.id != attempt_id && exchange.origin.window_label == "workspace-2"
    ));
}

#[test]
fn sign_in_here_exceptions_are_exact_expiring_single_use_and_per_window() {
    let now = Instant::now();
    let mut state = SignInState::default();
    let main_url = url("https://acme.slack.com/sso/saml/start?redir=%2Fclient%2FT1");
    let second_url = url("https://second.slack.com/sso/saml/start?redir=%2Fclient%2FT2");
    state.allow_next_sign_in_here(
        "main".to_owned(),
        main_url.clone(),
        now,
        Duration::from_secs(30),
    );
    state.allow_next_sign_in_here(
        "workspace-2".to_owned(),
        second_url.clone(),
        now,
        Duration::from_secs(30),
    );

    assert!(!state.allow_sign_in_here("workspace-2", &main_url, now));
    assert!(state.allow_sign_in_here("main", &main_url, now));
    assert!(!state.allow_sign_in_here("main", &main_url, now));
    assert!(state.allow_sign_in_here("workspace-2", &second_url, now));

    state.allow_next_sign_in_here(
        "main".to_owned(),
        main_url.clone(),
        now,
        Duration::from_secs(30),
    );
    assert!(!state.allow_sign_in_here("main", &main_url, now + Duration::from_secs(30)));
}

#[test]
fn rejected_callback_can_be_submitted_again() {
    let now = Instant::now();
    let mut state = SignInState::default();
    let decision = state.submit_callback(
        login("retry-token"),
        origin("main"),
        now,
        Duration::from_secs(60),
    );
    let CallbackDecision::NeedsConfirmation(exchange) = decision else {
        panic!("callback should require confirmation");
    };

    state.reject_callback(&exchange);
    let retry = state.submit_callback(
        login("retry-token"),
        origin("main"),
        now,
        Duration::from_secs(60),
    );

    assert!(matches!(retry, CallbackDecision::NeedsConfirmation(_)));
}

#[test]
fn attempt_id_is_read_only_from_auth_window_labels() {
    let id = AttemptId(42);

    assert_eq!(
        AttemptId::from_auth_window_label(&id.auth_window_label()),
        Some(id)
    );
    assert_eq!(AttemptId::from_auth_window_label("workspace-42"), None);
}
