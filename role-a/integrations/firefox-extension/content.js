(() => {
  const SENSITIVE_PATH = /\b(login|log-in|signin|sign-in|auth|oauth|sso|account|billing|payment|checkout|wallet)\b/i;
  const INTERACTIVE_SELECTOR = "a,button,input,select,[role='button'],[role='link'],[role='checkbox'],[role='radio'],[role='switch'],[role='menuitem'],form";
  let captureEnabled = false;
  let contextEnabled = false;

  function isSensitivePage() {
    if (SENSITIVE_PATH.test(location.pathname)) return true;
    return Boolean(document.querySelector(
      "input[type='password'], input[autocomplete*='cc-'], input[name*='card' i], input[name*='payment' i]"
    ));
  }

  function interactiveTarget(target) {
    if (!(target instanceof Element)) return null;
    return target.closest(INTERACTIVE_SELECTOR);
  }

  function text(value, maximum) {
    return typeof value === "string" ? value.replace(/\s+/g, " ").trim().slice(0, maximum) : "";
  }

  function labelFor(element) {
    const ariaLabel = text(element.getAttribute("aria-label"), 160);
    if (ariaLabel) return ariaLabel;
    if (element.labels && element.labels.length) return text(element.labels[0].textContent, 160);
    if (["BUTTON", "A", "OPTION"].includes(element.tagName)) return text(element.textContent, 160);
    return "";
  }

  function describeTarget(element, sensitive) {
    const tag = element.tagName.toLowerCase();
    const role = text(element.getAttribute("role"), 64).toLowerCase();
    if (sensitive) return { tag, role };
    const target = { tag, role };
    const label = labelFor(element);
    if (label) target.label = label;
    if (element instanceof HTMLInputElement) {
      target.input_type = element.type.toLowerCase();
      if (["checkbox", "radio"].includes(element.type)) target.checked = element.checked;
    } else if (element instanceof HTMLSelectElement) {
      target.input_type = "select";
    }
    if (element instanceof HTMLAnchorElement && element.href) target.href = element.href;
    return target;
  }

  function socialAction(element) {
    const value = (element.getAttribute("data-testid") || "").toLowerCase();
    return { like: "like", unlike: "like", reply: "reply", retweet: "repost", unretweet: "repost", share: "share", follow: "follow", unfollow: "unfollow" }[value] || null;
  }

  function postContext(element, sensitive) {
    if (sensitive || !contextEnabled) return undefined;
    const article = element.closest("article");
    if (!article) return undefined;
    const text = textContent(article, 1000);
    if (!text) return undefined;
    const author = textContent(article.querySelector('[data-testid="User-Name"]'), 160);
    return { kind: "social_post", text_excerpt: text, ...(author ? { author } : {}) };
  }

  function textContent(element, maximum) { return element ? text(element.textContent, maximum) : ""; }

  function send(action, element, sensitive) {
    if (!captureEnabled || !element) return;
    browser.runtime.sendMessage({
      kind: "intent-os-user-action",
      payload: { action, sensitive_page: sensitive, target: describeTarget(element, sensitive), context: postContext(element, sensitive) }
    }).catch(() => undefined);
  }

  function onClick(event) {
    if (!event.isTrusted || !captureEnabled) return;
    const element = interactiveTarget(event.target);
    if (!element) return;
    const sensitive = isSensitivePage();
    if (element instanceof HTMLInputElement && ["checkbox", "radio"].includes(element.type)) return;
    if (element instanceof HTMLSelectElement) return;
    send(socialAction(element) || (element instanceof HTMLAnchorElement ? "link_activation" : "click"), element, sensitive);
  }

  function onSubmit(event) {
    if (!event.isTrusted || !captureEnabled || !(event.target instanceof HTMLFormElement)) return;
    send("form_submit", event.target, isSensitivePage());
  }

  function onChange(event) {
    if (!event.isTrusted || !captureEnabled || isSensitivePage()) return;
    const element = event.target;
    if (element instanceof HTMLInputElement && ["checkbox", "radio"].includes(element.type)) {
      send("toggle", element, false);
    } else if (element instanceof HTMLSelectElement) {
      send("select_change", element, false);
    }
  }

  async function refreshCaptureStatus() {
    try {
      const status = await browser.runtime.sendMessage({ kind: "intent-os-detailed-capture-status" });
      captureEnabled = Boolean(status && status.enabled);
      contextEnabled = Boolean(status && status.context_enabled);
    } catch {
      captureEnabled = false;
    }
  }

  document.addEventListener("click", onClick, true);
  document.addEventListener("submit", onSubmit, true);
  document.addEventListener("change", onChange, true);
  void refreshCaptureStatus();
  setInterval(() => void refreshCaptureStatus(), 30_000);
})();
