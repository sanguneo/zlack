use std::cell::Cell;
use webkit2gtk::{
    glib::{self, prelude::ObjectExt},
    traits::WebViewExt,
    PolicyError, WebView,
};

const WEB_CLIENT_URL: &str = "https://app.slack.com/client";

#[derive(Default)]
struct LoginFallback {
    used: Cell<bool>,
}

impl LoginFallback {
    fn target(&self, uri: &str, error: &glib::Error) -> Option<&'static str> {
        if !error.matches(PolicyError::CannotShowUri)
            || !url::Url::parse(uri).is_ok_and(|url| url.scheme() == "slack")
            || self.used.replace(true)
        {
            return None;
        }

        // Never propagate the deep link's query, fragment, or credentials.
        Some(WEB_CLIENT_URL)
    }
}

pub(super) fn install(webview: &WebView) {
    let fallback = LoginFallback::default();
    webview.connect_load_failed(move |webview, _event, uri, error| {
        let Some(target) = fallback.target(uri, error) else {
            return false;
        };

        // Slack can finish sign-in by launching its desktop protocol. WebKitGTK
        // cannot display it. Continue in the same view/context so the completed
        // web login remains available, without invoking an external application.
        // Defer until load-failed has unwound and suppress its default error page.
        // One attempt per view also bounds redirects back to slack: if login did
        // not complete. Do not reset on load-changed: failed loads emit Finished.
        let webview = webview.downgrade();
        glib::idle_add_local_once(move || {
            if let Some(webview) = webview.upgrade() {
                webview.load_uri(target);
            }
        });
        true
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use webkit2gtk::NetworkError;

    fn unsupported_uri() -> glib::Error {
        glib::Error::new(PolicyError::CannotShowUri, "synthetic error")
    }

    #[test]
    fn recovers_rejected_slack_links_without_forwarding_parameters() {
        for uri in [
            "slack://open?team=T123",
            "SLACK://open?team=T123",
            "slack://signin?token=synthetic-secret&redirect=https://example.org/#fragment",
            "slack://user:password@example.org/path?url=https://example.org/",
        ] {
            assert_eq!(
                LoginFallback::default().target(uri, &unsupported_uri()),
                Some(WEB_CLIENT_URL)
            );
        }
    }

    #[test]
    fn leaves_other_schemes_and_error_domains_to_webkit() {
        let fallback = LoginFallback::default();
        for uri in [
            "https://app.slack.com/client",
            "http://example.org/",
            "file:///tmp/example",
            "mailto:example@example.org",
            "slack-other://open",
            "not a URL",
            "",
        ] {
            assert_eq!(fallback.target(uri, &unsupported_uri()), None);
        }
        for error in [
            glib::Error::new(NetworkError::Cancelled, "cancelled"),
            glib::Error::new(PolicyError::CannotShowMimeType, "MIME type"),
            glib::Error::new(glib::FileError::Failed, "unrelated failure"),
        ] {
            assert_eq!(fallback.target("slack://open", &error), None);
        }
        // Unrelated failures do not consume the recovery attempt.
        assert_eq!(
            fallback.target("slack://open", &unsupported_uri()),
            Some(WEB_CLIENT_URL)
        );
    }

    #[test]
    fn bounds_recovery_per_view_without_sharing_state_between_workspaces() {
        let first_view = LoginFallback::default();
        let second_view = LoginFallback::default();
        let error = unsupported_uri();
        assert_eq!(
            first_view.target("slack://open", &error),
            Some(WEB_CLIENT_URL)
        );
        assert_eq!(first_view.target("slack://open", &error), None);
        assert_eq!(
            second_view.target("slack://open", &error),
            Some(WEB_CLIENT_URL)
        );
    }

    #[test]
    #[ignore = "requires a GTK display; run under xvfb-run with --ignored --test-threads=1"]
    fn rejected_slack_navigation_recovers_in_the_same_webview() {
        use gtk::prelude::*;
        use std::{rc::Rc, time::Duration};
        use webkit2gtk::{
            NavigationPolicyDecision, NavigationPolicyDecisionExt, PolicyDecisionExt,
            PolicyDecisionType, URIRequestExt,
        };

        gtk::init().expect("GTK display required");
        let webview = WebView::new();
        let window = gtk::Window::new(gtk::WindowType::Toplevel);
        window.add(&webview);
        window.show_all();
        install(&webview);

        let main_loop = glib::MainLoop::new(None, false);
        let recovered = Rc::new(Cell::new(false));
        let observed = recovered.clone();
        let finished = main_loop.clone();
        webview.connect_decide_policy(move |_, decision, kind| {
            if kind == PolicyDecisionType::NavigationAction {
                let navigation = decision.downcast_ref::<NavigationPolicyDecision>().unwrap();
                if navigation
                    .navigation_action()
                    .and_then(|action| action.request())
                    .and_then(|request| request.uri())
                    .as_deref()
                    == Some(WEB_CLIENT_URL)
                {
                    // Observe the real navigation but keep the test offline.
                    decision.ignore();
                    observed.set(true);
                    finished.quit();
                    return true;
                }
            }
            false
        });
        let deadline = main_loop.clone();
        let timeout =
            glib::timeout_add_local_once(Duration::from_secs(10), move || deadline.quit());
        webview.load_uri("slack://open?team=T_SYNTHETIC");
        main_loop.run();
        if recovered.get() {
            timeout.remove();
        }
        window.close();
        assert!(
            recovered.get(),
            "WebKit did not recover the rejected slack: navigation"
        );
    }
}
