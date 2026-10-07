import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { createServer } from "node:http";
import { mkdir, readFile, writeFile } from "node:fs/promises";
import { chromium } from "playwright";
import { installValidationFixture } from "./ui-validation-fixture.mjs";

const origin = "http://127.0.0.1:4186";
const output = "test-results/ui-validation";
const plans = JSON.parse(await readFile("test-results/template-form-plans.json", "utf8"));
const screens = [
  ["instances", "", ".instance-card"], ["templates", "", ".template-card"],
  ["instance-create", "?templateId=registered", ".template-form"],
  ["instance-detail", "?instanceId=target", ".detail-connection"],
  ["instance-clone", "?instanceId=target", ".clone-diff"],
  ["instance-edit", "?instanceId=target", "#edit-name"],
  ["operation", "?operationId=op&instanceId=target", ".operation-timeline"],
  ["diagnostics", "", ".diagnostic-row"], ["retained", "", ".retained-config"],
  ["settings", "", ".settings-path"],
];
// Serve only checked-in mock resources, without executing their actions in the app.
const mocks = createServer(async (request, response) => {
  const path = new URL(request.url, "http://localhost").pathname;
  if (!/^\/(?:[a-z-]+\.html|assets\/[a-z.-]+\.(?:css|js))$/.test(path)) { response.writeHead(404).end(); return; }
  try {
    const content = await readFile(new URL(`../../../docs/ui-mockups${path}`, import.meta.url));
    response.setHeader("Content-Type", path.endsWith(".css") ? "text/css" : path.endsWith(".js") ? "text/javascript" : "text/html");
    response.end(content);
  } catch { response.writeHead(404).end(); }
});
await new Promise((resolve) => mocks.listen(0, "127.0.0.1", resolve));
const mockOrigin = `http://127.0.0.1:${mocks.address().port}`;
const server = spawn(process.execPath, ["node_modules/vite/bin/vite.js", "--host", "127.0.0.1", "--port", "4186", "--strictPort"], { stdio: "pipe" });
const records = [];
const images = [];
let browser;

async function metrics(page) {
  return page.evaluate(() => {
    const measure = (selector) => {
      const element = [...document.querySelectorAll(selector)].find((item) => item.getClientRects().length);
      const style = getComputedStyle(element), box = element.getBoundingClientRect();
      return { x: box.x, y: box.y, width: box.width, padding: style.padding, color: style.color,
        background: style.backgroundColor, fontSize: style.fontSize, fontWeight: style.fontWeight, radius: style.borderRadius };
    };
    const surfaces = ".panel, .instance-card, .template-card, .grid-panel";
    return { viewportWidth: innerWidth, scrollWidth: document.documentElement.scrollWidth,
      sidebar: measure(".sidebar"), workspace: measure(".workspace"), heading: measure("h1"), panel: measure(surfaces),
      headings: [...document.querySelectorAll("h2,h3")].filter((element) => element.getClientRects().length).map((element) => element.textContent),
      panels: [...document.querySelectorAll(surfaces)].filter((element) => element.getClientRects().length).map((element) => {
        const box = element.getBoundingClientRect(); return { x: box.x, y: box.y, width: box.width, height: box.height };
      }) };
  });
}

async function capture(page, name) {
  assert.equal(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth), true, `${name}: page overflow`);
  assert.equal(await page.getByText("never-render-secret", { exact: false }).count(), 0, `${name}: secret mask`);
  assert.equal(await page.getByText("private-backend-error", { exact: false }).count(), 0, `${name}: safe failure text`);
  await page.screenshot({ path: `${output}/${name}.png`, fullPage: true });
  images.push(name);
}

async function compare(page, mock, name) {
  const actual = await metrics(page), reference = await metrics(mock);
  await capture(page, `${name}-react`);
  await mock.screenshot({ path: `${output}/${name}-mock.png`, fullPage: true });
  records.push({ name, actual, reference });
  for (const [part, properties] of Object.entries({ sidebar: ["width", "background"], workspace: ["padding"], heading: ["fontSize", "fontWeight", "color"], panel: ["background", "radius"] })) {
    for (const property of properties) assert.equal(actual[part][property], reference[part][property], `${name}: ${part}.${property}`);
  }
  assert.ok(actual.workspace.x >= actual.sidebar.width, `${name}: content stays beside navigation`);
  assert.ok(parseFloat(actual.heading.fontSize) > 17, `${name}: heading hierarchy`);
  await writeFile(`${output}/${name}-aria.yml`, await page.locator(".main").ariaSnapshot());
}

