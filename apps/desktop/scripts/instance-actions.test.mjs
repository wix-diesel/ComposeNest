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
    window.view = { lifecycle: "managed", id: "target", name: "開発用データベース", revision: 3, runtimeStatus: "stopped", observedAt: "2026-10-05 08:00:00", operationId: null, operationStatus: null, operationKind: null, operationPhase: null, actions: ["rename", "start", "stop", "restart", "delete"] };
    window.edit = { templateId: "generic-service", selectedVersion: "custom-17", storageMethod: "volume", specRevision: 1, appliedSpecRevision: 1,
      inputs: [{ slot: "username", label: "接続ユーザー", secret: false, value: "app-user" }, { slot: "password", label: "認証用パスワード", secret: true, value: null }, { slot: "enabled", label: "機能を有効化", secret: false, value: false }],
      ports: [{ slot: "db", hostIp: "127.0.0.1", containerPort: 5432, oldPort: 15432, committedPort: 15432, candidatePort: null, oldReservation: "committed", candidateReservation: null }, { slot: "metrics", hostIp: "127.0.0.1", containerPort: 9000, oldPort: 19000, committedPort: 19000, candidatePort: null, oldReservation: "committed", candidateReservation: null }] };
    window.__TAURI_EVENT_PLUGIN_INTERNALS__ = { unregisterListener: () => {} };
    window.__TAURI_INTERNALS__ = { transformCallback: () => 1, invoke: async (command, { request }) => {
      window.calls.push({ command, request: structuredClone(request) });
      const response = (result, error = null) => ({ apiVersion: 1, requestId: (request.context ?? request).requestId, result, error });
      if (command === "get_bootstrap") return response({ applicationTitle: "ComposeNest", startedAtUnixSeconds: 1 });
      if (command === "get_instance_detail") {
        if (window.mode === "read_failure") throw new Error("Disconnected");
        const state = structuredClone(window.view);
        if (window.mode === "wrong_target") state.id = "another-instance";
        return response({ state, creationStartedAt: "2026-10-01 05:15:00", cloneSourceId: "source-id", locations: window.locations ?? [{ slot: "data", location: "/managed/data/target/data" }],
          instance: { ...state, templateId: window.edit.templateId, selectedVersion: window.edit.selectedVersion, templateVersion: "2.3.4",
            specRevision: window.edit.specRevision, appliedSpecRevision: window.edit.appliedSpecRevision, inputs: structuredClone(window.edit.inputs),
            observation: state.observedAt ? { observedAt: state.observedAt, freshness: window.freshness ?? "fresh" } : null,
            storage: [{ slot: "data", method: window.edit.storageMethod, presence: "present" }],
            connections: window.edit.ports.map((port) => ({ slot: port.slot, label: port.slot, port: { ...port, hostPort: port.committedPort }, inputSlots: ["username", "password", "enabled"] })) } });
      }
      if (command === "plugin:event|listen") return 1;
      if (command === "plugin:event|unlisten") return null;
      if (command === "list_instances") return response(window.view.lifecycle === "retired" ? [] : [{ ...structuredClone(window.view), ...structuredClone(window.edit), ports: [], observation: null, needsAttention: window.view.lifecycle === "retiring" }]);
      if (command === "list_retained_storage") return response([]);
      if (command === "get_operation") return response({ operation: { id: window.view.operationId, kind: window.view.operationKind, status: window.view.operationStatus, phase: window.view.operationPhase, startedAt: "2026-10-07 00:00:00" },
        instance: { ...structuredClone(window.view), ...structuredClone(window.edit), ports: [], connections: [], lastOperation: { id: window.view.operationId } }, sequence: 0, completedAt: window.view.lifecycle === "retired" ? "2026-10-07 00:01:00" : null });
      if (command === "get_instance_actions") { if (window.mode === "read_failure") throw new Error("Disconnected"); return response(structuredClone(window.view)); }
      const editView = () => ({ ...structuredClone(window.edit), state: structuredClone(window.view), canEditPorts: ["stopped", "absent"].includes(window.view.runtimeStatus) && window.view.actions.includes("rename") });
      if (command === "get_instance_edit") {
        if (window.mode === "fresh_running") Object.assign(window.view, { runtimeStatus: "ready", actions: ["rename", "stop"] });
        return response(editView());
      }
      if (command === "edit_instance_ports") {
        if (window.mode === "port_conflict") return response(null, { code: "PORT_EDIT_CONFLICT" });
        if (window.mode === "delay") await new Promise((resolve) => { window.finishChange = resolve; });
        if (!window.view.operationId) {
          Object.assign(window.view, { operationId: "port-operation", operationKind: "edit_port", operationStatus: window.mode === "outcome_unknown" ? "OutcomeUnknown" : "Accepted", operationPhase: "recreate", actions: [] });
          window.edit.ports.forEach((port) => { port.candidatePort = request.ports[port.slot]; port.candidateReservation = port.candidatePort === port.oldPort ? "committed" : "held"; });
        }
        if (window.mode === "port_success") {
          Object.assign(window.view, { revision: 4, runtimeStatus: "stopped", operationStatus: "Succeeded", operationPhase: "stopped", actions: ["rename", "start"] });
          Object.assign(window.edit, { specRevision: 2, appliedSpecRevision: 2 });
          window.edit.ports.forEach((port) => { port.committedPort = port.candidatePort; port.candidateReservation = "committed"; port.oldReservation = port.oldPort === port.committedPort ? "committed" : "released"; });
        }
        if (window.mode === "lost") throw new Error("Disconnected after port acceptance");
        return response(editView());
      }
      if (command === "change_instance") {
        if (window.mode === "delay") await new Promise((resolve) => { window.finishChange = resolve; });
        if (window.mode === "stale") return response(null, { code: "INSTANCE_STALE" });
        if (!window.view.operationId) Object.assign(window.view, { operationId: "accepted-op", operationStatus: "Accepted", operationKind: request.action, operationPhase: "inspect", actions: [], revision: request.action === "delete" ? request.expectedRevision + 1 : window.view.revision, lifecycle: request.action === "delete" ? "retiring" : "managed" });
        if (window.mode === "lost") throw new Error("Disconnected after acceptance");
        if (window.mode === "bad_delete_response") return response({ ...structuredClone(window.view), id: "another-instance" });
        return response(structuredClone(window.view));
      }
      if (command === "rename_instance") {
        if (window.mode === "lost_before_rename") throw new Error("Disconnected before rename");
        if (window.mode === "rename_race") {
          window.view.name = request.name.trim().normalize("NFC"); window.view.revision = request.expectedRevision + 1;
          return response(null, { code: "INSTANCE_STALE" });
        }
        if (window.mode === "duplicate") return response(null, { code: "NAME_OR_REQUEST_CONFLICT" });
        if (window.mode === "stale") return response(null, { code: "INSTANCE_STALE" });
        if (window.mode === "delay") await new Promise((resolve) => { window.finishChange = resolve; });
        window.view.name = request.name.trim().normalize("NFC"); window.view.revision++;
        if (window.mode === "lost") throw new Error("Disconnected after rename");
        return response(structuredClone(window.view));
      }
      throw new Error(`Unexpected command ${command}`);
    } };
  });
  async function open(edit = false) {
    await page.goto("about:blank");
    await page.goto(`http://127.0.0.1:4176/#/instance-${edit ? "edit" : "detail"}?instanceId=target`);
    await page.locator(".instance-actions h1, .instance-actions h2").filter({ hasText: "開発用データベース" }).waitFor();
  }
  const calls = (command) => page.evaluate((command) => window.calls.filter((call) => call.command === command), command);
  async function rename(name = "変更した名前") {
    await page.locator("#edit-name").fill(name);
    await page.getByRole("button", { name: "変更内容を確認", exact: true }).click();
    await page.getByRole("button", { name: "名前変更を適用", exact: true }).click();
  }
  if (process.argv.includes("--delete")) {
    const remove = () => page.getByRole("button", { name: "環境を削除", exact: true }).click();
    const confirm = () => page.getByRole("button", { name: "データを残して削除", exact: true });
    const acknowledge = () => page.getByRole("checkbox").check();
    await open(); await remove();
    await page.getByRole("heading", { name: "環境を削除。データは残ります", exact: true }).waitFor();
    for (const value of ["/managed/data/target/data", "対象版 r3", "generic-service custom-17"])
      assert.ok((await page.getByRole("dialog").textContent()).includes(value), value);
    assert.equal(await confirm().isDisabled(), true);
    assert.equal(await page.getByRole("button", { name: "キャンセル", exact: true }).evaluate((button) => document.activeElement === button), true);
    await acknowledge(); await page.keyboard.press("Escape");
    assert.equal((await calls("change_instance")).length, 0);
    assert.equal(await page.getByRole("button", { name: "環境を削除", exact: true }).evaluate((button) => document.activeElement === button), true);
    await remove(); await acknowledge(); await page.getByRole("button", { name: "キャンセル", exact: true }).click();
    assert.equal((await calls("change_instance")).length, 0);
    await remove(); await acknowledge();
    await page.evaluate(() => { window.view.revision++; });
    await page.getByText("現在の状態を再確認し、削除内容を確認し直してください。", { exact: true }).waitFor();
    assert.equal(await confirm().isDisabled(), true);
    await page.keyboard.press("Escape");
    await open(); await remove(); await acknowledge();
    await page.evaluate(() => { window.mode = "stale"; }); await confirm().click();
    await page.getByRole("alert").waitFor();
    assert.equal((await calls("change_instance")).length, 1);
    assert.equal(await page.getByRole("button", { name: "環境を削除", exact: true }).isDisabled(), true);
    for (const [method, location] of [["bind", "C:\\ProgramData\\ComposeNest\\data\\target\\data"], ["volume", "cn-target-data"]]) {
      await open();
      await page.evaluate(({ method, location }) => { window.edit.storageMethod = method; window.locations = [{ slot: "data", location }]; }, { method, location });
      await page.getByText(location, { exact: true }).waitFor(); await remove();
      assert.equal(await page.getByRole("dialog").getByText(location, { exact: true }).count(), 1);
      await page.keyboard.press("Escape");
    }
    await open(); await page.evaluate(() => { window.locations = []; });
    await page.getByText("保存先は未取得", { exact: true }).waitFor(); await remove(); await acknowledge();
    assert.equal(await confirm().isDisabled(), true); await page.keyboard.press("Escape");
    await open(); await remove(); await acknowledge();
    await page.evaluate(() => { window.mode = "delay"; });
    await confirm().evaluate((button) => { button.click(); button.click(); });
    assert.equal((await calls("change_instance")).length, 1);
    assert.equal(await page.getByRole("button", { name: "環境を削除", exact: true }).isDisabled(), true);
    await page.getByText("処理の受付を確認中です。画面を離れても送信した要求は取り消されません。", { exact: true }).waitFor();
    await page.evaluate(() => window.finishChange());
    await page.getByRole("heading", { name: "処理状況", exact: true }).waitFor();
    const sent = (await calls("change_instance"))[0].request;
    assert.equal(sent.expectedRevision, 3); assert.equal(sent.retainDataConfirmed, true); assert.equal(sent.action, "delete");
    assert.equal(await page.getByRole("button", { name: "保持データを確認", exact: true }).count(), 0);
    for (const status of ["Failed", "OutcomeUnknown", "Succeeded"]) {
      await page.evaluate((status) => { window.view.operationStatus = status; }, status);
      await page.getByRole("button", { name: "再照会", exact: true }).click();
      await page.getByText("環境の削除は完了していません", { exact: true }).waitFor();
      assert.equal(await page.getByRole("button", { name: "保持データを確認", exact: true }).count(), 0);
    }
    await page.evaluate(() => { window.view.lifecycle = "retired"; });
    await page.getByRole("button", { name: "再照会", exact: true }).click();
    await page.getByText("環境の削除が完了しました。データは残ります", { exact: true }).waitFor();
    await page.getByRole("button", { name: "保持データを確認", exact: true }).click();
    await page.getByRole("heading", { name: "保持データ", exact: true }).waitFor();
    await page.evaluate(() => { location.hash = "#/instances"; });
    await page.getByText("環境はまだありません。「環境を作成」から追加してください。", { exact: true }).waitFor();
    for (const mode of ["lost", "bad_delete_response"]) {
      await open(); await remove(); await acknowledge();
      await page.evaluate((mode) => { window.mode = mode; }, mode); await confirm().click();
      await page.getByText("受付結果の確認が必要です。別の変更は実行できません。", { exact: true }).waitFor();
      await page.getByRole("button", { name: "戻る", exact: true }).click();
      await page.getByRole("heading", { name: "環境一覧", exact: true }).waitFor();
      await page.evaluate(() => { window.mode = "normal"; location.hash = "#/instance-detail?instanceId=target"; });
      await page.getByRole("button", { name: "受付を再確認", exact: true }).click();
      await page.getByRole("heading", { name: "処理状況", exact: true }).waitFor();
      const repeated = await calls("change_instance"); assert.equal(repeated.length, 2); assert.deepEqual(repeated[0].request, repeated[1].request);
      await page.evaluate(() => { window.view.operationStatus = "Failed"; window.view.operationPhase = "delete_check"; });
      await page.getByRole("button", { name: "再照会", exact: true }).click();
      await page.getByRole("button", { name: "閉じる", exact: true }).click();
      await page.getByText("開発用データベース", { exact: true }).waitFor();
      assert.equal((await calls("change_instance")).length, 2, "closing progress never cancels or resubmits");
    }
    for (const [theme, width] of [["light", 1280], ["dark", 390]]) {
      await open(); await page.setViewportSize({ width, height: 900 });
      await page.evaluate((theme) => { document.documentElement.dataset.theme = theme; }, theme);
      await remove();
      assert.equal(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth), true);
      await mkdir("test-results", { recursive: true });
      await page.screenshot({ path: `test-results/delete-${theme}-${width}.png`, fullPage: true });
      await page.keyboard.press("Escape");
    }
    console.log("Delete: explicit retention/revision, cancellation, stale guard, duplicate clicks, durable progress, Retiring failures, retained navigation, same-request replay and themes passed.");
  } else {
  await open();
  assert.equal(await page.getByRole("heading", { level: 1 }).textContent(), "開発用データベース");
  assert.equal((await calls("get_instance_detail"))[0].request.instanceId, "target");
  for (const value of ["generic-service custom-17", "app-user", "false", "/managed/data/target/data", "2.3.4", "source-id", "2026-10-01 05:15:00 UTC", "保存 r1 / 適用 r1", "外部構成は未確認"])
    assert.ok(await page.getByText(value, { exact: true }).count() > 0, value);
  assert.equal(await page.getByText("••••••••（非表示）", { exact: true }).count(), 2);
  for (const label of ["接続ユーザー", "認証用パスワード", "機能を有効化"])
    assert.equal(await page.locator(".detail-connection dt").filter({ hasText: label }).count(), 2, label);
  for (const slot of ["username", "password", "enabled"])
    assert.equal(await page.getByText(slot, { exact: true }).count(), 0, slot);
  const tabs = page.getByRole("tab");
  await tabs.nth(0).focus();
  for (const [key, selected] of [["ArrowRight", 1], ["End", 2], ["ArrowRight", 0], ["ArrowLeft", 2], ["Home", 0]]) {
    await page.keyboard.press(key);
    assert.equal(await tabs.nth(selected).getAttribute("aria-selected"), "true");
    assert.equal(await tabs.nth(selected).evaluate((button) => button === document.activeElement), true);
    assert.equal(await page.getByRole("tabpanel").count(), 1);
    assert.equal(await page.locator('[role="tab"][tabindex="0"]').count(), 1);
  }
  await tabs.nth(1).click();
  await page.getByText("ログ購読は未接続です。ログは取得していません。", { exact: true }).waitFor();
  assert.equal(await page.getByRole("button", { name: "追従を開始", exact: true }).isDisabled(), true);
  await tabs.nth(2).click();
  assert.equal(await page.getByRole("button", { name: "原文を表示", exact: true }).isDisabled(), true);
  assert.deepEqual((await page.evaluate(() => window.calls)).filter((call) => !["get_bootstrap", "get_instance_detail"].includes(call.command)), []);
  await tabs.nth(0).click();
  await page.evaluate(() => { window.freshness = "stale"; window.view.observedAt = "2026-09-30 06:05:04"; window.edit.appliedSpecRevision = null; });
  await page.getByText("保存 r1 / 適用 未確認", { exact: true }).waitFor();
  await page.getByText("最終観測（保存値） 2026-09-30 06:05:04 UTC（過去の観測）", { exact: true }).waitFor();
  for (const [theme, width] of [["light", 1280], ["dark", 390]]) {
    await page.setViewportSize({ width, height: 900 });
    await page.evaluate((theme) => { document.documentElement.dataset.theme = theme; }, theme);
    assert.equal(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth), true);
    await mkdir("test-results", { recursive: true });
    await page.screenshot({ path: `test-results/instance-detail-${theme}-${width}.png`, fullPage: true });
  }
  await page.setViewportSize({ width: 1280, height: 900 });
  await page.evaluate(() => { window.view.observedAt = null; });
  await page.getByText("最終観測（保存値） 未確認", { exact: true }).waitFor();
  await open();
  await page.evaluate(() => { window.mode = "read_failure"; });
  await page.getByRole("alert").waitFor();
  assert.equal(await page.getByText("/managed/data/target/data", { exact: true }).count(), 0);
  assert.equal(await page.getByRole("button", { name: "停止", exact: true }).isDisabled(), true);
  await open(); await page.evaluate(() => { window.mode = "wrong_target"; });
  await page.getByRole("alert").waitFor();
  assert.equal(await page.getByText("/managed/data/target/data", { exact: true }).count(), 0);
  assert.equal(await page.getByRole("button", { name: "編集", exact: true }).isDisabled(), true);
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
    await open(true); await page.locator("#edit-port-db").fill("15435"); await page.evaluate((mode) => { window.mode = mode; }, mode); await rename();
    await page.getByText(text, { exact: true }).waitFor();
    assert.equal(await page.locator("#edit-name").isDisabled(), mode === "stale");
    assert.equal(await page.locator("#edit-port-db").inputValue(), "15435");
    await page.getByRole("button", { name: "現在の状態を再確認", exact: true }).click();
    await page.waitForFunction(() => !document.querySelector("#edit-name").disabled);
    assert.equal(await page.locator("#edit-name").inputValue(), "変更した名前", "refresh preserves the unaccepted draft");
    assert.equal(await page.locator("#edit-port-db").inputValue(), "15435");
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

  async function confirmPorts(port = "15433") {
    await page.locator("#edit-port-db").fill(port);
    await page.getByRole("button", { name: "ポート変更内容を確認", exact: true }).click();
  }
  const applyPorts = () => page.getByRole("button", { name: "ポート変更を適用", exact: true }).click();
  await open(true);
  assert.equal(await page.getByText("generic-service custom-17", { exact: true }).count(), 1);
  assert.equal(await page.getByText("app-user", { exact: true }).count(), 1);
  assert.equal(await page.getByText("false", { exact: true }).count(), 1);
  assert.equal(await page.getByText("••••••••（非表示）", { exact: true }).count(), 1);
  assert.equal(await page.getByText("named volume", { exact: true }).count(), 1);
  assert.equal(await page.locator("input").count(), 3, "version, initialization and storage have no editable controls");
  for (const value of ["80", "65536", "15432.5", "19000", ""]) {
    await page.locator("#edit-port-db").fill(value);
    assert.equal(await page.getByRole("button", { name: "ポート変更内容を確認", exact: true }).isDisabled(), true);
  }
  await page.locator("#edit-port-db").fill("15433");
  await rename();
  assert.equal(await page.locator("#edit-port-db").inputValue(), "15433", "rename preserves the separate port draft");
  assert.equal((await calls("edit_instance_ports")).length, 0);
  await confirmPorts();
  await page.getByText("15432 → 15433", { exact: true }).waitFor();
  await page.keyboard.press("Escape");
  assert.equal((await calls("edit_instance_ports")).length, 0);
  await confirmPorts(); await page.evaluate(() => { window.mode = "port_conflict"; }); await applyPorts();
  await page.getByText("新しいポートを利用できません。ポートとDockerの接続を確認してください。", { exact: true }).waitFor();
  assert.equal(await page.getByText("環境名を変更しました。", { exact: true }).count(), 1, "rename success is separate from the port failure");
  assert.equal(await page.locator("#edit-port-db").isDisabled(), false, "a definitive conflict leaves the draft editable");
  assert.equal(await page.locator("#edit-port-db").inputValue(), "15433");
  await page.locator("#edit-name").fill("未送信の名前");
  await page.getByRole("button", { name: "現在の状態を再確認", exact: true }).click();
  await page.waitForFunction(() => !document.querySelector("#edit-name").disabled);
  assert.equal(await page.locator("#edit-name").inputValue(), "未送信の名前");
  assert.equal(await page.locator("#edit-port-db").inputValue(), "15433", "refresh preserves both independent drafts");
  const separate = (await calls("edit_instance_ports"))[0].request;
  assert.equal(separate.expectedRevision, 4); assert.equal(separate.expectedSpecRevision, 1);
  assert.deepEqual(separate.ports, { db: 15433, metrics: 19000 }); assert.equal("name" in separate, false);
  await confirmPorts("15434"); await page.evaluate(() => { window.mode = "normal"; }); await applyPorts();
  await page.getByText("ポート変更: 受付済み・未適用。", { exact: true }).waitFor();
  const corrected = (await calls("edit_instance_ports"))[1].request;
  assert.equal(corrected.ports.db, 15434);
  assert.notEqual(corrected.context.requestId, separate.context.requestId);
  assert.equal(await page.locator(".operation-link").textContent(), "処理ID: port-operation · コンテナ再作成 復旧・結果確認");
  await page.evaluate(() => { window.view.operationPhase = "future_phase"; });
  await page.waitForFunction(() => document.querySelector(".operation-link").textContent.includes("確認中"));


  await open(true); await confirmPorts();
  await page.evaluate(() => { window.mode = "fresh_running"; }); await applyPorts();
  await page.getByText("停止またはコンテナ不在を新しく確認できなかったため、ポート変更は受け付けていません。", { exact: true }).waitFor();
  assert.equal((await calls("edit_instance_ports")).length, 0, "fresh running state blocks sending the request");
  assert.equal(await page.locator("#edit-name").isDisabled(), false);
  assert.equal(await page.locator("#edit-port-db").isDisabled(), true);
  await open(true); await page.evaluate(() => { window.view.runtimeStatus = "unknown"; });
  await page.waitForFunction(() => document.querySelector("#edit-port-db").disabled);
  assert.equal(await page.locator("#edit-name").isDisabled(), false, "unknown runtime does not prohibit independent rename");

  await open(true); await page.evaluate(() => { window.view.runtimeStatus = "absent"; });
  await page.getByText("コンテナ不在", { exact: true }).waitFor();
  await confirmPorts(); await page.evaluate(() => { window.mode = "delay"; });
  await page.getByRole("button", { name: "ポート変更を適用", exact: true }).evaluate((button) => { button.click(); button.click(); });
  await page.waitForFunction(() => typeof window.finishChange === "function");
  await page.getByText("変更の開始準備中です。", { exact: true }).waitFor();
  assert.equal((await calls("edit_instance_ports")).length, 1);
  assert.equal(await page.locator("#edit-name").isDisabled(), true);
  await page.evaluate(() => window.finishChange());
  await page.getByText("ポート変更: 受付済み・未適用。", { exact: true }).waitFor();
  assert.equal(await page.getByText("15432 · 確定予約を保持", { exact: true }).count(), 1);
  assert.equal(await page.getByText("15433 · 候補予約を保持", { exact: true }).count(), 1);
  assert.equal(await page.locator("#edit-port-db").isDisabled(), true);
  assert.equal((await calls("change_instance")).length, 0, "port edit never starts a container");
  await page.evaluate(() => {
    Object.assign(window.view, { revision: 4, runtimeStatus: "stopped", operationStatus: "Succeeded", actions: ["rename", "start"] });
    Object.assign(window.edit, { specRevision: 2, appliedSpecRevision: 2 });
    window.edit.ports.forEach((port) => { port.committedPort = port.candidatePort; port.oldReservation = port.oldPort === port.committedPort ? "committed" : "released"; port.candidateReservation = "committed"; });
  });
  await page.getByText("ポート変更: 適用完了。", { exact: true }).waitFor();
  await page.getByText("表示していた版が古くなりました。現在の状態を再確認してください。", { exact: true }).waitFor();
  await page.getByRole("button", { name: "現在の状態を再確認", exact: true }).click();
  assert.equal(await page.locator("#edit-port-db").inputValue(), "15433");
  await confirmPorts("15434");
  await page.evaluate(() => { window.edit.specRevision++; });
  await page.getByText("表示していた版が古くなりました。現在の状態を再確認してください。", { exact: true }).waitFor();
  assert.equal(await page.getByRole("button", { name: "ポート変更を適用", exact: true }).isDisabled(), true);
  await page.keyboard.press("Escape");
  assert.equal((await calls("edit_instance_ports")).length, 1);

  await open(true); await confirmPorts(); await page.evaluate(() => { window.mode = "port_success"; }); await applyPorts();
  await page.getByText("ポート変更: 適用完了。", { exact: true }).waitFor();
  assert.equal(await page.locator(".badge").textContent(), "停止中");
  assert.equal(await page.locator("#edit-port-db").inputValue(), "15433");
  assert.equal(await page.getByText("15432 · 解放済み", { exact: true }).count(), 1);
  assert.equal(await page.getByText("保存 r2 / 適用 r2", { exact: true }).count(), 1);
  assert.equal((await calls("rename_instance")).length, 0);
  assert.equal((await calls("change_instance")).length, 0);
  for (const mode of ["outcome_unknown", "lost"]) {
    await open(true); await confirmPorts(); await page.evaluate((mode) => { window.mode = mode; }, mode); await applyPorts();
    await page.getByText(mode === "lost" ? "受付結果の確認が必要です。別の変更は実行できません。" : "ポート変更: 結果不明。", { exact: true }).waitFor();
    await page.getByText("15433 · 候補予約を保持", { exact: true }).waitFor();
    assert.equal(await page.locator("#edit-name").isDisabled(), true);
    assert.equal(await page.getByText("15432 · 確定予約を保持", { exact: true }).count(), 1);
    assert.equal(await page.getByText("15433 · 候補予約を保持", { exact: true }).count(), 1);
    if (mode === "lost") {
      await page.evaluate(() => { location.hash = "#/instances"; });
      await page.getByRole("heading", { level: 1, name: "環境一覧", exact: true }).waitFor();
      await page.evaluate(() => { window.mode = "normal"; location.hash = "#/instance-edit?instanceId=target"; });
      await page.getByRole("button", { name: "受付を再確認", exact: true }).click();
      await page.waitForFunction(() => !document.querySelector(".instance-edit").textContent.includes("受付結果の確認が必要です"));
      const requests = await calls("edit_instance_ports"); assert.equal(requests.length, 2); assert.deepEqual(requests[0].request, requests[1].request);
      assert.equal(await page.locator("#edit-name").isDisabled(), true, "known acceptance does not imply completed application");
    }
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
    await page.locator(".notice").filter({ hasText: "未解決の処理があるため、別の変更は実行できません。" }).waitFor();
    assert.equal(await page.getByRole("button", { name: "復旧・結果確認", exact: true }).isVisible(), true);
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
  await open(true); await page.evaluate(() => { window.mode = "lost_before_rename"; }); await rename();
  const pendingMessage = "名前変更の確定を確認できませんでした。入力内容を保持しています。受付を再確認してください。";
  await page.getByRole("button", { name: "受付を再確認", exact: true }).click();
  await page.getByText(pendingMessage, { exact: true }).waitFor();
  assert.equal(await page.locator("#edit-name").inputValue(), "変更した名前");
  assert.equal(await page.locator("#edit-name").isDisabled(), true, "an old name/revision cannot release the pending request");
  assert.equal((await calls("rename_instance")).length, 2);
  await page.evaluate(() => { location.hash = "#/instances"; });
  await page.getByRole("heading", { level: 1, name: "環境一覧", exact: true }).waitFor();
  await page.evaluate(() => { location.hash = "#/instance-edit?instanceId=target"; });
  await page.locator("#edit-name").waitFor();
  await page.waitForFunction(() => document.querySelector("#edit-name").value === "変更した名前");
  await page.evaluate(() => { window.mode = "normal"; });
  await page.getByRole("button", { name: "受付を再確認", exact: true }).click();
  await page.waitForFunction(() => !document.querySelector("#edit-name").disabled);
  const nameRequests = await calls("rename_instance");
  assert.equal(nameRequests.length, 3);
  for (const call of nameRequests) assert.deepEqual(call.request, nameRequests[0].request);

  for (const current of [{ name: "別の名前", revision: 4 }, { name: "変更した名前", revision: 5 }]) {
    await open(true); await page.evaluate(() => { window.mode = "lost"; }); await rename();
    await page.evaluate((current) => { Object.assign(window.view, current); window.mode = "normal"; }, current);
    await page.getByRole("button", { name: "受付を再確認", exact: true }).click();
    await page.getByText(pendingMessage, { exact: true }).waitFor();
    assert.equal(await page.locator("#edit-name").inputValue(), "変更した名前");
    assert.equal(await page.locator("#edit-name").isDisabled(), true);
    assert.equal((await calls("rename_instance")).length, 1, "a conflicting revision must not be replayed");
  }
  await open(true); await page.evaluate(() => { window.mode = "lost_before_rename"; }); await rename();
  await page.evaluate(() => { window.mode = "rename_race"; });
  await page.getByRole("button", { name: "受付を再確認", exact: true }).click();
  await page.waitForFunction(() => !document.querySelector("#edit-name").disabled);
  const raced = await calls("rename_instance"); assert.equal(raced.length, 2); assert.deepEqual(raced[0].request, raced[1].request);

  await open(true); await page.evaluate(() => { window.mode = "lost"; }); await rename("　Cafe\u0301　");
  await page.evaluate(() => { window.mode = "normal"; });
  await page.getByRole("button", { name: "受付を再確認", exact: true }).click();
  await page.waitForFunction(() => document.querySelector("#edit-name").value === "Café" && !document.querySelector("#edit-name").disabled);
  assert.equal((await calls("rename_instance")).length, 1, "Core-normalized names confirm an already committed rename");
  await mkdir("test-results", { recursive: true });
  for (const [theme, width] of [["light", 1280], ["dark", 390]]) {
    await page.setViewportSize({ width, height: 900 }); await page.evaluate((theme) => { document.documentElement.dataset.theme = theme; }, theme);
    assert.equal(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth), true);
    await page.screenshot({ path: `test-results/instance-edit-${theme}-${width}.png`, fullPage: true });
  }
  await open(); await page.evaluate(() => { window.mode = "read_failure"; });
  await page.getByRole("alert").waitFor();
  assert.equal(await page.getByRole("button", { name: "編集", exact: true }).isDisabled(), true);
  }
  assert.deepEqual(errors, []);
  console.log("Instance actions/edit: lifecycle and rename regressions, separate port results, fresh state guards, old/new reservations, stopped completion, receipt retry/navigation, immutable masked settings and responsive themes passed.");
} finally { await browser?.close(); server.kill(); }
