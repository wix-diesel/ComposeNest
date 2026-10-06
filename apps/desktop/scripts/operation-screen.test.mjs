import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { mkdir } from "node:fs/promises";
import { chromium } from "playwright";

const origin = "http://127.0.0.1:4185";
const server = spawn(process.execPath, ["node_modules/vite/bin/vite.js", "--host", "127.0.0.1", "--port", "4185", "--strictPort"], { stdio: "pipe" });
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
  const port = { slot: "db", hostIp: "127.0.0.1", hostPort: 15432, containerPort: 5432 };
  let stored = { operation: { id: "op", kind: "create", status: "Accepted", phase: "accepted", startedAt: "2026-10-06 15:00:00" },
    instance: { id: "target", name: "開発用DB", templateId: "custom.database", selectedVersion: "18", storageMethod: "bind", ports: [port], runtimeStatus: "preparing",
      lastOperation: { id: "op" }, connections: [{ slot: "db", label: "Database", port, inputSlots: ["password"] }], inputs: [{ slot: "password", secret: true, value: null }] },
    sequence: 0, completedAt: null };
  const calls = []; let mode = "normal"; const pending = [];
  const waitPending = async () => {
    const deadline = Date.now() + 5000;
    while (!pending.length) {
      if (Date.now() > deadline) throw new Error("Delayed read did not reach the transport");
      await new Promise((resolve) => setTimeout(resolve, 20));
    }
  };
  await page.exposeFunction("operationTransport", async (command, request) => {
    calls.push({ command, request });
    const context = request.context ?? request;
    const response = (result) => ({ apiVersion: 1, requestId: context.requestId, result, error: null });
    if (command === "get_bootstrap") return response({ applicationTitle: "ComposeNest", startedAtUnixSeconds: 1 });
    if (command === "list_instances") return response([]);
    assert.equal(command, "get_operation");
    assert.deepEqual(Object.keys(request).sort(), ["context", "operationId"]);
    assert.equal(request.operationId, "op");
    const reply = response(structuredClone(stored));
    if (mode === "delay") await new Promise((resolve) => pending.push(resolve));
    if (mode === "failure") throw new Error("private-backend-secret");
    if (mode === "wrong_id") reply.requestId = "other";
    if (mode === "wrong_version") reply.apiVersion = 2;
    if (mode === "wrong_operation") reply.result.operation.id = "other";
    if (mode === "wrong_target") reply.result.instance.id = "other";
    if (mode === "bad_sequence") reply.result.sequence = -1;
    if (mode === "null") reply.result = null;
    return reply;
  });
  await page.addInitScript(() => {
    window.callbacks = new Map(); let id = 0;
    window.__TAURI_EVENT_PLUGIN_INTERNALS__ = { unregisterListener: () => {} };
    window.__TAURI_INTERNALS__ = {
      transformCallback: (callback) => { const next = ++id; window.callbacks.set(next, callback); return next; },
      invoke: (command, args) => {
        if (command === "plugin:event|listen") return Promise.resolve(args.handler);
        if (command === "plugin:event|unlisten") { window.callbacks.delete(args.eventId); return Promise.resolve(); }
        return window.operationTransport(command, args.request);
      },
    };
  });
  const route = origin + "/#/operation?operationId=op&instanceId=target";
  const status = (text) => page.getByText(text, { exact: true });
  const ready = () => status("準備完了を確認しました");
  const settled = () => page.waitForFunction(() => document.querySelector(".operation-screen")?.getAttribute("aria-busy") === "false");
  const emit = async (sequence, kind = "progress", operationId = "op", instanceId = "target") => {
    await page.evaluate((payload) => { for (const callback of window.callbacks.values()) callback({ event: "operation-progress", id: 1, payload }); }, { operationId, instanceId, sequence, kind, revision: 1 });
  };
  const update = async (phase, sequence, result = "Executing", kind = "progress") => {
    stored.operation.phase = phase; stored.operation.status = result; stored.sequence = sequence;
    await emit(sequence, kind); await settled();
  };
  await page.goto(route); await page.locator(".operation-screen .badge").filter({ hasText: "受付済み" }).waitFor(); await settled();
  assert.equal(await ready().count(), 0);
  for (const [phase, index] of [["storage", 1], ["image", 2], ["artifact", 3], ["config", 3], ["create", 3], ["inspect_created", 3], ["start", 4], ["ready", 4]]) {
    await update(phase, index);
    await page.locator(".operation-timeline li[aria-current=step]").filter({ hasText: String(index) }).waitFor();
    assert.equal(await page.locator(".operation-check.done").count(), index - 1);
    assert.equal(await ready().count(), 0, phase);
  }
  const reads = calls.length; await page.waitForTimeout(2200); await settled();
  assert.ok(calls.length > reads, "periodic lookup recovers a missing notification");
  assert.equal(await ready().count(), 0, "elapsed time and ready phase are insufficient");
  await update("ready", 10); assert.equal(await ready().count(), 0, "sequence gap restores saved progress");
  await emit(10, "completed"); await settled(); assert.equal(await ready().count(), 0, "completion hint alone is insufficient");
  stored.instance.runtimeStatus = "ready"; stored.completedAt = "2026-10-06 15:00:09";
  await update("ready", 10, "Succeeded", "completed"); await ready().waitFor();
  assert.equal(await page.locator(".operation-check.done").count(), 5);
  assert.equal(await page.getByText("Database:", { exact: false }).count(), 1);
  const count = calls.length; await emit(99, "completed", "other"); await emit(99, "completed", "op", "other");
  assert.equal(calls.length, count, "ignores unrelated operation and target notifications");
  for (const [theme, width] of [["ライト", 1280], ["ダーク", 390]]) {
    await page.setViewportSize({ width, height: 900 }); await page.getByRole("button", { name: theme, exact: true }).click();
    assert.equal(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth), true);
    await mkdir("test-results", { recursive: true }); await page.screenshot({ path: `test-results/operation-${width}.png`, fullPage: true });
  }
  for (const invalid of ["failure", "wrong_id", "wrong_version", "wrong_operation", "wrong_target", "bad_sequence", "null"]) {
    mode = invalid; await page.getByRole("button", { name: "再照会", exact: true }).click();
    await page.getByRole("alert").waitFor(); await settled(); assert.equal(await ready().count(), 0);
    mode = "normal"; await page.getByRole("button", { name: "再照会", exact: true }).click(); await ready().waitFor(); await settled();
  }
  for (const result of ["Failed", "OutcomeUnknown", "AwaitingDecision", "Abandoned"]) {
    await update("ready", 10, result, "failed"); await status(result === "Failed" ? "失敗" : result === "OutcomeUnknown" ? "結果不明" : result === "AwaitingDecision" ? "判断待ち" : "解決済み").first().waitFor();
    assert.equal(await ready().count(), 0);
  }
  stored.operation.kind = "clone"; await update("ready", 10, "Succeeded", "completed"); await ready().waitFor();
  stored.instance.lastOperation.id = "newer"; await emit(10, "completed"); await settled();
  assert.equal(await ready().count(), 0, "historical success does not claim current availability");
  stored.instance.lastOperation.id = "op"; stored.instance.runtimeStatus = "unknown"; await emit(10); await settled();
  assert.equal(await ready().count(), 0, "stale runtime observation does not claim readiness");
  await update("image", 1); mode = "delay"; await emit(2);
  await page.waitForFunction(() => document.querySelector(".operation-screen")?.getAttribute("aria-busy") === "true");
  await waitPending();
  stored.instance.runtimeStatus = "ready"; stored.operation.status = "Succeeded"; stored.operation.phase = "ready";
  await emit(10, "completed"); mode = "normal"; pending.splice(0).forEach((resolve) => resolve()); await ready().waitFor();
  await settled(); await page.getByRole("button", { name: "閉じる", exact: true }).click();
  await page.waitForURL(/#\/instances$/); await page.waitForFunction(() => window.callbacks.size === 0);
  await page.goto(route); await ready().waitFor(); await settled();
  await update("ready", 11, "Executing"); mode = "delay"; await emit(12);
  await page.waitForFunction(() => document.querySelector(".operation-screen")?.getAttribute("aria-busy") === "true");
  await waitPending(); await page.getByRole("button", { name: "閉じる", exact: true }).click(); mode = "normal";
  pending.splice(0).forEach((resolve) => resolve()); await page.waitForURL(/#\/instances$/);
  assert.equal(await page.locator(".operation-screen").count(), 0);
  assert.equal(await page.getByText("private-backend-secret", { exact: false }).count(), 0);
  assert.equal(await page.getByRole("button", { name: /完了の表示例|失敗の表示例|再試行/ }).count(), 0);
  assert.deepEqual(errors, []);
  console.log("Operation: durable stages/results, events/gaps, polling, invalid responses, delayed reads, remount, cleanup and themes passed.");
} finally { await browser?.close(); server.kill(); }
