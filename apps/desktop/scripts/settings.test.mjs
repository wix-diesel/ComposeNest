import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { mkdir } from "node:fs/promises";
import { chromium } from "playwright";

const origin = "http://127.0.0.1:4183";
const server = spawn(process.execPath, ["node_modules/vite/bin/vite.js", "--host", "127.0.0.1", "--port", "4183", "--strictPort"], { stdio: "pipe" });
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
  const calls = []; let stored = "bind"; let mode = "normal"; let finishSave;
  const root = "/actual/host/management/root/with/a/long/directory/name";
  // The simulated backend lives outside the document so reload cannot reset it.
  await page.exposeFunction("settingsTransport", async (command, request) => {
    calls.push({ command, request });
    const context = request.context ?? request;
    const response = (result) => ({ apiVersion: 1, requestId: context.requestId, error: null, result });
    if (command === "get_bootstrap") return response({ applicationTitle: "ComposeNest", startedAtUnixSeconds: 1 });
    if (command === "list_instances") return response([]);
    assert.ok(["get_settings", "save_settings"].includes(command));
    if (command === "save_settings") {
      assert.deepEqual(Object.keys(request).sort(), ["context", "storageMethod"]);
      if (mode === "delay") await new Promise((resolve) => { finishSave = resolve; });
    }
    if (mode === "failure") return { ...response(null), error: { code: "STORE_UNAVAILABLE", reason: "private-password-and-database-detail" } };
    if (command === "save_settings") stored = request.storageMethod;
    const reply = response({ storageMethod: stored, managementRoot: root });
    if (mode === "wrong_id") reply.requestId = "another-request";
    if (mode === "wrong_version") reply.apiVersion = 2;
    if (mode === "null") reply.result = null;
    if (mode === "invalid_method") reply.result.storageMethod = "arbitrary";
    if (mode === "empty_root") reply.result.managementRoot = "";
    if (mode === "mismatched_method") reply.result.storageMethod = stored === "bind" ? "volume" : "bind";
    return reply;
  });
  await page.addInitScript(() => {
    window.preferenceWrites = [];
    const write = Storage.prototype.setItem;
    Storage.prototype.setItem = function (key, value) { window.preferenceWrites.push([key, value]); return write.call(this, key, value); };
    window.__TAURI_INTERNALS__ = { invoke: (command, { request }) => window.settingsTransport(command, request) };
  });
  await page.goto(origin + "/#/settings");
  const bind = page.getByRole("radio", { name: /ホストフォルダー/ });
  const volume = page.getByRole("radio", { name: /Docker管理/ });
  const save = page.getByRole("button", { name: "設定を保存", exact: true });
  const reload = page.getByRole("button", { name: "保存済み設定を再読込み", exact: true });
  await page.getByText(root, { exact: true }).waitFor();
  assert.equal(await bind.isChecked(), true);
  for (const text of ["認証情報は暗号化せず保存", "日本語", "手動更新", "アプリを閉じても、起動済みの環境は停止しません。", "Apache-2.0"]) assert.ok(await page.getByText(text, { exact: true }).count());
  assert.equal(await page.getByRole("textbox").count(), 0, "management root cannot be edited");
  await volume.check();
  assert.equal(stored, "bind", "draft does not save immediately");
  mode = "delay"; await save.click();
  await page.getByRole("button", { name: "保存中…", exact: true }).waitFor();
  assert.equal(stored, "bind");
  assert.equal(await bind.isDisabled(), true);
  assert.equal(await reload.isDisabled(), true);
  assert.equal(await page.getByRole("status").count(), 0, "no optimistic success");
  await page.waitForFunction(() => document.querySelector('button[type="submit"]')?.disabled);
  assert.equal(typeof finishSave, "function"); finishSave(); mode = "normal";
  await page.getByRole("status").filter({ hasText: "設定を保存しました" }).waitFor();
  assert.equal(stored, "volume");
  assert.equal(calls.filter(({ command }) => command === "save_settings").length, 1);
  await page.reload(); await page.getByText(root, { exact: true }).waitFor();
  assert.equal(await volume.isChecked(), true, "restores backend settings after reload");
  await page.locator(".sidebar").getByRole("button", { name: "環境一覧", exact: true }).click();
  await page.locator(".sidebar").getByRole("button", { name: "設定", exact: true }).click();
  await page.getByText(root, { exact: true }).waitFor();
  assert.equal(await volume.isChecked(), true);

  for (const invalid of ["failure", "wrong_id", "wrong_version", "null", "invalid_method", "empty_root", "mismatched_method"]) {
    const before = stored; await (before === "bind" ? volume : bind).check();
    mode = invalid; await save.click();
    await page.getByRole("alert").filter({ hasText: "設定の保存を確認できませんでした" }).waitFor();
    assert.equal(await page.getByRole("status").count(), 0);
    assert.equal(await page.getByText("private-password-and-database-detail", { exact: false }).count(), 0);
    if (invalid === "failure") assert.equal(stored, before);
    mode = "normal"; await reload.click();
    await page.getByRole("alert").filter({ hasText: "設定の保存を確認できませんでした" }).waitFor({ state: "detached" });
    await page.getByText(root, { exact: true }).waitFor();
    assert.equal(await (stored === "bind" ? bind : volume).isChecked(), true);
  }
  for (const invalid of ["failure", "wrong_id", "wrong_version", "null", "invalid_method", "empty_root"]) {
    mode = invalid; await reload.click();
    await page.getByRole("alert").filter({ hasText: "設定を取得できませんでした" }).waitFor();
    assert.equal(await save.isDisabled(), true);
    assert.equal(await bind.isChecked(), false);
    assert.equal(await volume.isChecked(), false);
    assert.equal(await page.getByText(root, { exact: true }).count(), 0);
    mode = "normal"; await reload.click(); await page.getByText(root, { exact: true }).waitFor();
  }
  await bind.check(); await save.click();
  await page.getByRole("status").filter({ hasText: "設定を保存しました" }).waitFor();
  const saves = calls.filter(({ command }) => command === "save_settings").length;
  await page.getByRole("button", { name: "通知を閉じる", exact: true }).click();
  assert.equal(calls.filter(({ command }) => command === "save_settings").length, saves, "dismissal does not submit settings");
  for (const [theme, width] of [["light", 1280], ["dark", 390]]) {
    await page.setViewportSize({ width, height: 900 });
    await page.locator(".display-settings").getByRole("button", { name: theme === "light" ? "ライト" : "ダーク", exact: true }).click();
    assert.equal(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth), true);
    await mkdir("test-results", { recursive: true }); await page.screenshot({ path: `test-results/settings-${theme}-${width}.png`, fullPage: true });
  }
  const writes = await page.evaluate(() => window.preferenceWrites);
  assert.ok(writes.every(([key, value]) => key === "composenest-theme" && ["light", "dark"].includes(value)));
  assert.deepEqual(await page.evaluate(() => Object.keys(localStorage)), ["composenest-theme"]);
  assert.ok(calls.every(({ command }) => ["get_bootstrap", "get_settings", "save_settings", "list_instances"].includes(command)));
  assert.deepEqual(errors, []);
  console.log("Settings: readback, delayed/failing saves, restart/navigation, invalid responses, retry, immutable host root, localStorage isolation and responsive themes passed.");
} finally { await browser?.close(); server.kill(); }
