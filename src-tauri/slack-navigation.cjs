// Decides where a Slack page that Slack asks to open in a new window should go.
//
// WKWebView (and WebView2/WebKitGTK under Tauri 1) silently drops
// window.open() and target="_blank" navigations, so Slack's "Add a workspace"
// actions ("Sign in to another workspace", "Find workspaces", "Create a new
// workspace") did nothing (#9). Sign-in pages have to load in this WebView so
// the new workspace's session cookie lands next to the existing ones; other
// non-app Slack pages (help center, marketing, legal) go to the OS browser
// like any other external link.
const ZlackSlackNavigation = (function buildSlackNavigation() {
  const SLACK_HOST_RE = /(^|\.)slack\.com$/i;
  // Hosts that serve files or APIs, never a page a user signs in on.
  const NON_PAGE_HOST_RE = /^(?:files|files-edge|files-origin|api|edgeapi|wss-[\w-]+|slack-imgs|ca|a|b|emoji|downloads|slackb)\.slack\.com$/i;
  const APP_PATH_RE = /^\/(?:client|archives)(?:\/|$)/i;
  // Account and workspace-onboarding flows.
  const AUTH_PATH_RE = /^\/(?:signin|sign_in|workspace-signin|get-started|getstarted|create|ssb|checkcookie|checkmail|confirm|join|shared_invite|invite|sso|oauth|openid|forgot|reset|account\/(?:signin|sso)|x-\d+)(?:[\/?#._-]|$)/i;

  function parse(href, baseHref) {
    try {
      return new URL(href, baseHref);
    } catch (_) {
      return null;
    }
  }

  function isHttp(url) {
    return Boolean(url) && (url.protocol === 'http:' || url.protocol === 'https:');
  }

  function isSlackHost(hostname) {
    return SLACK_HOST_RE.test(hostname || '');
  }

  // A workspace's own host (acme.slack.com, acme.enterprise.slack.com), whose
  // root page is that workspace's sign-in screen.
  function isWorkspaceHost(hostname) {
    const host = (hostname || '').toLowerCase();
    return isSlackHost(host)
      && host !== 'slack.com'
      && host !== 'www.slack.com'
      && host !== 'app.slack.com'
      && !NON_PAGE_HOST_RE.test(host);
  }

  // 'app'      — Slack client route; existing in-app handling applies.
  // 'signin'   — account/workspace flow; load in this WebView.
  // 'external' — open in the OS browser.
  // null       — not a page Zlack should route (non-http, file/API host).
  function classify(href, baseHref) {
    if (typeof href !== 'string' || !href.trim()) return null;
    const url = parse(href, baseHref);
    if (!isHttp(url)) return null;
    if (!isSlackHost(url.hostname)) return 'external';
    if (url.username || url.password) return null;
    if (NON_PAGE_HOST_RE.test(url.hostname)) return null;
    if (APP_PATH_RE.test(url.pathname)) return 'app';
    if (AUTH_PATH_RE.test(url.pathname)) return 'signin';
    // Invite links: join.slack.com/t/<workspace>/shared_invite/...
    if (url.hostname.toLowerCase() === 'join.slack.com') return 'signin';
    if (isWorkspaceHost(url.hostname) && (url.pathname === '/' || url.pathname === '')) return 'signin';
    // Other app.slack.com pages (huddle windows and the like) keep the
    // WebView's own behavior rather than being pushed to the OS browser.
    if (url.hostname.toLowerCase() === 'app.slack.com') return null;
    return 'external';
  }

  return { classify, isWorkspaceHost };
})();

if (typeof module !== 'undefined' && module.exports) {
  module.exports = ZlackSlackNavigation;
}
