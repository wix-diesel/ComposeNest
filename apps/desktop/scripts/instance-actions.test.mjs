import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { mkdir } from "node:fs/promises";
import { chromium } from "playwright";

const server = spawn(process.execPath, ["node_modules/vite/bin/vite.js", "--host", "127.0.0.1", "--port", "4176", "--strictPort"], { stdio: "pipe" });
let browser;
try {
  const deadline = Date.now() + 20000;
  while (true) {
    if (server.exitCode !== null) throw new Error("Vite exited");
    try { if ((await fetch("http://127.0.0.1:4176", { signal: AbortSignal.timeout(1000) })).ok) break; } catch { /* Local startup. */ }
    if (Date.now() > deadline) throw new Error("Vite startup timed out");
    await new Promise((resolve) => setTimeout(resolve, 100));
  }
  browser = await chromium.launch({ headless: true, ...(process.env.COMPOSENEST_TEST_BROWSER ? { executablePath: process.env.COMPOSENEST_TEST_BROWSER } : {}) });
  const page = await browser.newPage({ viewport: { width: 1280, height: 900 } });
  const errors = []; page.on("pageerror", (error) => errors.push(error.message));
  await page.addInitScript(() => {
    window.calls = []; window.mode = "normal";
    window.view = { id: "target", name: "開発用データベース", revision: 3, runtimeStatus: "stopped", observedAt: "2026-10-05 08:00:00", operationId: null, operationStatus: null, operationKind: null, operationPhase: null, actions: ["rename", "start", "stop", "restart"] };
    window.__TAURI_INTERNALS__ = { invoke: async (command, { request }) => {
      window.calls.push({ command, request: structuredClone(request) });
      const response = (result, error = null) => ({ apiVersion: 1, requestId: (request.context ?? request).requestId, result, error });
      if (command === "get_bootstrap") return response({ applicationTitle: "ComposeNest", startedAtUnixSeconds: 1 });
      if (command === "get_instance_actions") { if (window.mode === "read_failure") throw new Error("Disconnected"); return response(structuredClone(window.view)); }
      if (command === "change_instance") {
        if (window.mode === "delay") await new Promise((resolve) => { window.finishChange = resolve; });
        if (!window.view.operationId) Object.assign(window.view, { operationId: "accepted-op", operationStatus: "Accepted", operationKind: request.action, operationPhase: "inspect", actions: [] });
        if (window.mode === "lost") throw new Error("Disconnected after acceptance");
        return response(structuredClone(window.view));
      }
      if (command === "rename_instance") {
        if (window.mode === "duplicate") return response(null, { code: "NAME_OR_REQUEST_CONFLICT" });
        if (window.mode === "stale") return response(null, { code: "INSTANCE_STALE" });
        if (window.mode === "delay") await new Promise((resolve) => { window.finishChange = resolve; });
        window.view.name = request.name.trim(); window.view.revision++;
        if (window.mode === "lost") throw new Error("Disconnected after rename");
        return response(structuredClone(window.view));
      }
      throw new Error(`Unexpected command ${command}`);
    } };
  });
  async function open(edit = false) {
    await page.goto("about:blank");
    await page.goto(`http://127.0.0.1:4176/#/instance-${edit ? "edit" : "detail"}?instanceId=target`);
    await page.locator(".instance-actions h2").filter({ hasText: "開発用データベース" }).waitFor();
  }
  const calls = (command) => page.evaluate((command) => window.calls.filter((call) => call.command === command), command);
  async function rename() {
    await page.locator("#edit-name").fill("変更した名前");
    await page.getByRole("button", { name: "変更内容を確認", exact: true }).click();
    await page.getByRole("button", { name: "名前変更を適用", exact: true }).click();
  }
  await open();
  await page.evaluate(() => { window.mode = "delay"; });
  await page.getByRole("button", { name: "起動", exact: true }).evaluate((button) => { button.click(); button.click(); });
  assert.equal((await calls("change_instance")).length, 1);
  assert.equal(await page.getByRole("button", { name: "編集", exact: true }).isDisabled(), true);
  assert.equal(await page.locator(".badge").textContent(), "停止中");
  await page.evaluate(() => window.finishChange());
  await page.getByText("起動 · 受付済み", { exact: true }).waitFor();
  assert.equal(await page.locator(".badge").textContent(), "停止中");
  await page.evaluate(() => { Object.assign(window.view, { runtimeStatus: "ready", operationStatus: "Succeeded", operationPhase: "running", actions: ["rename", "stop", "restart"] }); });
  await page.getByText("起動 · 完了", { exact: true }).waitFor();
  assert.equal(await page.locator(".badge").textContent(), "利用可能");
  await page.getByRole("button", { name: "編集", exact: true }).click();
  assert.equal(await page.locator("#edit-name").isDisabled(), false, "running name-only changes are allowed");
  await page.evaluate(() => { window.mode = "normal"; });
  await rename(); await page.getByText("環境名を変更しました。", { exact: true }).waitFor();
  assert.equal((await calls("rename_instance"))[0].request.expectedRevision, 3);
  assert.equal(await page.locator(".badge").textContent(), "利用可能");

  for (const [mode, text] of [["duplicate", "同じ環境名が使用されています。別の名前を入力してください。"], ["stale", "環境が更新されています。現在の状態を再確認し、変更内容を確認し直してください。"]]) {
    await open(true); await page.evaluate((mode) => { window.mode = mode; }, mode); await rename();
    await page.getByText(text, { exact: true }).waitFor();
    assert.equal(await page.locator("#edit-name").isDisabled(), true);
    await page.getByRole("button", { name: "現在の状態を再確認", exact: true }).click();
    assert.equal(await page.locator("#edit-name").inputValue(), "開発用データベース");
  }
  for (const action of ["停止", "再起動"]) {
    await open();
    await page.evaluate(() => { Object.assign(window.view, { runtimeStatus: "ready", actions: ["rename", "stop", "restart"] }); });
    await page.waitForFunction(() => document.querySelector(".badge").textContent === "利用可能");
    await page.getByRole("button", { name: action, exact: true }).click();
    await page.getByText(`${action} · 受付済み`, { exact: true }).waitFor();
    assert.equal((await calls("change_instance"))[0].request.action, action === "停止" ? "stop" : "restart");
    assert.equal(await page.locator(".badge").textContent(), "利用可能");
  }
  await open(true);
  await page.locator("#edit-name").fill("変更案");
  await page.getByRole("button", { name: "変更内容を確認", exact: true }).click();
  await page.evaluate(() => { window.view.revision++; });
  await page.getByText("表示していた版が古くなりました。現在の状態を再確認してください。", { exact: true }).waitFor();
  assert.equal(await page.getByRole("button", { name: "名前変更を適用", exact: true }).isDisabled(), true);
  await page.keyboard.press("Escape");
  assert.equal((await calls("rename_instance")).length, 0);
  await open();
  await page.evaluate(() => { Object.assign(window.view, { runtimeStatus: "absent", actions: ["rename", "start"] }); });
  await page.getByText(/コンテナが不在です/).waitFor();
  assert.equal(await page.getByRole("button", { name: "再起動", exact: true }).isDisabled(), true);
  assert.equal(await page.getByRole("button", { name: "起動", exact: true }).isDisabled(), false);
  for (const status of ["Failed", "OutcomeUnknown", "AwaitingDecision"]) {
    await page.evaluate((status) => { Object.assign(window.view, { operationId: "failed-op", operationStatus: status, actions: [] }); }, status);
    await page.getByText("未解決の処理があるため、別の変更は実行できません。", { exact: true }).waitFor();
    assert.equal(await page.getByRole("button", { name: "起動", exact: true }).count(), 0);
    assert.equal(await page.getByRole("button", { name: "停止", exact: true }).isDisabled(), true);
  }
  await open(); await page.evaluate(() => { window.mode = "lost"; });
  await page.getByRole("button", { name: "起動", exact: true }).click();
  await page.getByText("受付結果の確認が必要です。別の変更は実行できません。", { exact: true }).waitFor();
  await page.evaluate(() => { location.hash = "#/instances"; });
  await page.getByRole("heading", { level: 1, name: "環境一覧", exact: true }).waitFor();
  await page.evaluate(() => { window.mode = "normal"; location.hash = "#/instance-detail?instanceId=target"; });
  await page.getByRole("button", { name: "受付を再確認", exact: true }).click();
  await page.getByText("起動 · 受付済み", { exact: true }).waitFor();
  const repeated = await calls("change_instance"); assert.equal(repeated.length, 2); assert.deepEqual(repeated[0].request, repeated[1].request);

  await open(true); await page.evaluate(() => { window.mode = "lost"; }); await rename();
  await page.getByRole("button", { name: "受付を再確認", exact: true }).waitFor();
  await page.evaluate(() => { window.mode = "normal"; });
  await page.getByRole("button", { name: "受付を再確認", exact: true }).click();
  await page.locator("#edit-name").filter({ visible: true }).waitFor();
  await page.waitForFunction(() => document.querySelector("#edit-name").value === "変更した名前" && !document.querySelector("#edit-name").disabled);
  assert.equal((await calls("rename_instance")).length, 1, "uncertain rename is read back without resubmission");
  await mkdir("test-results", { recursive: true });
  for (const [theme, width] of [["light", 1280], ["dark", 390]]) {
    await page.setViewportSize({ width, height: 900 }); await page.evaluate((theme) => { document.documentElement.dataset.theme = theme; }, theme);
    assert.equal(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth), true);
    await page.screenshot({ path: `test-results/instance-edit-${theme}-${width}.png`, fullPage: true });
  }
  await open(); await page.evaluate(() => { window.mode = "read_failure"; });
  await page.getByRole("alert").waitFor();
  assert.equal(await page.getByRole("button", { name: "編集", exact: true }).isDisabled(), true);
  assert.deepEqual(errors, []);
  console.log("Instance actions: acceptance, double clicks, persisted completion, absent restart, unresolved failure, name conflicts/version, uncertain requests/navigation, running rename and responsive layouts passed.");
} finally { await browser?.close(); server.kill(); }
