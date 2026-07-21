const EVENT_ENDPOINT = "http://127.0.0.1:9477/v1/event";
const CONFIG_ENDPOINT = "http://127.0.0.1:9477/v1/detailed-capture/config";
const MAX_PENDING_EVENTS = 100;
const DEBOUNCE_MS = 300;
const CONFIG_CACHE_MS = 30_000;
const USER_ACTIONS = new Set(["click", "link_activation", "form_submit", "toggle", "select_change", "scroll", "like", "reply", "repost", "share", "follow", "unfollow"]);

const pendingEvents = [];
const lastTabs = new Map();
const timers = new Map();
let configCache = null;
let configFetchedAt = 0;
let configRequest = null;

export function sanitizeUrl(rawUrl) {
  try {
    const url = new URL(rawUrl);
    if (url.protocol !== "http:" && url.protocol !== "https:") return null;
    url.username = "";
    url.password = "";
    url.search = "";
    url.hash = "";
    return url.toString();
  } catch {
    return null;
  }
}

function boundedText(value, maximum) {
  return typeof value === "string" ? value.replace(/\s+/g, " ").trim().slice(0, maximum) : "";
}

export function makeEvent(tab) {
  const url = sanitizeUrl(tab.url);
  if (!url) return null;
  return {
    id: crypto.randomUUID(),
    ts: Math.floor(Date.now() / 1000),
    source: "firefox",
    type: "tab_change",
    payload: {
      url,
      title: boundedText(tab.title, 512),
      tab_id: tab.id,
      window_id: tab.windowId
    }
  };
}

export function makeUserActionEvent(tab, actionPayload) {
  if (!tab || tab.incognito || !actionPayload || !USER_ACTIONS.has(actionPayload.action)) return null;
  const url = sanitizeUrl(tab.url);
  const rawTarget = actionPayload.target || {};
  const tag = boundedText(rawTarget.tag, 64).toLowerCase();
  const role = boundedText(rawTarget.role, 64).toLowerCase();
  if (!url || !tag) return null;

  const sensitivePage = Boolean(actionPayload.sensitive_page);
  if (sensitivePage && actionPayload.action === "scroll") return null;
  const action = sensitivePage ? (actionPayload.action === "form_submit" ? "form_submit" : "click") : actionPayload.action;
  const target = { tag, role };
  if (!sensitivePage) {
    const label = boundedText(rawTarget.label, 160);
    const inputType = boundedText(rawTarget.input_type, 64).toLowerCase();
    const href = sanitizeUrl(rawTarget.href);
    if (label) target.label = label;
    if (inputType) target.input_type = inputType;
    if (href) target.href = href;
    if (typeof rawTarget.checked === "boolean") target.checked = rawTarget.checked;
  }

  let context;
  if (!sensitivePage && actionPayload.context && typeof actionPayload.context === "object") {
    const kind = boundedText(actionPayload.context.kind, 32);
    const text_excerpt = boundedText(actionPayload.context.text_excerpt, 1000);
    const author = boundedText(actionPayload.context.author, 160);
    if (kind && text_excerpt) context = { kind, text_excerpt, ...(author ? { author } : {}) };
  }

  let scroll;
  if (action === "scroll") {
    const rawScroll = actionPayload.scroll || {};
    if (tag !== "document" || role !== "document") return null;
    if (!["up", "down"].includes(rawScroll.direction) || !Number.isInteger(rawScroll.position_bucket) || rawScroll.position_bucket < 0 || rawScroll.position_bucket > 10) return null;
    scroll = { direction: rawScroll.direction, position_bucket: rawScroll.position_bucket };
  }

  return {
    id: crypto.randomUUID(),
    ts: Math.floor(Date.now() / 1000),
    source: "firefox",
    type: "user_action",
    payload: {
      url,
      tab_id: tab.id,
      window_id: tab.windowId,
      action,
      target,
      sensitive_page: sensitivePage,
      ...(scroll ? { scroll } : {}),
      ...(context ? { context } : {})
    }
  };
}

