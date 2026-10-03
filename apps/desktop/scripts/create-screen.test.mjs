import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { readFile } from "node:fs/promises";
import { chromium } from "playwright";

const clone = process.argv.includes("--clone");
const fixture = JSON.parse(await readFile("test-results/template-form-plans.json", "utf8"))[clone ? "clone" : "create"];
const server = spawn(process.execPath, ["node_modules/vite/bin/vite.js", "--host", "127.0.0.1", "--port", "4175", "--strictPort"], { stdio: "pipe" });
let browser;
try {
  const deadline = Date.now() + 20000;
  while (true) {
    if (server.exitCode !== null) throw new Error("Vite exited");
    try { if ((await fetch("http://127.0.0.1:4175", { signal: AbortSignal.timeout(1000) })).ok) break; } catch { /* Local server startup. */ }
    if (Date.now() > deadline) throw new Error("Vite startup timed out");
    await new Promise((resolve) => setTimeout(resolve, 100));
  }
  browser = await chromium.launch({ headless: true, ...(process.env.COMPOSENEST_TEST_BROWSER ? { executablePath: process.env.COMPOSENEST_TEST_BROWSER } : {}) });
  let page;
  const errors = [];
  const selected = "selected-registered-revision";
  const hash = clone ? `#/instance-clone?instanceId=${selected}` : `#/instance-create?templateId=${selected}&returnTo=templates`;
  async function open(mode = "normal") {
    await page?.close();
    page = await browser.newPage({ viewport: { width: 1280, height: 1000 } });
    page.on("pageerror", (error) => errors.push(error.message));
    await page.addInitScript(({ fixture, mode, clone }) => {
      window.createMode = mode;
      window.calls = []; window.receiptFailure = false; window.acceptedReceipt = null;
      window.plan = { ...fixture, displayName: "", concerns: [{ code: "DISPLAY_NAME_INVALID", fieldPath: "displayName" }] };
      window.__TAURI_INTERNALS__ = { invoke: async (command, { request }) => {
        window.calls.push({ command, request: structuredClone(request) });
        command = command.replace("clone", "create");
        const context = request.context ?? request;
        const response = (result, error = null) => ({ apiVersion: 1, requestId: context.requestId, result, error });
        if (command === "get_bootstrap") return response({ applicationTitle: "ComposeNest", startedAtUnixSeconds: 1 });
        if (command === "prepare_create") {
          if (window.createMode === "prepare_failure") throw new Error("unavailable");
          if (clone) { window.plan.sourceId = request.sourceId; window.sourceUpdated = false; }
          else window.plan.templateRevisionId = request.templateRevisionId;
          return response(structuredClone(window.plan));
        }
        if (command === "update_create_plan") {
          if (request.edit.expectedRevision !== window.plan.planRevision) throw new Error("stale test edit");
          if (request.edit.displayName !== null) window.plan.displayName = request.edit.displayName;
          window.plan.concerns = window.plan.displayName ? [] : [{ code: "DISPLAY_NAME_INVALID", fieldPath: "displayName" }];
          window.plan.planRevision++;
          return response(structuredClone(window.plan));
        }
        if (command === "view_create_plan") {
          if (window.sourceUpdated) return response(null, { code: "PLAN_STALE", reason: "複製元を再確認してください。", fieldPath: "sourceId" });
          window.plan.ports = Object.fromEntries(Object.entries(window.plan.ports).map(([key]) => [key, 12789]));
          return response(structuredClone(window.plan));
        }
        if (command === "get_create_receipt") {
          if (window.receiptFailure) throw new Error("lookup disconnected");
          return response(window.acceptedReceipt);
        }
        if (command === "discard_create_plan") return response(null);
        if (command === "confirm_create") {
          if (window.createMode === "lost") throw new Error("acceptance disconnected");
          if (window.createMode === "rejected") return response(null, { code: "PLAN_RECONFIRM", reason: "ポートを再確認してください。", fieldPath: "ports", retryability: "notRetryable", operationId: null, safeDetails: null });
          if (window.createMode === "delay") await new Promise((resolve) => setTimeout(resolve, 200));
          window.acceptedReceipt = { planId: request.planId, confirmedRevision: request.revision, operationId: "accepted-operation", instanceId: "a".repeat(32) };
          if (window.createMode === "accepted_loss") throw new Error("response disconnected");
          return response(window.acceptedReceipt);
        }
        throw new Error(`Unexpected command: ${command}`);
      } };
    }, { fixture, mode, clone });
    await page.goto(`http://127.0.0.1:4175/${hash}`);
    if (mode !== "prepare_failure") await page.locator("#display-name").waitFor();
  }
  async function review() {
    await page.locator("#display-name").fill("My database");
    await page.getByRole("button", { name: clone ? "複製内容を確認" : "作成内容を確認", exact: true }).click();
    await page.getByRole("dialog").waitFor();
  }
  async function confirm() {
    for (const box of await page.getByRole("dialog").getByRole("checkbox").all()) await box.check();
    await page.getByRole("button", { name: "作成・起動を確定", exact: true }).click();
  }
  async function calls(command) { return page.evaluate((command) => window.calls.filter((call) => call.command === command), clone ? command.replace("create", "clone") : command); }

  await open();
  assert.equal((await calls("prepare_create")).length, 1, "StrictMode must not prepare twice");
  assert.equal((await calls("prepare_create"))[0].request[clone ? "sourceId" : "templateRevisionId"], selected);
  await page.getByRole("button", { name: clone ? "複製内容を確認" : "作成内容を確認", exact: true }).click();
  await page.getByText("未回答・入力エラー・ポートの確認事項を解消してください。").waitFor();
  assert.equal(await page.getByRole("dialog").count(), 0);
  await page.locator("#input-password").fill("OnlyUserEnteredSecret");
  await review();
  assert.equal((await calls("update_create_plan"))[0].request.edit.expectedRevision, 1);
  assert.equal(await page.getByRole("button", { name: "作成・起動を確定" }).isDisabled(), true);
  assert.match(await page.getByRole("dialog").textContent(), /127\.0\.0\.1:12789/);
  assert.doesNotMatch(await page.getByRole("dialog").textContent(), /OnlyUserEnteredSecret/);
  for (const [theme, width] of [["light", 1280], ["dark", 390]]) {
    await page.setViewportSize({ width, height: 1000 });
    await page.evaluate((theme) => { document.documentElement.dataset.theme = theme; }, theme);
    assert.equal(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth), true);
    await page.screenshot({ path: `test-results/create-review-${theme}-${width}.png`, fullPage: true });
  }
  await page.keyboard.press("Escape");
  assert.equal((await calls("confirm_create")).length, 0);
  await page.getByRole("button", { name: "作成を取り消す", exact: true }).click();
  await page.waitForURL(clone ? /#\/instances$/ : /#\/templates$/);
  assert.equal((await calls("discard_create_plan")).length, 1);
  assert.equal((await calls("confirm_create")).length, 0);

  await open("accepted_loss"); await review(); await confirm();
  await page.waitForURL(/operationId=accepted-operation/);
  assert.equal((await calls("get_create_receipt")).length, 1);
  const confirmed = (await calls("confirm_create"))[0].request;
  assert.equal(confirmed.revision, 2);
  assert.equal(confirmed.acceptPlaintext, true);
  if (clone) assert.equal(confirmed.acceptConfigurationOnly, true);
  assert.deepEqual(confirmed.confirmedPorts, { db: 12789 });
  assert.doesNotMatch(page.url(), /OnlyUserEnteredSecret|planId|requestId/);

  await open("lost"); await review();
  await page.evaluate(() => { window.receiptFailure = true; });
  await confirm(); await page.getByText("接続を確認できませんでした。同じプランの受付結果を再確認してください。").waitFor();
  assert.equal(await page.locator("#display-name").isDisabled(), true);
  assert.equal(await page.getByRole("button", { name: "作成を取り消す" }).isDisabled(), true);
  await page.evaluate(() => { window.receiptFailure = false; location.hash = "#/instances"; });
  await page.getByRole("heading", { name: "環境一覧", exact: true }).waitFor();
  await page.evaluate((hash) => { location.hash = hash; }, hash);
  await page.getByText("受付記録はまだありません。同じ内容・同じ要求IDで再送できます。").waitFor();
  assert.equal((await calls("prepare_create")).length, 1, "reopening must reconcile the original plan");
  await page.evaluate(() => { window.createMode = "delay"; });
  const resend = page.getByRole("button", { name: "同じ内容で再送" });
  await resend.evaluate((button) => { button.click(); button.click(); });
  await page.waitForURL(/operationId=accepted-operation/);
  const submissions = await calls("confirm_create");
  assert.equal(submissions.length, 2);
  assert.deepEqual(submissions[0].request, submissions[1].request);

  await open("delay"); await review(); await confirm();
  await page.evaluate(() => { location.hash = "#/instances"; });
  await page.getByRole("heading", { name: "環境一覧", exact: true }).waitFor();
  await page.waitForFunction(() => window.acceptedReceipt !== null);
  await page.evaluate((hash) => { location.hash = hash; }, hash);
  await page.waitForURL(/operationId=accepted-operation/);
  assert.equal((await calls("prepare_create")).length, 1);

  await open("rejected"); await review(); await confirm();
  await page.getByText("ポートを再確認してください。", { exact: true }).waitFor();
  assert.equal(await page.locator("#display-name").isDisabled(), false);
  assert.equal((await calls("prepare_create")).length, 1);
  await open("prepare_failure"); await page.getByRole("alert").waitFor();
  assert.equal((await calls("confirm_create")).length, 0);
  if (clone) {
    await open(); await review();
    const boxes = page.getByRole("dialog").getByRole("checkbox");
    assert.equal(await boxes.count(), 2);
    await boxes.nth(1).check();
    assert.equal(await page.getByRole("button", { name: "作成・起動を確定" }).isDisabled(), true);
    await page.keyboard.press("Escape");
    await page.evaluate(() => { window.sourceUpdated = true; });
    await page.getByRole("button", { name: "複製内容を確認", exact: true }).click();
    await page.getByRole("button", { name: "複製元を読み直す" }).waitFor();
    assert.equal(await page.getByRole("dialog").count(), 0);
    assert.equal(await page.locator("#display-name").isDisabled(), true);
    await page.getByRole("button", { name: "複製元を読み直す" }).click();
    await page.waitForFunction(() => !document.querySelector("#display-name").disabled);
    assert.equal((await calls("prepare_create")).length, 2);
    assert.equal((await calls("discard_create_plan")).length, 1);
    await review();
    for (const box of await page.getByRole("dialog").getByRole("checkbox").all()) assert.equal(await box.isChecked(), false);
  }
  assert.deepEqual(errors, []);
  console.log(`${clone ? "Clone" : "Create"}: selection, current revision/ports, consent, cancellation, lost response/receipt, same-request resend, navigation recovery and themes passed.`);
} finally { await browser?.close(); server.kill(); }
