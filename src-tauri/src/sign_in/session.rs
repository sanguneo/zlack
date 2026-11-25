use super::MagicLogin;
use std::{
    collections::{HashMap, VecDeque},
    time::{Duration, Instant},
};
use url::Url;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct AttemptId(u64);

impl AttemptId {
    pub(super) fn auth_window_label(self) -> String {
        format!("sign-in-{}", self.0)
    }

    pub(super) fn from_auth_window_label(label: &str) -> Option<Self> {
        label.strip_prefix("sign-in-")?.parse().ok().map(Self)
    }
}

#[derive(Debug, Clone)]
pub(super) struct AttemptOrigin {
    pub(super) window_label: String,
    pub(super) return_url: Url,
}

#[derive(Debug, Clone)]
pub(super) struct Exchange {
    pub(super) id: AttemptId,
    pub(super) login: MagicLogin,
    pub(super) origin: AttemptOrigin,
}

#[derive(Debug)]
pub(super) enum CallbackDecision {
    Duplicate,
    NeedsConfirmation(Exchange),
    Start(Exchange),
    Queued,
}

#[derive(Debug)]
struct BrowserAttempt {
    id: AttemptId,
    origin: AttemptOrigin,
    expires_at: Instant,
}

#[derive(Debug)]
struct SignInHereException {
    url: Url,
    expires_at: Instant,
}

#[derive(Default)]
pub(crate) struct SignInState {
    next_attempt_id: u64,
    browser_attempts: VecDeque<BrowserAttempt>,
    queued_exchanges: VecDeque<Exchange>,
    active_exchange: Option<Exchange>,
    seen_callbacks: HashMap<String, Instant>,
    sign_in_here: HashMap<String, SignInHereException>,
}

impl SignInState {
    fn next_id(&mut self) -> AttemptId {
        self.next_attempt_id = self.next_attempt_id.wrapping_add(1);
        AttemptId(self.next_attempt_id)
    }

    fn remove_expired(&mut self, now: Instant) {
        self.browser_attempts
            .retain(|attempt| attempt.expires_at > now);
        self.seen_callbacks
            .retain(|_, expires_at| *expires_at > now);
        self.sign_in_here
            .retain(|_, exception| exception.expires_at > now);
    }

    pub(super) fn record_browser_attempt(
        &mut self,
        origin: AttemptOrigin,
        now: Instant,
        lifetime: Duration,
    ) -> AttemptId {
        self.remove_expired(now);
        let id = self.next_id();
        self.browser_attempts.push_back(BrowserAttempt {
            id,
            origin,
            expires_at: now + lifetime,
        });
        id
    }

    pub(super) fn remove_browser_attempt(&mut self, id: AttemptId) {
        self.browser_attempts.retain(|attempt| attempt.id != id);
    }

    pub(super) fn submit_callback(
        &mut self,
        login: MagicLogin,
        fallback_origin: AttemptOrigin,
        now: Instant,
        callback_lifetime: Duration,
    ) -> CallbackDecision {
        self.remove_expired(now);
        let fingerprint = login.request_url().to_string();
        if self.seen_callbacks.contains_key(&fingerprint) {
            return CallbackDecision::Duplicate;
        }
        self.seen_callbacks
            .insert(fingerprint, now + callback_lifetime);

        let attempt = self.browser_attempts.pop_front();
        let (id, origin) = attempt
            .map(|attempt| (attempt.id, attempt.origin))
            .unwrap_or_else(|| (self.next_id(), fallback_origin));
        CallbackDecision::NeedsConfirmation(Exchange { id, login, origin })
    }

    pub(super) fn confirm_callback(&mut self, exchange: Exchange) -> CallbackDecision {
        if self.active_exchange.is_some() {
            self.queued_exchanges.push_back(exchange);
            CallbackDecision::Queued
        } else {
            self.active_exchange = Some(exchange.clone());
            CallbackDecision::Start(exchange)
        }
    }

    pub(super) fn reject_callback(&mut self, exchange: &Exchange) {
        self.seen_callbacks
            .remove(&exchange.login.request_url().to_string());
    }

    pub(super) fn response_matches(&self, id: AttemptId, url: &Url) -> bool {
        self.active_exchange
            .as_ref()
            .is_some_and(|exchange| exchange.id == id && exchange.login.request_url() == *url)
    }

    pub(super) fn complete(&mut self, id: AttemptId) -> Option<(Exchange, Option<Exchange>)> {
        if !self
            .active_exchange
            .as_ref()
            .is_some_and(|exchange| exchange.id == id)
        {
            return None;
        }
        let completed = self.active_exchange.take()?;
        let next = self.queued_exchanges.pop_front();
        self.active_exchange = next.clone();
        Some((completed, next))
    }

    pub(super) fn allow_sign_in_here(
        &mut self,
        window_label: &str,
        url: &Url,
        now: Instant,
    ) -> bool {
        self.remove_expired(now);
        if self
            .sign_in_here
            .get(window_label)
            .is_some_and(|exception| exception.url == *url)
        {
            self.sign_in_here.remove(window_label);
            true
        } else {
            false
        }
    }

    pub(super) fn allow_next_sign_in_here(
        &mut self,
        window_label: String,
        url: Url,
        now: Instant,
        lifetime: Duration,
    ) {
        self.sign_in_here.insert(
            window_label,
            SignInHereException {
                url,
                expires_at: now + lifetime,
            },
        );
    }

    pub(super) fn clear_sign_in_here(&mut self, window_label: &str) {
        self.sign_in_here.remove(window_label);
    }
}

#[cfg(test)]
#[path = "session_tests.rs"]
mod tests;
