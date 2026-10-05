import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { mkdir } from "node:fs/promises";
import { chromium } from "playwright";

const origin = "http://127.0.0.1:4178";
const server = spawn(process.execPath, ["node_modules/vite/bin/vite.js", "--host", "127.0.0.1", "--port", "4178", "--strictPort"], { stdio: "pipe" });
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
    window.mode = "delayed"; window.listCalls = 0;
    const base = { selectedVersion: "18", ports: [{ slot: "database", hostIp: "127.0.0.1", hostPort: 5432, containerPort: 5432 }],
      storageMethod: "bind", specRevision: 1, appliedSpecRevision: 1, needsAttention: false,
      observation: { runtimeState: "running", health: "healthy", observedAt: "2026-10-05 08:00:00", freshness: "fresh" } };
    window.rows = [
      { ...base, id: "ready & one", name: "開発用データベース", templateId: "postgresql", runtimeStatus: "ready" },
      { ...base, id: "stopped", name: "キャッシュ", templateId: "redis", runtimeStatus: "stopped", storageMethod: "volume" },
      { ...base, id: "unknown", name: "未確認の環境", templateId: "custom", runtimeStatus: "unknown", observation: null },
      { ...base, id: "failed", name: "要確認の環境", templateId: "redis", runtimeStatus: "ready", needsAttention: true },
    ];
    window.__TAURI_INTERNALS__ = { invoke: async (command, { request }) => {
      const response = (result) => ({ apiVersion: window.mode === "bad-version" && command === "list_instances" ? 2 : 1,
        requestId: (request.context ?? request).requestId, error: null, result });
      if (command === "get_bootstrap") return response({ applicationTitle: "ComposeNest", startedAtUnixSeconds: 1 });
      if (command === "list_instances") {
        window.listCalls++;
        if (window.mode === "delayed") await new Promise((resolve) => { window.releaseList = resolve; });
        if (window.mode === "failure") throw new Error("Unavailable");
        return response(structuredClone(window.rows));
      }
      if (command === "get_instance_actions") return response({ id: request.instanceId, name: "開発用データベース", revision: 1, runtimeStatus: "ready", actions: [], observedAt: null });
      throw new Error(`Unexpected command ${command}`);
    } };
  });
  await page.goto(origin);
  const refresh = page.getByRole("button", { name: "更新", exact: true });
  await page.getByText("環境を読み込み中…", { exact: true }).waitFor();
  assert.equal(await refresh.isDisabled(), true);
  assert.equal(await page.locator(".instance-card").count(), 0);
  await page.evaluate(() => { window.mode = "normal"; window.releaseList(); });
  await page.getByText("4 / 4 環境", { exact: true }).waitFor();
  assert.deepEqual(await page.locator(".stat-value").allTextContents(), ["4環境", "1環境", "1環境", "2環境"]);
  const tabs = page.getByRole("group", { name: "環境の絞り込み" });
  const search = page.getByRole("searchbox", { name: "環境を検索" });
  await tabs.getByRole("button", { name: "停止中" }).click();
  assert.deepEqual(await page.locator(".instance-card h3").allTextContents(), ["キャッシュ"]);
  await search.fill("POSTGRESQL");
  await page.getByText("0 / 4 環境", { exact: true }).waitFor();
  await page.getByText("条件に一致する環境はありません。", { exact: true }).waitFor();
  await tabs.getByRole("button", { name: "すべて" }).click();
  assert.equal(await page.locator(".instance-card").count(), 1);
  await search.fill("開発用"); assert.equal(await page.locator(".instance-card").count(), 1);
  await search.fill("");
  for (const theme of ["ダーク", "ライト"]) {
    await page.locator(".topbar").getByRole("button", { name: theme, exact: true }).click();
    for (const width of [1280, 800, 390]) {
      await page.setViewportSize({ width, height: 900 });
      assert.equal(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth), true);
      await mkdir("test-results", { recursive: true });
      await page.screenshot({ path: `test-results/instance-list-${theme === "ダーク" ? "dark" : "light"}-${width}.png`, fullPage: true });
    }
  }
  await page.evaluate(() => { window.rows[0].name = "更新後の環境"; window.rows[0].observation.observedAt = "2026-10-05 09:00:00"; window.rows[0].ports[0].hostPort = 15432; });
  await refresh.click(); await page.getByRole("heading", { name: "更新後の環境", exact: true }).waitFor();
  await page.getByText("最終観測 2026-10-05 09:00:00 UTC", { exact: true }).waitFor();
  assert.equal(await page.getByText("127.0.0.1:15432", { exact: true }).count(), 1);
  await page.getByRole("button", { name: "更新後の環境の詳細", exact: true }).click();
  await page.getByRole("heading", { level: 1, name: "環境の詳細", exact: true }).waitFor();
  assert.match(page.url(), /instanceId=ready\+%26\+one/);
  await page.getByRole("button", { name: "戻る", exact: true }).click();
  await page.getByText("4 / 4 環境", { exact: true }).waitFor();
  for (const mode of ["failure", "bad-version"]) {
    await page.evaluate((mode) => { window.mode = mode; }, mode); await refresh.click();
    await page.getByText("環境一覧の取得に失敗しました。更新して再確認してください。", { exact: true }).waitFor();
    assert.equal(await page.locator(".instance-card").count(), 0);
  }
  await page.evaluate(() => { window.mode = "normal"; window.rows = []; }); await refresh.click();
  await page.getByText("0 / 0 環境", { exact: true }).waitFor();
  await page.getByText("環境はまだありません。", { exact: false }).waitFor();
  await page.getByRole("button", { name: "環境を作成", exact: true }).click();
  await page.getByRole("heading", { level: 1, name: "テンプレート", exact: true }).waitFor();
  assert.deepEqual(errors, []);
  console.log("Instance list: loading, counts, search/filter, refresh, timestamps, detail identity, errors/retry, empty state and responsive themes passed.");
} finally { await browser?.close(); server.kill(); }
