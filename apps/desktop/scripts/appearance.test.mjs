import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { mkdir, readFile } from "node:fs/promises";
import { chromium } from "playwright";

const origin = "http://127.0.0.1:4177";
const csp = JSON.parse(await readFile("src-tauri/tauri.conf.json", "utf8")).app.security.csp;
// Test the built entry point, including the blocking script under the real Tauri CSP.
const server = spawn(process.execPath, ["node_modules/vite/bin/vite.js", "preview", "--host", "127.0.0.1", "--port", "4177", "--strictPort"], { stdio: "pipe" });
let browser;
const errors = [];
try {
  const deadline = Date.now() + 20000;
  while (true) {
    if (server.exitCode !== null) throw new Error("Vite exited");
    try { if ((await fetch(origin, { signal: AbortSignal.timeout(1000) })).ok) break; } catch { /* Local server startup. */ }
    if (Date.now() > deadline) throw new Error("Vite startup timed out");
    await new Promise((resolve) => setTimeout(resolve, 100));
  }
  browser = await chromium.launch({ headless: true, ...(process.env.COMPOSENEST_TEST_BROWSER ? { executablePath: process.env.COMPOSENEST_TEST_BROWSER } : {}) });
  async function context(mode = "normal", seed = {}) {
    const context = await browser.newContext({ viewport: { width: 1280, height: 900 } });
    context.on("page", (page) => page.on("pageerror", (error) => errors.push(error.message)));
    await context.route(origin + "/**", async (route) => {
      if (route.request().resourceType() !== "document") return route.continue();
      const response = await route.fetch();
      await route.fulfill({ response, headers: { ...response.headers(), "content-security-policy": csp } });
    });
    await context.addInitScript(({ mode, seed }) => {
      if (location.origin !== "http://127.0.0.1:4177") return;
      if (!sessionStorage.getItem("seeded")) {
        if (localStorage.length === 0) for (const [key, value] of Object.entries(seed)) localStorage.setItem(key, value);
        sessionStorage.setItem("seeded", "true");
      }
      window.preferenceWrites = [];
      const write = Storage.prototype.setItem;
      Storage.prototype.setItem = function (key, value) {
        window.preferenceWrites.push([key, value]);
        if (mode === "write" || mode === "both" || (mode === "theme" && key === "composenest-theme" && !window.allowThemeWrite)) throw new DOMException("Storage unavailable");
        return write.call(this, key, value);
      };
      if (mode === "read" || mode === "both") Storage.prototype.getItem = () => { throw new DOMException("Storage unavailable"); };
      if (mode === "getter") Object.defineProperty(window, "localStorage", { get() { throw new DOMException("Storage denied"); } });
      let plan = {
        planId: "private-plan", planRevision: 1, displayName: "検証環境", templateRevisionId: "registered",
        templateOrigin: "local", version: "1", versions: ["1"], form: {}, storageMethod: "bind",
        inputs: [{ key: "password", definition: {}, value: null, hasSecret: false }], ports: {}, storageSlots: [], concerns: [],
        templateForm: { name: "検証", templateVersion: "1", description: "", ports: [], storage: [], connections: [],
          inputs: [{ key: "password", label: "Password", inputType: "secret", required: false, description: null, validation: {}, options: [], canGenerate: false }] },
      };
      window.__TAURI_INTERNALS__ = { invoke: async (command, { request }) => {
        const context = request.context ?? request;
        let result = null;
        if (command === "get_bootstrap") result = { applicationTitle: "ComposeNest", startedAtUnixSeconds: 1 };
        else if (command === "get_settings") result = { storageMethod: "bind", managementRoot: "/managed" };
        else if (command === "list_instances") result = [];
        else if (command === "prepare_create" || command === "view_create_plan") result = plan;
        else if (command === "update_create_plan") {
          plan = { ...plan, planRevision: plan.planRevision + 1 };
          result = plan;
        } else if (command !== "discard_create_plan") throw new Error(`Unexpected command: ${command}`);
        return { apiVersion: 1, requestId: context.requestId, error: null, result: structuredClone(result) };
      } };
    }, { mode, seed });
    return context;
  }
  const darkSeed = { "composenest-theme": "dark", "composenest-list-view": "grid" };
  const normal = await context("normal", darkSeed);
  const page = await normal.newPage();
  let releaseEntry;
  const entryBlocked = new Promise((resolve) => { releaseEntry = resolve; });
  await page.route(/\/assets\/.*\.js$/, async (route) => { await entryBlocked; await route.continue(); });
  const navigation = page.goto(origin);
  await page.waitForFunction(() => document.querySelector("#root") !== null);
  assert.equal(await page.evaluate(() => document.documentElement.dataset.theme), "dark", "theme restored before React loads");
  assert.equal(await page.locator("#root").textContent(), "");
  releaseEntry(); await navigation;
  await page.getByText("アプリに接続済み", { exact: true }).waitFor();
  const theme = page.locator(".topbar").getByRole("group", { name: "テーマ", exact: true });
  assert.equal(await theme.getByRole("button", { name: "ダーク" }).getAttribute("aria-pressed"), "true");
  assert.equal(await page.getByRole("button", { name: "DataGrid", exact: true }).getAttribute("aria-pressed"), "true");
  assert.equal(await page.locator("[data-view]").getAttribute("data-view"), "grid");
  await page.locator(".sidebar").getByRole("button", { name: "設定", exact: true }).click();
  await page.getByRole("heading", { name: "デザイン", exact: true }).waitFor();
  await page.locator(".display-settings").getByRole("button", { name: "ライト", exact: true }).click();
  assert.equal(await theme.getByRole("button", { name: "ライト" }).getAttribute("aria-pressed"), "true");
  await page.getByRole("button", { name: "カード", exact: true }).click();
  await page.reload();
  await page.getByRole("heading", { name: "デザイン", exact: true }).waitFor();
  assert.equal(await theme.getByRole("button", { name: "ライト" }).getAttribute("aria-pressed"), "true");
  assert.equal(await page.getByRole("button", { name: "カード", exact: true }).getAttribute("aria-pressed"), "true");
  const second = await normal.newPage();
  await second.goto(origin);
  await second.getByText("アプリに接続済み", { exact: true }).waitFor();
  await theme.getByRole("button", { name: "ダーク" }).click();
  await second.waitForFunction(() => document.documentElement.dataset.theme === "dark");
  await page.getByRole("button", { name: "DataGrid", exact: true }).click();
  await second.waitForFunction(() => document.querySelector('[data-view="grid"]') !== null);
  await page.evaluate(() => sessionStorage.setItem("composenest-theme", "light"));
  assert.equal(await page.evaluate(() => document.documentElement.dataset.theme), "dark", "session storage is ignored");
  await page.evaluate(() => localStorage.clear());
  await second.waitForFunction(() => document.documentElement.dataset.theme === "light" && document.querySelector('[data-view="cards"]') !== null);

  await page.goto(origin + "/#/instance-create?templateId=registered");
  await page.locator("#input-password").fill("OnlyEntered-Secret");
  await page.getByRole("button", { name: "作成内容を確認", exact: true }).click();
  await page.getByRole("dialog").waitFor();
  await page.keyboard.press("Escape");
  // Log and Compose screens are separate issues; probe their shared style classes.
  await page.evaluate(() => {
    for (const className of ["log", "compose-preview"]) {
      const pre = document.createElement("pre"); pre.className = className; pre.textContent = "service: database\nready";
      document.querySelector(".workspace").append(pre);
    }
  });
  await mkdir("test-results", { recursive: true });
  for (const value of ["light", "dark"]) {
    await theme.getByRole("button", { name: value === "light" ? "ライト" : "ダーク", exact: true }).click();
    const palette = await page.locator("#input-password").evaluate((input) => {
      const style = getComputedStyle(input); return { text: style.color, background: style.backgroundColor };
    });
    assert.equal(palette.background, value === "dark" ? "rgb(37, 53, 44)" : "rgb(255, 255, 255)");
    assert.equal(palette.text, value === "dark" ? "rgb(225, 233, 228)" : "rgb(37, 54, 47)");
    assert.notEqual(palette.text, palette.background);
    for (const selector of [".log", ".compose-preview"]) {
      assert.equal(await page.locator(selector).evaluate((element) => getComputedStyle(element).backgroundColor), value === "dark" ? "rgb(16, 26, 21)" : "rgb(35, 53, 45)");
    }
    for (const width of [1280, 390]) {
      await page.setViewportSize({ width, height: 900 });
      await page.getByRole("button", { name: "作成内容を確認", exact: true }).click();
      await page.getByRole("dialog").waitFor();
      assert.equal(await page.getByRole("dialog").evaluate((element) => getComputedStyle(element).backgroundColor), value === "dark" ? "rgb(30, 42, 35)" : "rgb(255, 255, 255)");
      assert.equal(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth), true);
      await page.screenshot({ path: `test-results/appearance-${value}-${width}.png`, fullPage: true });
      await page.keyboard.press("Escape");
    }
  }
  assert.deepEqual(await page.evaluate(() => Object.keys(localStorage).sort()), ["composenest-theme"]);
  for (const [key, value] of await page.evaluate(() => window.preferenceWrites)) {
    assert.ok(["composenest-theme", "composenest-list-view"].includes(key));
    assert.ok(["light", "dark", "cards", "grid"].includes(value));
  }
  await normal.close();
  for (const mode of ["read", "write", "both", "getter", "theme"]) {
    const isolated = await context(mode);
    const fallback = await isolated.newPage();
    await fallback.goto(origin);
    await fallback.getByText("アプリに接続済み", { exact: true }).waitFor();
    await fallback.getByRole("button", { name: "ダーク", exact: true }).click();
    await fallback.getByRole("button", { name: "DataGrid", exact: true }).click();
    await fallback.locator(".sidebar").getByRole("button", { name: "設定", exact: true }).click();
    await fallback.getByRole("heading", { name: "デザイン", exact: true }).waitFor();
    assert.equal(await fallback.evaluate(() => document.documentElement.dataset.theme), "dark");
    assert.equal(await fallback.getByRole("button", { name: "DataGrid", exact: true }).getAttribute("aria-pressed"), "true");
    assert.equal(await fallback.getByRole("status").count(), mode === "read" ? 0 : 1);
    if (mode === "theme") {
      await fallback.evaluate(() => { window.allowThemeWrite = true; });
      await fallback.locator(".topbar").getByRole("button", { name: "ライト", exact: true }).click();
      assert.equal(await fallback.getByRole("status").count(), 0, "saving another key must not hide an unsaved theme");
    }
    await isolated.close();
  }
  const invalid = await context("normal", { "composenest-theme": "private-plan", "composenest-list-view": "OnlyEntered-Secret" });
  const defaults = await invalid.newPage();
  await defaults.goto(origin);
  await defaults.getByText("アプリに接続済み", { exact: true }).waitFor();
  assert.equal(await defaults.evaluate(() => document.documentElement.dataset.theme), "light");
  assert.equal(await defaults.getByRole("button", { name: "カード", exact: true }).getAttribute("aria-pressed"), "true");
  await invalid.close();
  assert.deepEqual(errors, []);
  console.log("Appearance: production CSP/pre-React restore, persistence, navigation, storage events/failures, secret isolation, input/dialog/Compose/log palettes and responsive widths passed.");
} finally { await browser?.close(); server.kill(); }
