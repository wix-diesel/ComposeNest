import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { mkdir } from "node:fs/promises";
import { chromium } from "playwright";

const origin = "http://127.0.0.1:4187";
const server = spawn(process.execPath, ["node_modules/vite/bin/vite.js", "--host", "127.0.0.1", "--port", "4187", "--strictPort"], { stdio: "pipe" });
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
    window.calls = []; window.mode = "normal";
    const port = { slot: "db", hostIp: "127.0.0.1", containerPort: 5432, hostPort: 15433 };
    window.progress = { operation: { id: "op", kind: "edit_port", status: "Failed", phase: "ports", startedAt: "2026-10-07 02:00:00" }, lastFailureStatus: "Failed",
      instance: { id: "target", name: "復旧対象DB", templateId: "custom.db", selectedVersion: "18", lifecycle: "managed", ports: [port], storageMethod: "bind", runtimeStatus: "stopped", lastOperation: { id: "op" }, connections: [] }, sequence: 1, completedAt: null };
    window.recovery = { instanceId: "target", operationId: "op", attempt: 1, instanceRevision: 3, candidateRevision: 2, previousStatus: "Failed", currentRuntime: "stopped",
      actions: ["restore_ports", "retry_ports", "propose_ports", "abandon"], holdReasons: [], ports: [port], originalPorts: [{ ...port, hostPort: 15432 }], proposedPorts: [], artifactId: null, confirmationHash: null, files: [] };
    window.__TAURI_EVENT_PLUGIN_INTERNALS__ = { unregisterListener: () => {} };
    window.__TAURI_INTERNALS__ = { transformCallback: () => 1, invoke: async (command, args) => {
      if (command === "plugin:event|listen") return 1;
      if (command === "plugin:event|unlisten") return null;
      const request = args.request;
      window.calls.push({ command, request: structuredClone(request) });
      const response = (result, error = null) => ({ apiVersion: 1, requestId: (request.context ?? request).requestId, result, error });
      if (command === "get_bootstrap") return response({ applicationTitle: "ComposeNest", startedAtUnixSeconds: 1 });
      if (command === "list_instances") return response([]);
      if (command === "get_operation") return request.operationId === window.progress.operation.id ? response(structuredClone(window.progress)) : response(null, { code: "INSTANCE_MISSING" });
      if (command === "resolve_operation") {
        if (window.mode === "read_failure") throw new Error("private-backend-secret");
        const result = structuredClone(window.recovery);
        if (window.mode === "wrong_target") result.instanceId = "foreign";
        if (window.mode === "wrong_operation") result.operationId = "foreign";
        if (window.mode === "bad_attempt") result.attempt = -1;
        if (window.mode === "bad_actions") result.actions = null;
        const reply = response(result);
        if (window.mode === "bad_envelope") reply.requestId = "wrong";
        return reply;
      }
      if (command === "retry_operation") {
        if (window.mode === "delay") await new Promise((resolve) => { window.finishRecovery = resolve; });
        if (window.mode === "lost") throw new Error("private-backend-secret");
        if (request.action === "propose_ports") {
          window.recovery.proposedPorts = window.recovery.ports.map((p) => ({ ...p, hostPort: 15434 }));
        } else if (request.action === "restore_external") {
          const id = `recover-${request.context.requestId}`;
          window.recovery.operationId = id; window.recovery.actions = [];
          window.progress.operation.id = id; window.progress.operation.kind = "recover"; window.progress.operation.status = "Succeeded";
          window.progress.instance.lastOperation.id = id; window.progress.sequence++; window.progress.completedAt = "2026-10-07 02:01:00";
          if (window.mode === "lost_external") throw new Error("private-backend-secret");
        } else {
          window.progress.operation.status = "OutcomeUnknown";
          window.recovery.actions = []; window.recovery.currentRuntime = "unknown";
          window.recovery.holdReasons = ["CLI_TERMINATION_UNCONFIRMED"]; window.progress.sequence++;
        }
        return response(structuredClone(window.recovery));
      }
      throw new Error(`Unexpected command ${command}`);
    } };
  });
  const open = async () => { await page.goto("about:blank"); await page.goto(origin + "/#/operation?operationId=op&instanceId=target"); await page.getByRole("heading", { name: "復旧・実体の確認", exact: true }).waitFor(); };
  const read = async () => { await page.getByRole("button", { name: "実体を再確認", exact: true }).click(); await page.waitForFunction(() => document.querySelector(".recovery-panel")?.getAttribute("aria-busy") === "false"); };
  const calls = (command) => page.evaluate((command) => window.calls.filter((c) => c.command === command), command);
  await open(); assert.equal((await calls("resolve_operation")).length, 0, "opening does not execute recovery inspection or changes");
  await read(); await page.getByRole("button", { name: "旧ポートに戻す", exact: true }).click();
  const modal = page.getByRole("dialog"); await modal.waitFor();
  assert.ok((await modal.innerText()).includes("15433")); assert.ok((await modal.innerText()).includes("15432"));
  await modal.getByRole("button", { name: "キャンセル", exact: true }).click(); assert.equal((await calls("retry_operation")).length, 0);
  await page.getByRole("button", { name: "新しいポート候補を確認", exact: true }).click();
  await page.getByRole("button", { name: "候補ポートの差分を確認", exact: true }).click();
  assert.ok((await modal.innerText()).includes("15434")); assert.equal((await calls("retry_operation")).length, 1);
  await modal.getByRole("button", { name: "候補ポートを適用", exact: true }).click();
  await page.getByText("前のCLIが終了したことを確認できません。", { exact: false }).waitFor();
  const sent = (await calls("retry_operation"))[1].request;
  assert.equal(sent.operationId, "op"); assert.equal(sent.expectedAttempt, 1); assert.equal(sent.expectedRevision, 3); assert.equal(sent.candidateRevision, 2);
  assert.deepEqual(sent.ports, { db: 15434 }); assert.equal(sent.action, "confirm_ports");
  assert.equal(await page.getByRole("heading", { name: "処理が完了しました", exact: true }).count(), 0);

  await open(); await page.evaluate(() => { window.recovery.currentRuntime = "ready"; window.recovery.actions = ["complete"]; }); await read();
  assert.ok((await page.locator(".recovery-panel").innerText()).includes("Ready（利用可能）"));
  assert.ok((await page.locator(".recovery-panel").innerText()).includes("失敗"));
  assert.equal(await page.getByRole("heading", { name: "処理に失敗しました", exact: true }).count(), 1);
  for (const reason of ["CLI_TERMINATION_UNCONFIRMED", "STORAGE_MISSING", "OWNERSHIP_UNKNOWN", "RUNTIME_TARGET_MISMATCH"]) {
    await page.evaluate((reason) => { window.recovery.actions = []; window.recovery.holdReasons = [reason]; window.recovery.currentRuntime = "unknown"; }, reason); await read();
    assert.equal(await page.locator(".recovery-panel .actions button").count(), 0);
  }
  for (const mode of ["wrong_target", "wrong_operation", "bad_attempt", "bad_actions", "bad_envelope", "read_failure"]) {
    await page.evaluate((mode) => { window.mode = mode; }, mode); await read();
    assert.equal(await page.locator(".recovery-panel .actions button").count(), 0);
  }
  await open(); await read(); await page.evaluate(() => { window.mode = "delay"; });
  await page.getByRole("button", { name: "同じポート変更を再試行", exact: true }).click();
  await modal.getByRole("button", { name: "同じポート変更を再試行", exact: true }).click();
  await page.waitForFunction(() => typeof window.finishRecovery === "function");
  assert.equal(await page.getByRole("button", { name: "旧ポートに戻す", exact: true }).isDisabled(), true);
  assert.equal((await calls("retry_operation")).length, 1);
  await page.evaluate(() => { window.finishRecovery(); }); await page.getByText("前のCLIが終了したことを確認できません。", { exact: false }).waitFor();
  await open(); await read(); await page.evaluate(() => { window.mode = "lost"; });
  await page.getByRole("button", { name: "旧ポートに戻す", exact: true }).click(); await modal.getByRole("button", { name: "旧ポートに戻す", exact: true }).click();
  await page.getByText("復旧結果を確認できません。", { exact: false }).waitFor();
  assert.equal(await page.locator(".recovery-panel .actions button").count(), 0);
  await page.evaluate(() => { window.mode = "normal"; }); await read();
  assert.equal((await calls("retry_operation")).length, 1, "reconciliation only reads; it never resends a change");

  await open(); await page.evaluate(() => {
    window.recovery.actions = ["restore_external"]; window.recovery.artifactId = "artifact"; window.recovery.confirmationHash = "confirmed-hash";
    window.recovery.files = [{ path: "compose.yaml", recordedHash: "a".repeat(64), observedHash: "b".repeat(64) }, { path: ".env", recordedHash: null, observedHash: "c".repeat(64) }];
  }); await read();
  await page.getByRole("button", { name: "外部編集を退避して保存設定に戻す", exact: true }).click(); await modal.waitFor();
  assert.ok((await modal.innerText()).includes("compose.yaml")); assert.ok((await modal.innerText()).includes("追加ファイル"));
  await mkdir("test-results", { recursive: true });
  for (const [theme, width] of [["ライト", 1280], ["ダーク", 390]]) {
    if (theme === "ダーク") {
      await modal.getByRole("button", { name: "キャンセル", exact: true }).click();
      await page.getByRole("button", { name: "ダーク", exact: true }).click();
      await page.getByRole("button", { name: "外部編集を退避して保存設定に戻す", exact: true }).click();
      await modal.waitFor();
    }
    await page.setViewportSize({ width, height: 900 }); assert.equal(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth), true);
    await page.screenshot({ path: `test-results/recovery-${width}.png`, fullPage: true });
  }
  await page.evaluate(() => { window.mode = "lost_external"; }); await modal.getByRole("button", { name: "外部編集を退避して保存設定に戻す", exact: true }).click();
  await page.getByText("復旧結果を確認できません。", { exact: false }).waitFor();
  const external = (await calls("retry_operation"))[0].request;
  assert.equal(external.artifactId, "artifact"); assert.equal(external.confirmationHash, "confirmed-hash");
  await page.evaluate(() => { window.mode = "normal"; }); await read();
  await page.waitForURL(/operationId=recover-/);
  assert.equal((await calls("retry_operation")).length, 1);
  assert.equal(await page.getByText("private-backend-secret", { exact: false }).count(), 0);
  assert.deepEqual(errors, []);
  console.log("Recovery: Core holds, historical failure/current Ready, diffs, explicit confirmation, no optimistic success, duplicate guards, invalid responses and lost external receipt passed.");
} finally { await browser?.close(); server.kill(); }