async function getDetailedConfig() {
  if (configCache && Date.now() - configFetchedAt < CONFIG_CACHE_MS) return configCache;
  if (configRequest) return configRequest;
  configRequest = fetch(CONFIG_ENDPOINT)
    .then((response) => (response.ok ? response.json() : null))
    .then((config) => {
      configCache = config;
      configFetchedAt = Date.now();
      return config;
    })
    .catch(() => null)
    .finally(() => { configRequest = null; });
  return configRequest;
}

async function detailedCaptureEnabled() {
  const config = await getDetailedConfig();
  return Boolean(config && config.browser && config.browser.enabled);
}

async function deliver(event) {
  const response = await fetch(EVENT_ENDPOINT, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify(event)
  });
  if (!response.ok) throw new Error("Intent OS server returned " + response.status);
}

async function flush() {
  while (pendingEvents.length) {
    const event = pendingEvents[0];
    try {
      await deliver(event);
      pendingEvents.shift();
    } catch {
      return;
    }
  }
}

function queueEvent(event) {
  if (!event) return;
  if (pendingEvents.length === MAX_PENDING_EVENTS) pendingEvents.shift();
  pendingEvents.push(event);
  void flush();
}

function queueTabEvent(event) {
  if (!event) return;
  const fingerprint = event.payload.url + "\n" + event.payload.title;
  const previous = lastTabs.get(event.payload.tab_id);
  if (previous && previous.fingerprint === fingerprint) return;
  lastTabs.set(event.payload.tab_id, {
    fingerprint,
    url: event.payload.url,
    windowId: event.payload.window_id
  });
  queueEvent(event);
}

function queueTabCloseEvent(tabId) {
  const tab = lastTabs.get(tabId);
  if (!tab) return;
  queueEvent({
    id: crypto.randomUUID(),
    ts: Math.floor(Date.now() / 1000),
    source: "firefox",
    type: "tab_close",
    payload: { url: tab.url, tab_id: tabId, window_id: tab.windowId }
  });
}

function scheduleTab(tabId) {
  const priorTimer = timers.get(tabId);
  if (priorTimer) clearTimeout(priorTimer);
  timers.set(tabId, setTimeout(async () => {
    timers.delete(tabId);
    try {
      queueTabEvent(makeEvent(await browser.tabs.get(tabId)));
    } catch {
      // The tab may have closed before its URL became available.
    }
  }, DEBOUNCE_MS));
}

async function handleMessage(message, sender) {
  if (!message || !message.kind || !sender.tab || sender.tab.incognito) return { enabled: false };
  if (message.kind === "intent-os-detailed-capture-status") {
    const config = await getDetailedConfig();
    return { enabled: Boolean(config && config.browser && config.browser.enabled), context_enabled: Boolean(config && config.browser && config.browser.context_enabled) };
  }
  if (message.kind !== "intent-os-user-action" || !(await detailedCaptureEnabled())) {
    return { accepted: false };
  }
  const event = makeUserActionEvent(sender.tab, message.payload);
  if (!event) return { accepted: false };
  queueEvent(event);
  return { accepted: true };
}

browser.tabs.onActivated.addListener(({ tabId }) => scheduleTab(tabId));
browser.tabs.onUpdated.addListener((tabId, changeInfo) => {
  if (changeInfo.url || changeInfo.title || changeInfo.status === "complete") scheduleTab(tabId);
});
browser.tabs.onRemoved.addListener((tabId) => {
  queueTabCloseEvent(tabId);
  lastTabs.delete(tabId);
  const timer = timers.get(tabId);
  if (timer) clearTimeout(timer);
  timers.delete(tabId);
});
browser.runtime.onStartup.addListener(() => void flush());
browser.runtime.onMessage.addListener(handleMessage);