async function dialog(page, mock, trigger, name, mockAction) {
  await trigger.focus(); await page.keyboard.press("Enter");
  const modal = page.getByRole("dialog"); await modal.waitFor();
  const cancel = modal.getByRole("button", { name: "キャンセル", exact: true });
  assert.equal(await cancel.evaluate((element) => document.activeElement === element), true, `${name}: safe initial focus`);
  assert.notEqual(await cancel.evaluate((element) => getComputedStyle(element).outlineStyle), "none", `${name}: visible keyboard focus`);
  await page.keyboard.press("Shift+Tab");
  assert.equal(await modal.evaluate((element) => element.contains(document.activeElement)), true);
  for (let index = 0; index < 8; index++) {
    await page.keyboard.press("Tab");
    assert.equal(await modal.evaluate((element) => element.contains(document.activeElement)), true, `${name}: focus trap`);
  }
  await mockAction();
  await mock.getByRole("dialog").waitFor();
  const palette = (element) => { const style = getComputedStyle(element); return [style.backgroundColor, style.color, style.borderRadius, style.padding]; };
  assert.deepEqual(await modal.evaluate(palette), await mock.getByRole("dialog").evaluate(palette), `${name}: dialog palette/spacing`);
  await capture(page, `${name}-react`);
  await mock.screenshot({ path: `${output}/${name}-mock.png`, fullPage: true });
  await mock.keyboard.press("Escape");
  await page.keyboard.press("Escape"); await modal.waitFor({ state: "detached" });
  assert.equal(await trigger.evaluate((element) => document.activeElement === element), true, `${name}: restored focus`);
  assert.equal(await page.evaluate(() => window.uiCalls.some((command) => /^(confirm_|change_instance|rename_instance|edit_instance_ports)/.test(command))), false, `${name}: cancellation sends no mutations`);
}

