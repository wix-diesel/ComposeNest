import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { chromium } from "playwright";
import { mkdir } from "node:fs/promises";

// Vite's process is local to this test and is always terminated, including on failure.
const server = spawn(process.execPath, ["node_modules/vite/bin/vite.js", "--host", "127.0.0.1", "--port", "4173", "--strictPort"], { stdio: "pipe" });
let browser;
try {
  let output = "";
  for (const stream of [server.stdout, server.stderr])
    stream.on("data", (data) => { output = (output + data).slice(-4000); });
  const deadline = Date.now() + 20000;
  while (true) {
    if (server.exitCode !== null) throw new Error(`Vite exited: ${output}`);
    try {
      if ((await fetch("http://127.0.0.1:4173", { signal: AbortSignal.timeout(1000) })).ok) break;
    } catch { /* Connection refusal is expected until the local server is ready. */ }
    if (Date.now() > deadline) throw new Error(`Vite startup timed out: ${output}`);
    await new Promise((resolve) => setTimeout(resolve, 100));
  }
  browser = await chromium.launch({ headless: true, ...(process.env.COMPOSENEST_TEST_BROWSER ? { executablePath: process.env.COMPOSENEST_TEST_BROWSER } : {}) });
  const page = await browser.newPage({ viewport: { width: 1280, height: 800 } });
  const errors = [];
  page.on("pageerror", (error) => errors.push(error.message));
  await page.addInitScript(() => {
    window.bootstrapCalls = 0;
    window.__TAURI_INTERNALS__ = { invoke: async (command, { request }) => {
      if (command === "list_templates") return { apiVersion: 1, requestId: request.requestId, error: null, result: { localRoot: "/managed/templates/local", templates: [], results: [] } };
      if (command === "list_instances") return { apiVersion: 1, requestId: request.requestId, error: null, result: [] };
      if (command === "get_instance_detail") return { apiVersion: 1, requestId: request.context.requestId, error: null, result: {
        state: { id: request.instanceId, name: "対象の環境", revision: 1, runtimeStatus: "unknown", observedAt: null, operationId: null, actions: [] },
        instance: { id: request.instanceId, templateId: "custom", selectedVersion: "1", templateVersion: "1", specRevision: 1, appliedSpecRevision: null, connections: [], storage: [], observation: null },
        locations: [], creationStartedAt: null, cloneSourceId: null } };
      if (command === "get_instance_actions") return { apiVersion: 1, requestId: request.context.requestId, error: null,
        result: { id: request.instanceId, name: "対象の環境", revision: 1, runtimeStatus: "unknown", observedAt: null,
          operationId: null, operationStatus: null, operationKind: null, operationPhase: null, actions: [] } };
      window.bootstrapCalls++;
      if (command !== "get_bootstrap" || window.failBootstrap) throw new Error("test failure");
      return { apiVersion: 1, requestId: request.requestId, error: null, result: { applicationTitle: "ComposeNest", startedAtUnixSeconds: 1 } };
    } };
  });
  await page.goto("http://127.0.0.1:4173");
  await page.getByText("アプリに接続済み", { exact: true }).waitFor();
  const calls = await page.evaluate(() => window.bootstrapCalls);
  for (const label of ["テンプレート", "保持データ", "Docker診断", "設定", "環境一覧"]) {
    await page.locator(".sidebar").getByRole("button", { name: label, exact: true }).click();
    await page.waitForFunction((text) => document.querySelector("h1")?.textContent === text, label);
    assert.equal(await page.locator(".nav.active").getAttribute("aria-label"), label);
    assert.equal(await page.locator(".crumb strong").textContent(), label);
  }
  assert.equal(await page.evaluate(() => window.bootstrapCalls), calls, "navigation must not reload/bootstrap again");
  await page.goBack();
  await page.waitForFunction(() => document.querySelector("h1")?.textContent === "設定");
  for (const [path, parameter, value, heading, expected] of [
    ["instance-create", "templateId", "template", "環境を作成", "環境一覧"],
    ["instance-detail", "instanceId", "target", "対象の環境", "環境一覧"],
    ["instance-clone", "instanceId", "target", "設定を複製", "対象の環境"],
    ["instance-edit", "instanceId", "target", "環境を編集", "対象の環境"],
    ["operation", "operationId", "operation", "処理状況", "対象の環境"],
  ]) {
    await page.evaluate((hash) => { location.hash = hash; }, `#/${path}?${parameter}=${value}${path === "operation" ? "&instanceId=target" : ""}`);
    await page.getByRole("heading", { level: 1, name: heading, exact: true }).waitFor();
    await page.getByRole("button", { name: "戻る", exact: true }).click();
    await page.waitForFunction((text) => document.querySelector("h1")?.textContent === text, expected);
    if (expected === "対象の環境") assert.match(page.url(), /instanceId=target/);
  }
  const opener = page.getByRole("button", { name: "この画面について" });
  await opener.click();
  const dialog = page.getByRole("dialog");
  await dialog.waitFor();
  assert.equal(await page.evaluate(() => document.activeElement?.textContent), "閉じる");
  await page.keyboard.press("Tab");
  assert.equal(await page.evaluate(() => !!document.activeElement?.closest("dialog")), true);
  await page.keyboard.press("Shift+Tab");
  assert.equal(await page.evaluate(() => !!document.activeElement?.closest("dialog")), true);
  await page.keyboard.press("Escape");
  await dialog.waitFor({ state: "detached" });
  assert.equal(await opener.evaluate((element) => element === document.activeElement), true);
  await opener.click();
  await dialog.getByRole("button", { name: "閉じる", exact: true }).click();
  assert.equal(await opener.evaluate((element) => element === document.activeElement), true);
  await mkdir("test-results", { recursive: true });
  for (const width of [1280, 800, 390]) {
    await page.setViewportSize({ width, height: 800 });
    assert.equal(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth), true, `overflow at ${width}`);
    assert.equal(await page.locator(".sidebar .nav").count(), 5);
    assert.equal(Math.round((await page.locator(".sidebar").boundingBox()).width), width > 800 ? 224 : 70);
    await page.screenshot({ path: `test-results/shell-${width}.png`, fullPage: true });
  }
  await page.addInitScript(() => { window.failBootstrap = true; });
  await page.reload();
  await page.getByRole("alert").waitFor();
  assert.equal(await page.getByText("アプリに接続済み", { exact: true }).count(), 0);
  await page.evaluate(() => { window.failBootstrap = false; });
  await page.getByRole("button", { name: "再確認", exact: true }).click();
  await page.getByText("アプリに接続済み", { exact: true }).waitFor();
  assert.equal(await page.getByRole("alert").count(), 0);
  assert.deepEqual(errors, []);
  console.log("Shell: ten destinations, history, IDs, modal keyboard/focus, responsive widths and bootstrap failure/retry passed.");
} finally {
  await browser?.close();
  server.kill();
}
