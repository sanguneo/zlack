const assert = require("node:assert/strict");
const test = require("node:test");

const navigation = require("../src-tauri/slack-navigation.cjs");

const BASE = "https://app.slack.com/client/T1/C1";
const classify = (href) => navigation.classify(href, BASE);

test("Add a workspace flows load in the WebView", () => {
  for (const href of [
    "https://slack.com/signin",
    "https://slack.com/signin?redir=%2Fgantry%2Fauth",
    "https://slack.com/signin/find",
    "https://slack.com/workspace-signin",
    "https://slack.com/get-started#/createnew",
    "https://slack.com/get-started#/find",
    "https://slack.com/create",
    "https://app.slack.com/ssb/signin",
    "https://acme.slack.com/",
    "https://acme.slack.com",
    "https://acme.slack.com/sso/saml/start",
    "https://acme.enterprise.slack.com/",
    "https://join.slack.com/t/acme/shared_invite/zt-abc",
    "https://slack.com/checkcookie?redir=x",
    "/signin",
  ]) {
    assert.equal(classify(href), "signin", href);
  }
});

test("client routes keep the existing in-app handling", () => {
  for (const href of [
    "https://app.slack.com/client",
    "https://app.slack.com/client/T123/C456",
    "https://acme.slack.com/client",
    "https://acme.slack.com/archives/C123/p1700000000000100",
  ]) {
    assert.equal(classify(href), "app", href);
  }
});

test("other Slack pages open in the OS browser", () => {
  for (const href of [
    "https://slack.com/help/articles/123",
    "https://slack.com/intl/en-gb/help",
    "https://slack.com/pricing",
    "https://slack.com/apps/A123",
    "https://acme.slack.com/admin",
    "https://slack.com/signinx",
    "https://example.com/signin",
  ]) {
    assert.equal(classify(href), "external", href);
  }
});

test("file/API hosts, app.slack.com pages and non-http URLs are left alone", () => {
  for (const href of [
    "https://files.slack.com/files-pri/T1-F1/download/a.pdf",
    "https://api.slack.com/apps",
    "https://app.slack.com/huddle/T1/C1",
    "https://user:pass@slack.com/signin",
    "slack://open?team=T1",
    "mailto:a@example.com",
    "javascript:alert(1)",
    "",
  ]) {
    assert.equal(classify(href), null, href);
  }
});

test("look-alike hosts are not treated as Slack", () => {
  assert.equal(classify("https://evilslack.com/signin"), "external");
  assert.equal(classify("https://slack.com.evil.example/signin"), "external");
  assert.equal(navigation.isWorkspaceHost("evilslack.com"), false);
  assert.equal(navigation.isWorkspaceHost("acme.slack.com"), true);
  assert.equal(navigation.isWorkspaceHost("app.slack.com"), false);
  assert.equal(navigation.isWorkspaceHost("files.slack.com"), false);
});