try {
  await mkdir(output, { recursive: true });
  const deadline = Date.now() + 20000;
  while (true) {
    if (server.exitCode !== null) throw new Error("Vite exited");
    try { if ((await fetch(origin, { signal: AbortSignal.timeout(1000) })).ok) break; } catch { /* Local startup. */ }
    if (Date.now() > deadline) throw new Error("Vite startup timed out");
    await new Promise((resolve) => setTimeout(resolve, 100));
  }
  browser = await chromium.launch({ headless: true, ...(process.env.COMPOSENEST_TEST_BROWSER ? { executablePath: process.env.COMPOSENEST_TEST_BROWSER } : {}) });
  const errors = [];
  for (const theme of ["light", "dark"]) for (const width of [1280, 390]) {
    const context = await browser.newContext({ viewport: { width, height: 900 } });
    const page = await context.newPage(), mock = await context.newPage();
    page.on("pageerror", (error) => errors.push(error.message));
    mock.on("pageerror", (error) => errors.push(error.message));
    await installValidationFixture(page, plans);
    await context.addInitScript(({ theme }) => {
      localStorage.setItem("composenest-theme", theme);
      localStorage.setItem("composenest-mock-theme", theme);
    }, { theme });
    for (const [screen, query, ready] of screens) {
      const name = `${screen}-${theme}-${width}`;
      await page.goto(`${origin}/#/${screen}${query}`); await page.locator(ready).first().waitFor();
      if (screen === "settings") await page.getByText("/managed", { exact: true }).waitFor();
      await mock.goto(`${mockOrigin}/${screen}.html`);
      await compare(page, mock, name);
      assert.equal(await page.getByRole("heading", { level: 1 }).count(), 1, `${name}: unique page heading`);
      const unnamed = await page.evaluate(() => [...document.querySelectorAll("button,input,select,textarea")].filter((element) =>
        element.getClientRects().length && !element.disabled && !element.getAttribute("aria-label") && !element.getAttribute("aria-labelledby")
        && !(element.labels?.length) && !(element.tagName === "BUTTON" && element.textContent.trim())).map((element) => element.outerHTML));
      assert.deepEqual(unnamed, [], `${name}: accessible control names`);
      if (width === 390 && (screen === "instance-clone" || screen === "retained")) {
        const selector = screen === "instance-clone" ? ".clone-diff" : ".retained-storage .table-wrap";
        const region = page.locator(selector);
        assert.equal(await region.getAttribute("role"), "region");
        await region.focus(); await page.keyboard.press("ArrowRight");
        await page.waitForFunction((selector) => document.querySelector(selector).scrollLeft > 0, selector);
        assert.notEqual(await region.evaluate((element) => getComputedStyle(element).outlineStyle), "none");
        await capture(page, `${name}-keyboard-scroll`);
      }
      if (screen === "instances") {
        await page.getByText("未確認の環境", { exact: true }).waitFor();
        await page.getByRole("button", { name: "DataGrid", exact: true }).click();
        const sort = page.getByRole("button", { name: "環境名", exact: true });
        await sort.focus(); await page.keyboard.press("Enter");
        assert.equal(await sort.locator("..").getAttribute("aria-sort"), "ascending");
        await page.keyboard.press("Space"); assert.equal(await sort.locator("..").getAttribute("aria-sort"), "descending");
        await mock.locator('button[data-view="grid"]').click();
        await compare(page, mock, `${name}-grid`);
      }
      if (screen === "instance-detail") {
        await page.getByText("••••••••（非表示）", { exact: true }).waitFor();
        const tabs = page.getByRole("tab"); await tabs.first().focus();
        for (const [key, index] of [["ArrowRight", 1], ["End", 2], ["Home", 0], ["ArrowLeft", 2]]) {
          await page.keyboard.press(key);
          assert.equal(await tabs.nth(index).getAttribute("aria-selected"), "true");
          assert.equal(await tabs.nth(index).evaluate((element) => document.activeElement === element), true);
          assert.equal(await page.getByRole("tabpanel").count(), 1);
          if (index !== 0) assert.match(await page.getByRole("tabpanel").textContent(), /未接続/);
          await capture(page, `${name}-tab-${index}`);
          await mock.getByRole("tab").nth(index).click();
          await mock.screenshot({ path: `${output}/${name}-tab-${index}-mock.png`, fullPage: true });
        }
        await tabs.first().click();
        await mock.getByRole("tab").first().click();
        await dialog(page, mock, page.getByRole("button", { name: "環境を削除", exact: true }), `${name}-delete-dialog`, () => mock.locator('[data-action="delete"]').click());
      }
      if (screen === "instance-create" || screen === "instance-clone")
        await dialog(page, mock, page.getByRole("button", { name: screen === "instance-create" ? "作成内容を確認" : "複製内容を確認", exact: true }), `${name}-dialog`, async () => {
          await mock.locator(screen === "instance-create" ? "#environment-name" : "#clone-name").fill("検証先の環境");
          await mock.locator(screen === "instance-create" ? "#create-form" : "#clone-form").evaluate((form) => form.dispatchEvent(new Event("submit", { bubbles: true, cancelable: true })));
        });
      if (screen === "instance-edit") {
        await page.locator("#edit-name").fill("別の環境名");
        const mockEdit = () => mock.locator("#edit-form").evaluate((form) => form.dispatchEvent(new Event("submit", { bubbles: true, cancelable: true })));
        await dialog(page, mock, page.getByRole("button", { name: "変更内容を確認", exact: true }), `${name}-rename-dialog`, mockEdit);
        await page.locator("#edit-port-db").fill("25432");
        await dialog(page, mock, page.getByRole("button", { name: "ポート変更内容を確認", exact: true }), `${name}-ports-dialog`, mockEdit);
      }
    }
    // Include exceptional states in every theme/viewport combination.
    for (const mode of ["empty", "failure", "loading"]) {
      await page.evaluate(() => { window.uiMode = "normal"; });
      await page.goto(`${origin}/#/settings`); await page.getByText("/managed", { exact: true }).waitFor();
      await page.evaluate((mode) => { window.uiMode = mode; }, mode);
      await page.locator(".sidebar").getByRole("button", { name: "環境一覧", exact: true }).click();
      if (mode === "empty") await page.getByText("環境はまだありません。", { exact: false }).waitFor();
      else if (mode === "failure") await page.getByText("環境一覧の取得に失敗しました。更新して再確認してください。", { exact: true }).waitFor();
      else await page.getByText("環境を読み込み中…", { exact: true }).waitFor();
      await capture(page, `instances-${theme}-${width}-${mode}`);
      if (mode === "loading") await page.evaluate(() => { window.uiMode = "normal"; window.releaseUi(); });
    }
    assert.equal(await page.getByText("private-backend-error", { exact: false }).count(), 0);
    assert.equal(await page.evaluate(() => window.uiCalls.some((command) => /^(confirm_|change_instance|rename_instance|edit_instance_ports)/.test(command))), false, "cancelled dialogs do not send mutations");
    await context.close();
  }
  assert.deepEqual(errors, []);
  console.log(`UI validation: ${records.length} mock/React comparisons; themes, responsive layout, keyboard/dialogs, masks and exceptional states passed.`);
} finally {
  // Partial evidence is retained when a comparison fails.
  await writeFile(`${output}/metrics.json`, JSON.stringify(records, null, 2));
  const rows = records.map(({ name }) => `<section><h2>${name}</h2><div><figure><figcaption>HTML mock</figcaption><img src="${name}-mock.png"></figure><figure><figcaption>React</figcaption><img src="${name}-react.png"></figure></div><a href="${name}-aria.yml">ARIA snapshot</a></section>`);
  const extra = images.filter((name) => !records.some((record) => name === `${record.name}-react`));
  await writeFile(`${output}/metadata.json`, JSON.stringify({ generatedAt: new Date().toISOString(), browser: browser?.version(), comparisons: records.length }, null, 2));
  await writeFile(`${output}/index.html`, `<!doctype html><html lang="ja"><meta charset="utf-8"><title>ComposeNest UI comparison</title><style>body{font:14px sans-serif;margin:24px}section>div{display:flex;align-items:start;gap:16px}figure{margin:0;width:50%}img{width:100%;border:1px solid #aaa}</style><h1>ComposeNest UI comparison</h1><p>サンプル値の一致・ピクセル差分だけを合否条件にしません。画面別の差異は docs/ui-validation.md を参照してください。</p>${rows.join("\n")}<h2>ダイアログ・タブ・例外状態</h2>${extra.map((name) => `<p><a href="${name}.png">${name}</a>${name.endsWith("-react") ? ` / <a href="${name.replace(/-react$/, "-mock")}.png">HTML mock</a>` : ""}</p>`).join("\n")}</html>`);
  await browser?.close(); server.kill(); await new Promise((resolve) => mocks.close(resolve));
}
