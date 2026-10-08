import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { mkdir, writeFile } from "node:fs/promises";
import { arch, cpus, platform, release } from "node:os";
import { chromium } from "playwright";
import { installValidationFixture } from "./ui-validation-fixture.mjs";

const origin = "http://127.0.0.1:4187";
const output = "test-results/acceptance-performance.json";
const server = spawn(process.execPath, ["node_modules/vite/bin/vite.js", "preview", "--host", "127.0.0.1", "--port", "4187", "--strictPort"], { stdio: "ignore" });
const samples = [];
let browser;
let status = "failed";

// Timings include automation coordination and two paint opportunities; no fixed-host guarantee.
async function measure(page, label, action, ready) {
  await page.evaluate(() => { window.measurementStarted = performance.now(); });
  await action();
  await ready();
  const ms = await page.evaluate(async () => {
    await new Promise((resolve) => requestAnimationFrame(() => requestAnimationFrame(resolve)));
    return performance.now() - window.measurementStarted;
  });
  samples.push({ label, ms });
}

try {
  const deadline = Date.now() + 20000;
  while (true) {
    if (server.exitCode !== null) throw new Error("Vite preview exited");
    try { if ((await fetch(origin, { signal: AbortSignal.timeout(1000) })).ok) break; } catch { /* Local startup. */ }
    if (Date.now() > deadline) throw new Error("Vite preview startup timed out");
    await new Promise((resolve) => setTimeout(resolve, 100));
  }
  browser = await chromium.launch({ headless: true, ...(process.env.COMPOSENEST_TEST_BROWSER ? { executablePath: process.env.COMPOSENEST_TEST_BROWSER } : {}) });
  for (const theme of ["light", "dark"]) for (const view of ["cards", "grid"]) {
    const page = await browser.newPage({ viewport: { width: 1280, height: 900 } });
    const errors = [];
    page.on("pageerror", (error) => errors.push(error.message));
    await installValidationFixture(page, { create: {}, clone: {} });
    await page.addInitScript(({ theme }) => { localStorage.setItem("composenest-theme", theme); }, { theme });
    await page.goto(origin);
    await page.getByText("2 / 2 環境", { exact: true }).waitFor();
    await page.evaluate(async () => {
      const response = await window.__TAURI_INTERNALS__.invoke("list_instances", { request: { apiVersion: 1, requestId: "fixture" } });
      window.uiRows = Array.from({ length: 30 }, (_, index) => ({ ...response.result[0],
        id: `instance-${index}`, name: `受入環境 ${String(index).padStart(2, "0")}`,
        templateId: index % 2 ? "redis" : "postgresql", storageMethod: index % 2 ? "volume" : "bind",
        runtimeStatus: index % 2 ? "stopped" : "ready",
      }));
    });
    await page.getByRole("button", { name: "更新", exact: true }).click();
    // Select explicitly; preference storage is not the behavior under measurement.
    await page.getByText("30 / 30 環境", { exact: true }).waitFor();
    await page.getByRole("button", { name: view === "cards" ? "カード" : "DataGrid", exact: true }).click();
    const rows = page.locator(view === "cards" ? ".instance-card" : ".grid-panel tbody tr");
    const search = page.getByRole("searchbox", { name: "環境を検索" });
    const refresh = page.getByRole("button", { name: "更新", exact: true });
    const filters = page.getByRole("group", { name: "環境の絞り込み" });
    const count = (expected) => async () => assert.equal(await rows.count(), expected);
    for (let run = 0; run < 10; run++) {
      await measure(page, `${theme}/${view}/refresh`, () => refresh.click(), async () => {
        await page.waitForFunction(() => document.querySelector(".instance-list").getAttribute("aria-busy") === "false");
        await count(30)();
      });
      await measure(page, `${theme}/${view}/search`, () => search.fill("受入環境 29"), async () => {
        await page.getByText("1 / 30 環境", { exact: true }).waitFor(); await count(1)();
      });
      await search.fill("");
      await measure(page, `${theme}/${view}/filter`, () => filters.getByRole("button", { name: "停止中", exact: true }).click(), async () => {
        await page.getByText("15 / 30 環境", { exact: true }).waitFor(); await count(15)();
      });
      await filters.getByRole("button", { name: "すべて", exact: true }).click();
      if (view === "grid") await measure(page, `${theme}/${view}/sort`, () => page.getByRole("button", { name: "環境名", exact: true }).click(), async () => {
        assert.equal(await rows.first().locator(".environment-name").textContent(), run % 2 ? "受入環境 29" : "受入環境 00");
      });
    }
    await page.evaluate(() => { window.uiMode = "loading"; });
    await measure(page, `${theme}/${view}/pending-feedback`, () => refresh.click(), () => page.getByText("環境を読み込み中…", { exact: true }).waitFor());
    await search.fill("受入環境 29");
    assert.equal(await search.inputValue(), "受入環境 29", "input stays usable while IPC is pending");
    await page.evaluate(() => { window.uiMode = "normal"; window.releaseUi(); });
    await page.getByText("1 / 30 環境", { exact: true }).waitFor();
    await count(1)();
    assert.deepEqual(errors, []);
    await page.close();
  }
  status = "passed";
} finally {
  const labels = [...new Set(samples.map((sample) => sample.label))];
  const summary = labels.map((label) => {
    const values = samples.filter((sample) => sample.label === label).map((sample) => sample.ms).sort((a, b) => a - b);
    return { label, count: values.length, medianMs: values[Math.floor(values.length / 2)], p95Ms: values[Math.ceil(values.length * 0.95) - 1], maxMs: values.at(-1) };
  });
  await mkdir("test-results", { recursive: true });
  await writeFile(output, JSON.stringify({ status, timestamp: new Date().toISOString(), commit: process.env.GITHUB_SHA ?? null,
    environment: { os: platform(), release: release(), arch: arch(), cpu: cpus()[0]?.model, node: process.version, browser: browser?.version() },
    scope: "production React assets, headless Chromium, fake list IPC, 30 instances; excludes SQLite, native WebView, Docker and image acquisition",
    provisionalFeedbackTargetMs: 1000, summary, samples }, null, 2));
  await browser?.close();
  server.kill();
}
console.log(`30-instance acceptance responsiveness passed; timings: ${output}`);
