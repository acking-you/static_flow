#!/usr/bin/env node

const port = process.env.KIRO_DEVTOOLS_PORT;
const email = process.env.KIRO_GOOGLE_EMAIL;
const password = process.env.KIRO_GOOGLE_PASSWORD;
const timeoutSeconds = Number(process.env.KIRO_MANUAL_TIMEOUT_SECONDS || "300");

if (!port || !email || !password) {
  console.error("KIRO_DEVTOOLS_PORT, KIRO_GOOGLE_EMAIL, and KIRO_GOOGLE_PASSWORD are required");
  process.exit(2);
}

const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

async function connectPage() {
  const deadline = Date.now() + 25_000;
  while (Date.now() < deadline) {
    try {
      const pages = await (await fetch(`http://127.0.0.1:${port}/json/list`)).json();
      const page = pages.find((item) => item.type === "page");
      if (page?.webSocketDebuggerUrl) {
        return page;
      }
    } catch {
      // Chrome may still be starting.
    }
    await sleep(250);
  }
  throw new Error("Chrome DevTools page target not found");
}

const page = await connectPage();
const ws = new WebSocket(page.webSocketDebuggerUrl);
let nextId = 0;
const pending = new Map();

ws.onmessage = (event) => {
  const message = JSON.parse(event.data);
  if (message.id && pending.has(message.id)) {
    pending.get(message.id)(message);
    pending.delete(message.id);
  }
};

await new Promise((resolve, reject) => {
  ws.onopen = resolve;
  ws.onerror = reject;
});

function send(method, params = {}) {
  return new Promise((resolve) => {
    const id = ++nextId;
    pending.set(id, resolve);
    ws.send(JSON.stringify({ id, method, params }));
  });
}

async function evalJs(expression) {
  const response = await send("Runtime.evaluate", {
    expression,
    returnByValue: true,
    awaitPromise: true,
  });
  if (response.exceptionDetails) {
    throw new Error(JSON.stringify(response.exceptionDetails));
  }
  return response.result?.result?.value;
}

function jsString(value) {
  return JSON.stringify(value);
}

async function state() {
  return await evalJs(`(() => ({
    title: document.title,
    url: location.href,
    text: document.body ? document.body.innerText.slice(0, 2200) : "",
    hasEmailInput: !!document.querySelector('#identifierId'),
    hasPasswordInput: !!document.querySelector('input[type="password"]'),
    passwordLength: [...document.querySelectorAll('input[type="password"]')].map((e) => e.value.length),
    buttons: [...document.querySelectorAll('button,a,[role="button"]')]
      .map((e) => (e.innerText || e.getAttribute('aria-label') || '').trim())
      .filter(Boolean)
      .slice(0, 60),
  }))()`);
}

async function clickText(label) {
  return await evalJs(`(() => {
    const target = ${jsString(label)};
    const primary = [...document.querySelectorAll('button,a,[role="button"]')];
    const el = primary.find((e) => (e.innerText || e.getAttribute('aria-label') || '').trim() === target);
    if (!el) return false;
    el.click();
    return true;
  })()`);
}

async function setInput(selector, value) {
  return await evalJs(`(() => {
    const e = document.querySelector(${jsString(selector)});
    if (!e) return false;
    e.focus();
    const setter = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, 'value').set;
    setter.call(e, ${jsString(value)});
    e.dispatchEvent(new Event('input', { bubbles: true }));
    e.dispatchEvent(new Event('change', { bubbles: true }));
    return e.value.length;
  })()`);
}

async function clickNext() {
  return await clickText("Next");
}

await send("Runtime.enable");
await send("Page.enable");

const deadline = Date.now() + timeoutSeconds * 1000;
let lastAction = "started";
let lastManualNoticeAt = 0;

while (Date.now() < deadline) {
  const current = await state();
  const text = current.text || "";
  const buttons = current.buttons || [];

  if (text.includes("Device authorized")) {
    console.log("Browser automation: device authorized");
    ws.close();
    process.exit(0);
  }

  if (text.includes("Something went wrong") && buttons.includes("Restart")) {
    await clickText("Restart");
    lastAction = "clicked Restart after Google error";
    console.log("Browser automation: clicked Restart after Google error");
    await sleep(2500);
    continue;
  }

  if (text.includes("Authorization requested") && buttons.includes("Approve")) {
    await clickText("Accept");
    await sleep(300);
    await clickText("Approve");
    lastAction = "clicked Approve";
    console.log("Browser automation: clicked Approve");
    await sleep(1200);
    continue;
  }

  if (current.hasEmailInput && text.includes("Email or phone")) {
    const length = await setInput("#identifierId", email);
    await sleep(250);
    const clicked = await clickNext();
    lastAction = `submitted email len=${length} clicked=${clicked}`;
    console.log("Browser automation: submitted email");
    await sleep(2500);
    continue;
  }

  if (current.hasPasswordInput && text.includes("Enter your password")) {
    const length = await setInput('input[type="password"]', password);
    await sleep(250);
    const clicked = await clickNext();
    lastAction = `submitted password len=${length} clicked=${clicked}`;
    console.log("Browser automation: submitted password");
    await sleep(3500);
    continue;
  }

  if (text.includes("Choose an account")) {
    const selected = await clickText(email);
    if (!selected) {
      await clickText("Use another account");
    }
    lastAction = selected ? "selected existing account" : "clicked Use another account";
    console.log(`Browser automation: ${lastAction}`);
    await sleep(1800);
    continue;
  }

  if (buttons.includes("Continue")) {
    await clickText("Continue");
    lastAction = "clicked Continue";
    console.log("Browser automation: clicked Continue");
    await sleep(2000);
    continue;
  }

  const lower = text.toLowerCase();
  if (
    lower.includes("2-step verification") ||
    lower.includes("verify it") ||
    lower.includes("captcha") ||
    lower.includes("unusual traffic") ||
    lower.includes("couldn’t verify") ||
    lower.includes("couldn't verify")
  ) {
    if (Date.now() - lastManualNoticeAt > 10_000) {
      console.log("Browser automation: manual Google challenge detected; complete it in the launched browser");
      lastManualNoticeAt = Date.now();
    }
    lastAction = "waiting for manual challenge";
    await sleep(2000);
    continue;
  }

  await sleep(1000);
}

const finalState = await state();
ws.close();
console.error(
  `Browser automation timed out; lastAction=${lastAction}; title=${finalState.title}; url=${finalState.url}; text=${JSON.stringify((finalState.text || "").slice(0, 500))}`
);
process.exit(1);
