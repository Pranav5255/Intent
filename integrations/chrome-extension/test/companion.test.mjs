// The shared companion background script, run under a fake Chrome and a fake Firefox.
// `node --test` from integrations/chrome-extension.
import assert from "node:assert/strict";
import { test } from "node:test";

const BACKGROUND = new URL("../../firefox-extension/background.js", import.meta.url);

function event() {
  const listeners = [];
  return { addListener: (fn) => listeners.push(fn), listeners };
}

// A browser API with the parts background.js uses. `scheme` is the extension URL scheme.
function fakeApi(scheme) {
  return {
    runtime: {
      getURL: (path) => `${scheme}://abcdefghijklmnopabcdefghijklmnop/${path}`,
      onStartup: event(),
      onMessage: event()
    },
    tabs: { get: async () => ({}), onActivated: event(), onUpdated: event(), onRemoved: event() }
  };
}

let loads = 0;
async function load(global, scheme) {
  delete globalThis.browser;
  delete globalThis.chrome;
  const api = fakeApi(scheme);
  globalThis[global] = api;
  // A fresh module instance per browser.
  const mod = await import(`${BACKGROUND.href}?load=${++loads}`);
  return { api, mod };
}

test("Chrome sends chrome events with the same shapes", async () => {
  const { mod } = await load("chrome", "chrome-extension");
  assert.equal(mod.SOURCE, "chrome");
  const ev = mod.makeEvent({ id: 7, windowId: 2, url: "https://u:p@docs.rs/axum/?q=1#x", title: " axum  - Rust " });
  assert.equal(ev.source, "chrome");
  assert.equal(ev.type, "tab_change");
  assert.deepEqual(ev.payload, { url: "https://docs.rs/axum/", title: "axum - Rust", tab_id: 7, window_id: 2 });
  assert.equal(mod.makeEvent({ id: 1, url: "chrome://settings/" }), null);
  const action = mod.makeUserActionEvent(
    { id: 7, windowId: 2, url: "https://news.example/a?x=1", incognito: false },
    { action: "click", sensitive_page: false, target: { tag: "A", role: "link", label: "Read" } }
  );
  assert.equal(action.source, "chrome");
  assert.equal(action.payload.url, "https://news.example/a");
  assert.equal(mod.makeUserActionEvent({ id: 7, url: "https://a.example/", incognito: true }, { action: "click", target: { tag: "a" } }), null);
});

test("Firefox still sends firefox events", async () => {
  const { mod } = await load("browser", "moz-extension");
  assert.equal(mod.SOURCE, "firefox");
  assert.equal(mod.makeEvent({ id: 1, windowId: 1, url: "https://a.example/", title: "a" }).source, "firefox");
});

test("messages are answered through sendResponse, which Chrome needs", async () => {
  const { api } = await load("chrome", "chrome-extension");
  const [listener] = api.runtime.onMessage.listeners;
  const answer = await new Promise((resolve) => {
    // A private tab gets no capture, whatever the config says.
    const keepOpen = listener({ kind: "intent-detailed-capture-status" }, { tab: { incognito: true } }, resolve);
    assert.equal(keepOpen, true);
  });
  assert.deepEqual(answer, { enabled: false });
});
