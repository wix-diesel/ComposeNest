import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { mkdir } from "node:fs/promises";
import { chromium } from "playwright";

const server = spawn(process.execPath, ["node_modules/vite/bin/vite.js", "--host", "127.0.0.1", "--port", "4182", "--strictPort"], { stdio: "pipe" });
let browser;
try {
  const deadline = Date.now() + 20000;
  while (true) {
    if (server.exitCode !== null) throw new Error("Vite exited");
    try { if ((await fetch("http://127.0.0.1:4182", { signal: AbortSignal.timeout(1000) })).ok) break; } catch { /* Local startup. */ }
    if (Date.now() > deadline) throw new Error("Vite startup timed out");
    await new Promise((resolve) => setTimeout(resolve, 100));
  }
  browser = await chromium.launch({ headless: true, ...(process.env.COMPOSENEST_TEST_BROWSER ? { executablePath: process.env.COMPOSENEST_TEST_BROWSER } : {}) });
  const page = await browser.newPage({ viewport: { width: 1280, height: 900 } });
  const errors = []; page.on("pageerror", (error) => errors.push(error.message));
  await page.addInitScript(() => {
    window.calls = []; window.mode = "normal";
    window.report = { observedAt: 1791270000, checks: ["cli", "compose", "engine", "linux", "platform", "endpoint", "root"].map((name) => ({ name, status: "ready", version: name === "compose" ? "5.6.2" : ["cli", "engine"].includes(name) ? "30.1.4" : null })),
      endpoint: "unix:///actual/docker.sock", contextName: "same-context", platform: "linux/arm64", engineId: "actual-engine", registeredEngineId: "actual-engine", targetStatus: "verified", managementRoot: "/actual/management" };
    window.__TAURI_INTERNALS__ = { invoke: async (command, { request }) => {
      window.calls.push(command);
      const response = (result) => ({ apiVersion: 1, requestId: request.requestId, result, error: null });
      if (command === "get_bootstrap") return response({ applicationTitle: "ComposeNest", startedAtUnixSeconds: 1 });
      if (command === "list_instances") return response([{ id: "saved", name: "保存済み環境", templateId: "custom", selectedVersion: "1", runtimeStatus: "unknown", observation: null, ports: [], storageMethod: "bind", specRevision: 1, appliedSpecRevision: null, needsAttention: false }]);
      if (command !== "diagnose_runtime") throw new Error(`Unexpected command ${command}`);
      if (window.mode === "delay") await new Promise((resolve) => { window.finishDiagnosis = resolve; });
      if (window.mode === "failure") throw new Error("sensitive CLI error");
      const reply = response(structuredClone(window.report));
      if (window.mode === "wrong_id") reply.requestId = "unrelated";
      if (window.mode === "wrong_version") reply.apiVersion = 2;
      return reply;
    } };
  });
  await page.goto("http://127.0.0.1:4182");
  await page.getByText("アプリを閉じても環境は停止しません", { exact: true }).waitFor();
  await page.locator(".sidebar").getByRole("button", { name: "Docker診断", exact: true }).click();
  const refresh = page.getByRole("button", { name: "再確認", exact: true });
  await page.getByText("Dockerを利用できます", { exact: true }).waitFor();
  for (const text of ["30.1.4", "5.6.2", "linux/arm64", "same-context", "unix:///actual/docker.sock", "Engine ID一致", "/actual/management"]) assert.ok(await page.getByText(text, { exact: false }).count(), text);
  assert.equal(await page.locator(".diagnostic-row").count(), 7);
  for (const [name, status, label] of [["cli", "missing", "未導入"], ["cli", "permission_denied", "権限不足"], ["compose", "unsupported", "非対応"], ["engine", "unavailable", "確認不能"], ["engine", "permission_denied", "権限不足"], ["root", "permission_denied", "権限不足"], ["engine", "changed", "対象不一致"], ["linux", "unsupported", "非対応"], ["platform", "unsupported", "非対応"]]) {
    await page.evaluate(({ name, status }) => { window.report.checks.forEach((check) => { check.status = check.name === name ? status : "ready"; }); if (name === "engine" && status === "changed") { window.report.engineId = "another-engine"; window.report.targetStatus = "changed"; } }, { name, status });
    await refresh.click();
    await page.locator(".diagnostic-badge").filter({ hasText: label }).waitFor();
    assert.equal(await page.getByText("Dockerを利用できます", { exact: true }).count(), 0);
    assert.equal(await page.getByText("Engine ID一致", { exact: true }).count(), status === "changed" || name === "linux" || name === "platform" ? 0 : 1);
  }
  await page.evaluate(() => {
    window.report.targetStatus = "unverified"; window.report.registeredEngineId = null;
    window.report.checks.forEach((check) => { check.status = check.name === "root" ? "unavailable" : "ready"; });
  });
  await refresh.click();
  await page.locator(".diagnostic-row").filter({ hasText: "管理データの保存先" }).locator(".diagnostic-badge").filter({ hasText: "確認不能" }).waitFor();
  assert.equal(await page.locator(".diagnostic-badge.ready").count(), 6);
  assert.equal(await page.getByText("Dockerを利用できます", { exact: true }).count(), 0);
  assert.equal(await page.getByText("Engine ID一致", { exact: true }).count(), 0);
  assert.equal(await page.getByText("Engine停止・接続不能。起動と接続先を確認してください。", { exact: true }).count(), 0);
  await page.getByRole("button", { name: "保存済みの環境を見る", exact: true }).click();
  await page.getByText("保存済み環境", { exact: true }).waitFor();
  await page.locator(".sidebar").getByRole("button", { name: "Docker診断", exact: true }).click();
  await refresh.waitFor();
  for (const mode of ["failure", "wrong_id", "wrong_version"]) {
    await page.evaluate((mode) => { window.mode = mode; }, mode); await refresh.click();
    await page.getByRole("alert").waitFor();
    assert.equal(await page.getByText("30.1.4", { exact: false }).count(), 0);
    assert.equal(await page.getByText("sensitive CLI error", { exact: false }).count(), 0);
    assert.equal(await page.getByText("Engine ID一致", { exact: true }).count(), 0);
  }
  await page.evaluate(() => { window.mode = "delay"; }); await refresh.click();
  await page.waitForFunction(() => typeof window.finishDiagnosis === "function");
  assert.equal(await page.getByRole("button", { name: "確認中…", exact: true }).isDisabled(), true);
  assert.equal(await page.getByText("Engine ID一致", { exact: true }).count(), 0);
  await page.evaluate(() => { window.mode = "normal"; window.finishDiagnosis(); }); await refresh.waitFor();
  await page.evaluate(() => { Object.assign(window.report, { engineId: null, platform: null, targetStatus: "unverified" }); window.report.checks.forEach((check) => { check.status = "unavailable"; check.version = null; }); });
  await refresh.click(); await page.getByText("Engine停止・接続不能。起動と接続先を確認してください。", { exact: true }).waitFor();
  for (const [theme, width] of [["light", 1280], ["dark", 390]]) {
    await page.setViewportSize({ width, height: 900 }); await page.evaluate((theme) => { document.documentElement.dataset.theme = theme; }, theme);
    assert.equal(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth), true);
    await mkdir("test-results", { recursive: true }); await page.screenshot({ path: `test-results/diagnostics-${theme}-${width}.png`, fullPage: true });
  }
  assert.ok((await page.evaluate(() => window.calls)).every((command) => ["get_bootstrap", "list_instances", "diagnose_runtime"].includes(command)));
  assert.deepEqual(errors, []);
  console.log("Diagnostics: actual metadata, distinct failures, fixed identity, offline saved views, correlation/retry, startup guidance and responsive themes passed.");
} finally { await browser?.close(); server.kill(); }
