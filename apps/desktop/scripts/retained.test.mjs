import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { mkdir } from "node:fs/promises";
import { chromium } from "playwright";

const origin = "http://127.0.0.1:4184";
const server = spawn(process.execPath, ["node_modules/vite/bin/vite.js", "--host", "127.0.0.1", "--port", "4184", "--strictPort"], { stdio: "pipe" });
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
  const oldTime = "2026-09-20 01:02:03"; const newTime = "2026-10-06 13:40:00";
  const fixture = (id, name, method, states) => ({
    instance: { id, name, lifecycle: "retired", selectedVersion: "17", specRevision: 3,
      inputs: [{ slot: "password", secret: true, value: null }] },
    serviceName: "Custom database", deletedAt: "2026-09-19 23:50:00", settingsDatabase: "/real/management/state/composenest.sqlite",
    snapshotId: `snapshot-${id}`, artifacts: [
      { id: `artifact-${id}`, specRevision: 3, placement: "retained", directory: `/real/management/instances/${id}/artifacts/${id}` },
      { id: `staged-${id}`, specRevision: 2, placement: "staged", directory: null }],
    locations: states.map((state, index) => ({ storage: { slot: `data${index}`, method, presence: state, initialization: "unknown" },
      location: method === "volume" ? `cn-${id}-data${index}` : `/real/management/data/${id}/data${index}/long-path-segment`,
      ownership: "retained", ownershipVerified: state === "present", observedAt: state === "not_materialized" ? null : oldTime })),
  });
  let stored = [fixture("a", "旧検証用DB", "bind", ["present", "missing"]), fixture("b", "キャッシュ検証", "volume", ["unverified", "not_materialized"])];
  stored[1].instance.inputs[0].value = "never-render-original-inputs";
  const calls = []; let mode = "normal"; const finish = new Map();
  await page.exposeFunction("retainedTransport", async (command, request) => {
    calls.push({ command, request });
    const context = request.context ?? request;
    const response = (result) => ({ apiVersion: 1, requestId: context.requestId, error: null, result });
    if (command === "get_bootstrap") return response({ applicationTitle: "ComposeNest", startedAtUnixSeconds: 1 });
    if (command === "list_instances") return response([]);
    assert.ok(["list_retained_storage", "refresh_retained_storage"].includes(command), command);
    if (command === "list_retained_storage") {
      assert.deepEqual(Object.keys(request).sort(), ["apiVersion", "requestId"]);
      const reply = response(structuredClone(stored));
      if (mode === "list_failure") return { ...reply, result: null, error: { reason: "private-backend-secret" } };
      if (mode === "list_wrong_id") reply.requestId = "another-request";
      if (mode === "list_wrong_version") reply.apiVersion = 2;
      if (mode === "list_null") reply.result = null;
      return reply;
    }
    assert.deepEqual(Object.keys(request).sort(), ["context", "instanceId"]);
    if (mode === "delay") await new Promise((resolve) => { finish.set(request.instanceId, resolve); });
    if (mode === "partial" && request.instanceId === "b") throw new Error("private-backend-secret");
    const item = structuredClone(stored.find((item) => item.instance.id === request.instanceId));
    item.locations.forEach((location) => { location.storage.presence = "present"; location.ownershipVerified = true; location.observedAt = newTime; });
    const reply = response(item);
    if (mode === "wrong_id") reply.requestId = "another-request";
    if (mode === "wrong_version") reply.apiVersion = 2;
    if (mode === "wrong_instance") reply.result.instance.id = "another-instance";
    if (mode === "null") reply.result = null;
    if (mode === "failure") return { ...reply, result: null, error: { reason: "private-backend-secret" } };
    return reply;
  });
  await page.addInitScript(() => { window.__TAURI_INTERNALS__ = { invoke: (command, { request }) => window.retainedTransport(command, request) }; });
  await page.goto(origin + "/#/retained");
  const section = page.getByRole("region", { name: "削除した環境の保存領域", exact: true });
  const refresh = page.getByRole("button", { name: "状態を確認", exact: true });
  const status = (text) => section.getByText(text, { exact: true });
  const settled = () => page.waitForFunction(() => document.querySelector(".retained-storage")?.getAttribute("aria-busy") === "false");
  const pending = async () => {
    const deadline = Date.now() + 5000;
    while (finish.size !== 2) {
      if (Date.now() > deadline) throw new Error("Rechecks did not reach the transport");
      await new Promise((resolve) => setTimeout(resolve, 20));
    }
  };
  await status("保持済み (Present)").waitFor();
  for (const label of ["見つかりません (Missing)", "確認できません (Unverified)", "未作成 (NotMaterialized)"]) assert.equal(await status(label).count(), 1);
  assert.equal(await status("4 件").count(), 1, "counts allocations, including multiple slots");
  assert.equal(await status("最終観測: 未観測").count(), 1);
  assert.equal(calls.filter(({ command }) => command === "refresh_retained_storage").length, 0, "opening only reads saved observations");
  for (const text of ["snapshot-a", "snapshot-b", "/real/management/state/composenest.sqlite", "/real/management/instances/a/artifacts/a", "公開先の記録なし"]) assert.ok(await page.getByText(text, { exact: true }).count());
  assert.equal(await page.getByText("never-render-original-inputs", { exact: false }).count(), 0);
  assert.equal(await page.getByRole("link").filter({ hasText: /cn-b-data/ }).count(), 0);
  assert.equal(await page.getByRole("button", { name: /復元|完全削除|再利用|フォルダーを開く/ }).count(), 0);
  for (const [theme, width] of [["ライト", 1280], ["ダーク", 390]]) {
    await page.setViewportSize({ width, height: 900 }); await page.getByRole("button", { name: theme, exact: true }).click();
    assert.equal(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth), true);
    await mkdir("test-results", { recursive: true }); await page.screenshot({ path: `test-results/retained-${width}.png`, fullPage: true });
  }
  mode = "delay"; await refresh.click(); await page.getByRole("status").filter({ hasText: "状態を確認中" }).waitFor();
  assert.equal(await refresh.isDisabled(), true);
  await pending();
  assert.equal(await status("最終観測: " + newTime + " UTC").count(), 0, "does not report optimistic observations");
  assert.equal(finish.size, 2); for (const resolve of finish.values()) resolve(); mode = "normal";
  await status("最終観測: " + newTime + " UTC").first().waitFor();
  await settled();
  assert.equal(await status("保持済み (Present)").count(), 4);
  await page.reload(); await status("見つかりません (Missing)").waitFor();
  assert.equal(calls.filter(({ command }) => command === "refresh_retained_storage").length, 2, "reload does not recheck");

  mode = "partial"; await refresh.click(); await page.getByRole("alert").filter({ hasText: "一部の状態を確認できませんでした" }).waitFor();
  await settled();
  assert.equal(await status("最終観測: " + newTime + " UTC").count(), 2);
  assert.equal(await status("再確認失敗・前回の観測").count(), 2);
  for (const invalid of ["wrong_id", "wrong_version", "wrong_instance", "null", "failure"]) {
    mode = invalid; await refresh.click(); await page.getByRole("alert").filter({ hasText: "一部の状態を確認できませんでした" }).waitFor();
    await settled();
    assert.equal(await status("再確認失敗・前回の観測").count(), 4);
    assert.equal(await status("最終観測: " + newTime + " UTC").count(), 0);
  }
  for (const invalid of ["list_failure", "list_wrong_id", "list_wrong_version", "list_null"]) {
    mode = invalid; await refresh.click(); await page.getByRole("alert").filter({ hasText: "一覧を取得できませんでした" }).waitFor();
    await settled();
    assert.equal(await status("4 件").count(), 1);
    assert.equal(await status("見つかりません (Missing)").count(), 1);
  }
  mode = "normal"; await refresh.click(); await status("最終観測: " + newTime + " UTC").first().waitFor();
  await page.getByRole("alert").waitFor({ state: "detached" });
  // Ignore a recheck result after navigating away, including its old success time.
  mode = "delay"; finish.clear(); await refresh.click();
  await page.getByRole("status").filter({ hasText: "状態を確認中" }).waitFor(); await pending();
  await page.locator(".sidebar").getByRole("button", { name: "環境一覧", exact: true }).click();
  mode = "normal"; for (const resolve of finish.values()) resolve();
  await page.locator(".sidebar").getByRole("button", { name: "保持データ", exact: true }).click();
  await status("見つかりません (Missing)").waitFor();
  assert.equal(await status("最終観測: " + newTime + " UTC").count(), 0);
  stored[0].locations[0].ownershipVerified = false;
  stored[0].locations[1].storage.presence = "future_unknown_state";
  await page.reload(); await status("確認できません (Unverified)").first().waitFor();
  assert.equal(await status("保持済み (Present)").count(), 0, "unverified ownership and unknown states never claim retention");
  stored = [fixture("c", "保存領域なし", "bind", [])]; await page.reload();
  await page.getByText("削除した環境に保存領域の割当てはありません。元の設定の所在を確認できます。", { exact: true }).waitFor();
  assert.equal(await page.getByText("snapshot-c", { exact: true }).count(), 1);
  stored = []; await page.reload(); await page.getByText("保持データはありません。", { exact: true }).waitFor();
  const checks = calls.filter(({ command }) => command === "refresh_retained_storage").length;
  await refresh.click(); await settled();
  assert.equal(calls.filter(({ command }) => command === "refresh_retained_storage").length, checks);
  mode = "list_failure"; await page.reload(); await page.getByRole("alert").filter({ hasText: "一覧を取得できませんでした" }).waitFor();
  assert.equal(await page.getByText("保持データはありません。", { exact: true }).count(), 0, "failure is not an empty result");
  mode = "normal"; await refresh.click(); await page.getByText("保持データはありません。", { exact: true }).waitFor();
  assert.equal(await page.getByText("private-backend-secret", { exact: false }).count(), 0);
  assert.deepEqual(errors, []);
  console.log("Retained storage: states, scoped identities, recorded references, explicit/delayed rechecks, partial failures, invalid replies, retry, navigation, empty data and themes passed.");
} finally { await browser?.close(); server.kill(); }
