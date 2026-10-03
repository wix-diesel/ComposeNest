import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { readFile, mkdir } from "node:fs/promises";
import { chromium } from "playwright";

// Rust's example supplies masked previews from real create/clone Version switches.
const fixtures = JSON.parse(await readFile("test-results/template-form-plans.json", "utf8"));
const server = spawn(process.execPath, ["node_modules/vite/bin/vite.js", "--host", "127.0.0.1", "--port", "4174", "--strictPort"], { stdio: "pipe" });
let browser;
try {
  const deadline = Date.now() + 20000;
  while (true) {
    if (server.exitCode !== null) throw new Error("Vite exited");
    try { if ((await fetch("http://127.0.0.1:4174", { signal: AbortSignal.timeout(1000) })).ok) break; } catch { /* Local server startup. */ }
    if (Date.now() > deadline) throw new Error("Vite startup timed out");
    await new Promise((resolve) => setTimeout(resolve, 100));
  }
  browser = await chromium.launch({ headless: true, ...(process.env.COMPOSENEST_TEST_BROWSER ? { executablePath: process.env.COMPOSENEST_TEST_BROWSER } : {}) });
  let page;
  const errors = [];
  async function open(kind, extra = false) {
    await page?.close();
    page = await browser.newPage({ viewport: { width: 1280, height: 900 } });
    page.on("pageerror", (error) => errors.push(error.message));
    const initial = structuredClone(fixtures[kind]);
    if (extra === true && kind === "clone") {
      const input = initial.inputs.find((item) => item.key === "password");
      input.policy = "ask"; input.origin = "inherited"; input.changed = false; input.needsSecretConfirmation = true;
      initial.concerns.push({ code: "SECRET_REUSE_NEEDS_CONFIRMATION", fieldPath: "inputs.password" });
    }
    if (extra === "ask") initial.concerns.push({ code: "VERSION_NEEDS_ANSWER", fieldPath: "version" }, { code: "STORAGE_NEEDS_ANSWER", fieldPath: "storageMethod" });
    if (extra && kind === "create") {
      initial.templateForm.inputs.push({ key: "count", label: "件数", inputType: "integer", required: false, description: "整数テスト", validation: { min: -10, max: 10 }, options: [], canGenerate: false });
      initial.inputs.push({ key: "count", definition: {}, value: 0, hasSecret: false });
      initial.inputs.find((input) => input.key === "removed").value = false;
      initial.inputs.find((input) => input.key === "retained").value = "";
    }
    await page.addInitScript((fixture) => { window.formFixture = fixture; }, { initial: { kind, view: initial }, next: { kind, view: fixtures[`${kind}Next`] } });
    await page.goto("http://127.0.0.1:4174/scripts/template-form.html");
    await page.getByRole("heading", { name: kind === "clone" ? "変更内容の確認" : "基本設定", exact: true }).waitFor();
  }
  await open("create", true);
  assert.deepEqual(await page.locator('[data-field-path^="inputs."]').evaluateAll((items) => items.map((item) => item.dataset.fieldPath)), ["inputs.retained", "inputs.removed", "inputs.password", "inputs.count"]);
  assert.equal(await page.locator("#input-count").inputValue(), "0");
  assert.equal(await page.locator("#input-removed").inputValue(), "false");
  assert.equal(await page.locator("#input-retained").inputValue(), "");
  await page.getByText("空文字を設定。", { exact: true }).waitFor();
  await page.getByRole("button", { name: "件数を未設定にする" }).click();
  assert.equal(await page.locator("#input-count").inputValue(), "");
  await page.locator("#input-count").fill("0");
  await page.locator("#input-removed").selectOption("false");
  await page.locator("#input-retained").fill("kept");
  await page.locator("#input-retained").fill("");
  await page.getByRole("button", { name: "入力を反映" }).click();
  await page.waitForFunction(() => window.formRequests.length === 1 && !document.querySelector("fieldset").disabled);
  assert.deepEqual(await page.evaluate(() => window.formRequests[0].inputs), { count: 0, removed: false, retained: "" });
  await page.locator("#input-count").fill("1.5");
  await page.getByRole("button", { name: "入力を反映" }).click();
  await page.waitForFunction(() => window.formRequests.length === 2 && !document.querySelector("fieldset").disabled);
  assert.equal(await page.evaluate(() => window.formRequests[1].inputs.count), "1.5", "invalid integer must reach Core unchanged");
  await page.getByRole("radio", { name: /Docker管理/ }).check();
  await page.locator(".summary-list dd").getByText("Docker管理").waitFor();
  await page.getByText("Dockerが管理する新しい専用volumeを割り当てます。").waitFor();
  await page.locator("#input-password").fill("OnlyEntered-Secret");
  assert.equal(await page.locator("#input-password").getAttribute("type"), "password");
  await page.getByRole("button", { name: "Passwordを表示", exact: true }).click();
  assert.equal(await page.locator("#input-password").getAttribute("type"), "text");
  assert.equal(await page.locator("aside").getByText("OnlyEntered-Secret").count(), 0);
  assert.equal(await page.evaluate(() => JSON.stringify(localStorage).includes("OnlyEntered-Secret") || location.href.includes("OnlyEntered-Secret")), false);
  await page.locator("#service-version").selectOption("2");
  await page.locator("#input-added").waitFor();
  const changes = await page.evaluate(() => window.formRequests.slice(-2));
  assert.equal(changes[0].storageMethod, "volume");
  assert.equal(changes[1].version, "2");
  assert.equal(changes[1].expectedRevision, changes[0].expectedRevision + 1);
  assert.deepEqual(changes[1].inputs, {}, "Version switch must not submit removed keys");
  assert.equal(await page.locator("#input-removed").count(), 0);
  assert.equal(await page.locator("#input-count").count(), 0);
  assert.equal(await page.locator("#input-password").getAttribute("type"), "password");
  assert.equal(await page.locator("#input-added").inputValue(), "green", "new create input uses Core initialization");
  await page.locator('[data-field-path="inputs.retained"]').getByText("最小文字数: 6").waitFor();
  await page.locator("#input-added").selectOption("blue");
  await page.evaluate(() => { window.formDelay = 100; });
  const before = await page.evaluate(() => window.formRequests.length);
  await page.getByRole("button", { name: "入力を反映" }).evaluate((button) => { button.click(); button.click(); });
  await page.waitForFunction((count) => window.formRequests.length === count + 1 && !document.querySelector("fieldset").disabled, before);
  assert.equal(await page.evaluate(() => window.formRequests.length), before + 1);
  await page.evaluate(() => { window.formFailure = true; });
  await page.locator("#input-retained").fill("retry");
  await page.getByRole("button", { name: "入力を反映" }).click();
  await page.getByRole("alert").waitFor();
  assert.equal(await page.locator("#input-retained").inputValue(), "retry");
  assert.equal(await page.locator("#input-retained").getAttribute("aria-invalid"), "true");
  await open("clone", true);
  assert.deepEqual(await page.locator("#service-version option").allTextContents(), ["1", "2"]);
  await page.getByRole("heading", { name: "複製元の環境" }).waitFor();
  assert.equal(await page.getByRole("radio", { name: /ホストフォルダー/ }).isChecked(), true);
  assert.match(await page.locator('[data-diff-path="ports.db"]').textContent(), /5432/);
  assert.match(await page.locator('[data-diff-path="inputs.password"]').textContent(), /非表示/);
  await page.getByRole("button", { name: "秘密の引継ぎを確認", exact: true }).click();
  await page.waitForFunction(() => window.formRequests.length === 1 && !document.querySelector("fieldset").disabled);
  assert.deepEqual(await page.evaluate(() => window.formRequests[0].confirmSecrets), ["password"]);
  assert.deepEqual(await page.evaluate(() => window.formRequests[0].inputs), {});
  await open("clone", "ask");
  await page.getByRole("button", { name: "バージョン選択を確認", exact: true }).click();
  await page.waitForFunction(() => window.formRequests.length === 1 && !document.querySelector("fieldset").disabled);
  assert.equal(await page.locator("#service-version").inputValue(), "1");
  assert.equal(await page.evaluate(() => window.formRequests[0].version), "1");
  await page.getByRole("button", { name: "現在の保存方式を確認", exact: true }).click();
  await page.getByRole("button", { name: "入力を反映", exact: true }).click();
  await page.waitForFunction(() => window.formRequests.length === 2 && !document.querySelector("fieldset").disabled);
  assert.equal(await page.evaluate(() => window.formRequests[1].storageMethod), "bind");
  assert.deepEqual(await page.evaluate(() => window.formRequests[1].confirmSecrets), []);
  await open("clone");
  await page.locator("#service-version").selectOption("2");
  await page.locator("#input-added").waitFor();
  for (const path of ["inputs.removed", "ports.db", "storage.data"]) {
    const row = page.locator(`[data-diff-path="${path}"]`);
    assert.match(await row.textContent(), /削除/);
    assert.equal(await row.locator("input, select, button").count(), 0);
  }
  for (const path of ["inputs.added", "ports.api", "storage.cache"]) assert.match(await page.locator(`[data-diff-path="${path}"]`).textContent(), /追加/);
  assert.match(await page.locator('[data-diff-path="inputs.password"]').textContent(), /入力待ち/);
  assert.equal(await page.locator("#input-added").inputValue(), "", "missing Clone copy must stay unset despite a Template default");
  await page.locator('[data-field-path="inputs.added"]').getByText(/入力待ち/).waitFor();
  await page.locator("#input-added").selectOption("blue");
  await page.getByRole("button", { name: "入力を反映" }).click();
  await page.waitForFunction(() => window.formRequests.length === 2 && !document.querySelector("fieldset").disabled);
  assert.deepEqual(await page.evaluate(() => window.formRequests[1].inputs), { added: { action: "input", value: "blue" } });
  assert.equal(await page.getByRole("button", { name: "Passwordを再生成", exact: true }).count(), 0, "incompatible generator requires manual input");
  await page.getByRole("button", { name: "Freshを再生成", exact: true }).click();
  await page.waitForFunction(() => window.formRequests.length === 3 && !document.querySelector("fieldset").disabled);
  assert.deepEqual(await page.evaluate(() => window.formRequests[2].inputs), { fresh: { action: "generate" } });
  await mkdir("test-results", { recursive: true });
  for (const theme of ["light", "dark"]) for (const width of [1280, 390]) {
    await page.evaluate((theme) => { document.documentElement.dataset.theme = theme; }, theme);
    await page.setViewportSize({ width, height: 900 });
    assert.equal(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth), true, `${theme}/${width} overflow`);
    await page.locator("#display-name").fill(`${theme}-${width}`);
    await page.keyboard.press("Tab");
    assert.equal(await page.evaluate(() => document.activeElement.id), "service-version");
    await page.screenshot({ path: `test-results/template-form-${theme}-${width}.png`, fullPage: true });
  }
  assert.deepEqual(errors, []);
  console.log("Template form: Core previews, five types, unset/0/false/empty, Version changes, Clone input wait, errors, revision/double-click, secret masking, both themes and narrow width passed.");
} finally { await browser?.close(); server.kill(); }
