import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { mkdir } from "node:fs/promises";
import { chromium } from "playwright";
const origin = "http://127.0.0.1:4179";
const server = spawn(process.execPath, ["node_modules/vite/bin/vite.js", "--host", "127.0.0.1", "--port", "4179", "--strictPort"], { stdio: "pipe" });
let browser;
try {
  const deadline = Date.now() + 20000;
  while (true) {
    if (server.exitCode !== null) throw new Error("Vite exited");
    try { if ((await fetch(origin, { signal: AbortSignal.timeout(1000) })).ok) break; } catch { /* Local startup. */ }
    if (Date.now() > deadline) throw new Error("Vite startup timed out");
    await new Promise((resolve) => setTimeout(resolve, 100));
  }
  browser = await chromium.launch({ headless: true, ...(process.env.COMPOSENEST_TEST_BROWSER ? { executablePath: process.env.COMPOSENEST_TEST_BROWSER } : {}) });
  const page = await browser.newPage({ viewport: { width: 1280, height: 900 } });
  const errors = []; page.on("pageerror", (error) => errors.push(error.message));
  await page.addInitScript(() => {
    window.mode = "delay"; window.calls = [];
    window.view = { localRoot: "C:\\ProgramData\\ComposeNest\\templates\\local", templates: [
      { revisionId: "example.pg:1.0.0:exact&hash", templateId: "example.pg", name: "PostgreSQL", description: "<b>Plain text</b>", templateVersion: "1.0.0", versions: ["18", "17"], storageMethods: ["bind", "volume"], origin: "bundled", loaded: true },
      { revisionId: "example.cache:1.0.0:hash", templateId: "example.cache", name: "Custom cache", description: "Local", templateVersion: "1.0.0", versions: ["8.2"], storageMethods: [], origin: "local", loaded: false },
    ], results: [
      { package: "postgresql", origin: "bundled", revisionId: "example.pg:1.0.0:exact&hash", error: null, warnings: ["Version 17: versions/17.yaml: $.inputs.note: この値はコンテナ設定に反映されません。serviceから参照するか、不要な入力を削除してください。"] },
      { package: "cache", origin: "local", revisionId: null, error: "Version 8.2: versions/8.2.yaml (4行・12列): $.service.unknown: 未知の項目です。項目名の綴りを修正してください。", warnings: ["unlisted version file was ignored: versions/unused.yaml"] },
    ] };
    window.__TAURI_INTERNALS__ = { invoke: async (command, { request }) => {
      window.calls.push({ command, request });
      const response = (result) => ({ apiVersion: window.mode === "version" ? 2 : 1, requestId: window.mode === "identity" ? "wrong" : (request.context ?? request).requestId, error: null, result });
      if (command === "get_bootstrap") return response({ applicationTitle: "ComposeNest", startedAtUnixSeconds: 1 });
      if (command === "prepare_create") throw new Error("Docker unavailable in catalog test");
      if (!["list_templates", "reload_templates"].includes(command)) throw new Error(`Unexpected ${command}`);
      if (window.mode === "delay") await new Promise((resolve) => { window.release = resolve; });
      if (window.mode === "failure") throw new Error("Unavailable");
      return response(window.mode === "malformed" ? {} : structuredClone(window.view));
    } };
  });
  await page.goto(`${origin}/#/templates`);
  const reload = page.getByRole("button", { name: "再読込み", exact: true });
  await page.getByText("テンプレートを読込み中…", { exact: true }).waitFor();
  assert.equal(await reload.isDisabled(), true);
  await page.waitForFunction(() => window.release);
  await page.evaluate(() => { window.mode = "normal"; window.release(); });
  await page.getByRole("heading", { name: "PostgreSQL", exact: true }).waitFor();
  assert.equal(await page.locator(".template-card").count(), 2);
  assert.equal(await page.getByText("18 / 17", { exact: true }).count(), 1);
  assert.equal(await page.getByText("<b>Plain text</b>", { exact: true }).count(), 1);
  assert.equal(await page.locator(".template-card b").count(), 0);
  assert.deepEqual(await page.locator(".template-results dd").allTextContents(), ["1 件", "1 件"]);
  await page.getByText("既存登録版（今回の読込みでは未登録）", { exact: true }).waitFor();
  await page.getByText("Version 8.2: versions/8.2.yaml (4行・12列): $.service.unknown: 未知の項目です。項目名の綴りを修正してください。", { exact: true }).waitFor();
  await page.getByText("Version 17: versions/17.yaml: $.inputs.note: この値はコンテナ設定に反映されません。serviceから参照するか、不要な入力を削除してください。", { exact: true }).waitFor();
  await page.getByText("C:\\ProgramData\\ComposeNest\\templates\\local", { exact: true }).waitFor();
  assert.equal(await page.getByText("実機検証状態：未確認", { exact: true }).count(), 2);
  for (const theme of ["ダーク", "ライト"]) {
    await page.locator(".topbar").getByRole("button", { name: theme, exact: true }).click();
    for (const width of [1280, 800, 390]) {
      await page.setViewportSize({ width, height: 900 });
      assert.equal(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth), true);
      await mkdir("test-results", { recursive: true });
      await page.screenshot({ path: `test-results/templates-${theme === "ダーク" ? "dark" : "light"}-${width}.png`, fullPage: true });
    }
  }
  await reload.click(); await page.getByRole("heading", { name: "PostgreSQL", exact: true }).waitFor();
  assert.equal(await page.evaluate(() => window.calls.filter((call) => call.command === "reload_templates").length), 1);
  assert.equal(await page.evaluate(() => window.calls.some((call) => !["get_bootstrap", "list_templates", "reload_templates"].includes(call.command))), false);
  await page.getByRole("button", { name: "PostgreSQL 1.0.0のテンプレートを使う", exact: true }).click();
  await page.getByRole("heading", { level: 1, name: "環境を作成", exact: true }).waitFor();
  await page.waitForFunction(() => window.calls.some((call) => call.command === "prepare_create"));
  assert.equal(await page.evaluate(() => window.calls.find((call) => call.command === "prepare_create").request.templateRevisionId), "example.pg:1.0.0:exact&hash");
  await page.getByRole("button", { name: "戻る", exact: true }).click();
  await page.getByRole("heading", { name: "PostgreSQL", exact: true }).waitFor();
  for (const mode of ["failure", "version", "identity", "malformed"]) {
    await page.evaluate((mode) => { window.mode = mode; }, mode); await reload.click();
    await page.getByRole("alert").filter({ hasText: "テンプレート一覧の取得に失敗" }).waitFor();
    assert.equal(await page.locator(".template-card").count(), 0);
  }
  await page.evaluate(() => { window.mode = "normal"; window.view.templates = []; window.view.results = []; }); await reload.click();
  await page.getByText("利用できるテンプレートはありません。ローカル定義を追加して再読込みしてください。", { exact: true }).waitFor();
  assert.deepEqual(await page.locator(".template-results dd").allTextContents(), ["0 件", "0 件"]);
  assert.deepEqual(errors, []);
  console.log("Template catalog: package cards, origins, actual outcomes, retained revisions, safe text, create identity, reload isolation, empty/loading/errors, responsive themes passed.");
} finally { await browser?.close(); server.kill(); }
