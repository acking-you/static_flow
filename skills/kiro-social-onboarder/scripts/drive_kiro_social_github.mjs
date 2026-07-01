#!/usr/bin/env node

const port = process.env.KIRO_DEVTOOLS_PORT;
const githubLogin = process.env.KIRO_GITHUB_LOGIN || "";
const githubPassword = process.env.KIRO_GITHUB_PASSWORD || "";
const timeoutSeconds = Number(process.env.KIRO_MANUAL_TIMEOUT_SECONDS || "600");

if (!port) {
  console.error("KIRO_DEVTOOLS_PORT is required");
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
    text: document.body ? document.body.innerText.slice(0, 2600) : "",
    hasLoginInput: !!document.querySelector('#login_field,input[name="login"],input[name="user_login"],input[type="email"]'),
    hasPasswordInput: !!document.querySelector('#password,input[name="password"],input[type="password"]'),
    buttons: [...document.querySelectorAll('button,a,[role="button"],input[type="submit"]')]
      .map((e) => (e.innerText || e.value || e.getAttribute('aria-label') || '').trim())
      .filter(Boolean)
      .slice(0, 80),
  }))()`);
}

async function clickText(label) {
  return await evalJs(`(() => {
    const target = ${jsString(label)};
    const primary = [...document.querySelectorAll('button,a,[role="button"],input[type="submit"]')];
    const el = primary.find((e) => (e.innerText || e.value || e.getAttribute('aria-label') || '').trim() === target);
    if (!el) return false;
    el.click();
    return true;
  })()`);
}

async function clickTextContaining(fragment) {
  return await evalJs(`(() => {
    const target = ${jsString(fragment)}.toLowerCase();
    const primary = [...document.querySelectorAll('button,a,[role="button"],input[type="submit"]')];
    const el = primary.find((e) => ((e.innerText || e.value || e.getAttribute('aria-label') || '').trim().toLowerCase()).includes(target));
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

await send("Runtime.enable");
await send("Page.enable");

const deadline = Date.now() + timeoutSeconds * 1000;
let lastAction = "started";
let lastManualNoticeAt = 0;
let submittedGithubCredentials = false;

while (Date.now() < deadline) {
  const current = await state();
  const text = current.text || "";
  const lower = text.toLowerCase();
  const url = current.url || "";
  const buttons = current.buttons || [];

  if (
    lower.includes("device authorized") ||
    lower.includes("authorization complete") ||
    lower.includes("you may close this window")
  ) {
    console.log("Browser helper: device authorized");
    ws.close();
    process.exit(0);
  }

  if (lower.includes("something went wrong") && buttons.includes("Restart")) {
    await clickText("Restart");
    lastAction = "clicked Restart after Kiro error";
    console.log("Browser helper: clicked Restart after Kiro error");
    await sleep(2500);
    continue;
  }

  if (lower.includes("authorization requested")) {
    await clickText("Accept");
    await sleep(300);
    const approved = (await clickText("Approve")) || (await clickTextContaining("approve"));
    lastAction = `clicked Kiro approval=${approved}`;
    console.log("Browser helper: clicked Kiro approval");
    await sleep(1200);
    continue;
  }

  if (!url.includes("github.com") && buttons.includes("Continue")) {
    await clickText("Continue");
    lastAction = "clicked Kiro Continue";
    console.log("Browser helper: clicked Kiro Continue");
    await sleep(2000);
    continue;
  }

  const githubLoginPage =
    url.includes("github.com") &&
    (url.includes("/login") ||
      lower.includes("sign in to github") ||
      lower.includes("username or email address"));
  if (
    !submittedGithubCredentials &&
    githubLogin &&
    githubPassword &&
    githubLoginPage &&
    current.hasLoginInput &&
    current.hasPasswordInput
  ) {
    const loginLength = await setInput(
      '#login_field,input[name="login"],input[name="user_login"],input[type="email"]',
      githubLogin
    );
    const passwordLength = await setInput(
      '#password,input[name="password"],input[type="password"]',
      githubPassword
    );
    await sleep(250);
    const clicked = (await clickText("Sign in")) || (await clickTextContaining("sign in"));
    submittedGithubCredentials = true;
    lastAction = `submitted GitHub credentials login_len=${loginLength} password_len=${passwordLength} clicked=${clicked}`;
    console.log("Browser helper: submitted GitHub credentials");
    await sleep(3500);
    continue;
  }

  if (
    url.includes("github.com") ||
    lower.includes("sign in to github") ||
    lower.includes("two-factor") ||
    lower.includes("two factor") ||
    lower.includes("authentication code") ||
    lower.includes("verify your identity") ||
    lower.includes("authorize")
  ) {
    if (Date.now() - lastManualNoticeAt > 10_000) {
      console.log(
        "Browser helper: GitHub login/2FA/consent detected; complete it manually in the launched browser"
      );
      lastManualNoticeAt = Date.now();
    }
    lastAction = "waiting for manual GitHub step";
    await sleep(2000);
    continue;
  }

  await sleep(1000);
}

const finalState = await state();
ws.close();
console.error(
  `Browser helper timed out; lastAction=${lastAction}; title=${finalState.title}; url=${finalState.url}; text=${JSON.stringify((finalState.text || "").slice(0, 500))}`
);
process.exit(1);
