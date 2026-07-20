import test from "node:test";
import assert from "node:assert/strict";

globalThis.browser = {
  tabs: {
    onActivated: { addListener() {} },
    onUpdated: { addListener() {} },
    onRemoved: { addListener() {} }
  },
  runtime: { onStartup: { addListener() {} }, onMessage: { addListener() {} } }
};
const { makeEvent, makeUserActionEvent, sanitizeUrl } = await import("../background.js");

test("sanitizes browser URLs before they leave Firefox", () => {
  assert.equal(
    sanitizeUrl("https://alice:password@example.com/docs?token=secret#section"),
    "https://example.com/docs"
  );
  assert.equal(sanitizeUrl("about:preferences"), null);
  assert.equal(sanitizeUrl("file:///home/pranav/private.txt"), null);
});

test("emits active-tab metadata only", () => {
  const event = makeEvent({ id: 7, url: "https://example.com/a?x=1", title: "A page" });
  assert.match(event.id, /^[0-9a-f-]{36}$/);
  assert.equal(event.source, "firefox");
  assert.equal(event.type, "tab_change");
  assert.deepEqual(event.payload, { url: "https://example.com/a", title: "A page", tab_id: 7 });
});

test("emits bounded semantic actions without form values", () => {
  const event = makeUserActionEvent(
    { id: 7, windowId: 3, url: "https://example.com/settings?token=secret" },
    {
      action: "click",
      sensitive_page: false,
      target: { tag: "button", role: "button", label: "Save settings", input_type: "submit", href: "https://example.com/next?q=1" }
    }
  );
  assert.equal(event.type, "user_action");
  assert.deepEqual(event.payload, {
    url: "https://example.com/settings",
    tab_id: 7,
    window_id: 3,
    action: "click",
    target: { tag: "button", role: "button", label: "Save settings", input_type: "submit", href: "https://example.com/next" },
    sensitive_page: false
  });
  assert.equal("value" in event.payload.target, false);
});

test("strips sensitive-page target details and rejects private tabs", () => {
  const event = makeUserActionEvent(
    { id: 7, windowId: 3, url: "https://example.com/login" },
    { action: "link_activation", sensitive_page: true, target: { tag: "a", role: "link", label: "Sign in", href: "https://example.com/private" } }
  );
  assert.deepEqual(event.payload.target, { tag: "a", role: "link" });
  assert.equal(event.payload.action, "click");
  assert.equal(makeUserActionEvent({ id: 7, windowId: 3, incognito: true, url: "https://example.com" }, { action: "click", target: { tag: "button", role: "button" } }), null);
});

test("emits a coarse scroll action without page content or exact offsets", () => {
  const event = makeUserActionEvent(
    { id: 7, windowId: 3, url: "https://example.com/guide?token=secret" },
    {
      action: "scroll",
      sensitive_page: false,
      target: { tag: "document", role: "document" },
      scroll: { direction: "down", position_bucket: 6 }
    }
  );
  assert.deepEqual(event.payload, {
    url: "https://example.com/guide",
    tab_id: 7,
    window_id: 3,
    action: "scroll",
    target: { tag: "document", role: "document" },
    sensitive_page: false,
    scroll: { direction: "down", position_bucket: 6 }
  });
  assert.equal(makeUserActionEvent(
    { id: 7, windowId: 3, url: "https://example.com" },
    { action: "scroll", sensitive_page: true, target: { tag: "document", role: "document" }, scroll: { direction: "down", position_bucket: 1 } }
  ), null);
});
