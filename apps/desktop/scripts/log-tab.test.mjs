import assert from "node:assert/strict";
import { mkdir } from "node:fs/promises";

/** Exercises the actual tab/client using the shared instance DTO and Tauri transport fixture. */
export async function testLogs({ page, open, calls }) {
  const tab = (name) => page.getByRole("tab", { name, exact: true });
  const active = (count) => page.waitForFunction((count) => (window.activeLogs?.size ?? 0) === count, count);
  const output = page.locator(".detail-log");
  async function show(mode = "normal", lines = ["actual ******** log", "<script>literal text</script>"]) {
    await open();
    await page.evaluate(({ mode, lines }) => { window.mode = mode; window.logLines = lines; }, { mode, lines });
    await tab("ログ").click();
  }
  await show();
  await output.filter({ hasText: "actual ******** log" }).waitFor();
  assert.equal(await output.locator("script").count(), 0, "logs are text, never HTML");
  assert.equal(await output.textContent(), "actual ******** log\n<script>literal text</script>");
  const first = (await calls("subscribe_logs"))[0].request;
  assert.equal(first.instanceId, "target"); assert.equal(first.expectedSpecRevision, 1);
  await page.getByRole("button", { name: "追従を停止", exact: true }).click(); await active(0);
  const count = (await calls("get_logs")).length;
  await page.waitForTimeout(650);
  assert.equal((await calls("get_logs")).length, count);
  assert.ok((await output.textContent()).includes("actual ******** log"), "paused snapshot is preserved");
  await page.getByRole("button", { name: "追従を再開", exact: true }).click(); await active(1);
  const resumed = (await calls("subscribe_logs")).at(-1).request.context.requestId;
  assert.notEqual(resumed, first.context.requestId);
  await page.getByRole("button", { name: "再読込み", exact: true }).click();
  await page.waitForFunction((id) => window.calls.filter((c) => c.command === "subscribe_logs").at(-1)?.request.context.requestId !== id, resumed);
  await active(1);
  assert.ok((await calls("unsubscribe_logs")).some((c) => c.request.subscriptionId === resumed));
  await tab("概要・接続情報").click(); await active(0);
  await tab("ログ").click(); await active(1);
  await page.evaluate(() => { location.hash = "#/instances"; }); await active(0);

  await show(); await active(1);
  const unconfirmed = (await calls("subscribe_logs"))[0].request.context.requestId;
  await page.evaluate(() => { window.mode = "logs_unsubscribe_failure"; });
  await page.getByRole("button", { name: "追従を停止", exact: true }).click();
  await page.getByRole("alert").waitFor();
  const startsBeforeRetry = (await calls("subscribe_logs")).length;
  await page.getByRole("button", { name: "再読込み", exact: true }).click();
  await page.waitForFunction((id) => window.calls.filter((c) => c.command === "unsubscribe_logs" && c.request.subscriptionId === id).length >= 2, unconfirmed);
  await page.getByRole("alert").waitFor();
  assert.equal((await calls("subscribe_logs")).length, startsBeforeRetry, "continued cleanup failure blocks a new subscription");
  assert.equal(await page.evaluate((id) => window.activeLogs.has(id), unconfirmed), true);
  await page.evaluate(() => { window.mode = "normal"; });
  await page.getByRole("button", { name: "再読込み", exact: true }).click(); await active(1);
  await page.waitForFunction((id) => !window.activeLogs.has(id), unconfirmed);
  assert.ok((await calls("unsubscribe_logs")).filter((c) => c.request.subscriptionId === unconfirmed).length >= 2, "unconfirmed cleanup is retried before starting another session");

  await show("logs_delay");
  await page.waitForFunction(() => typeof window.finishLogs === "function");
  await tab("Compose").click(); await active(0);
  await page.evaluate(() => window.finishLogs());
  await page.waitForFunction(() => window.calls.filter((c) => c.command === "unsubscribe_logs").length >= 2);
  await active(0);
  assert.equal(await page.locator(".detail-log").count(), 0, "a late subscribe reply cannot reopen the tab");
  await show("logs_delay");
  await page.waitForFunction(() => typeof window.finishLogs === "function");
  await page.getByRole("button", { name: "追従を停止", exact: true }).click(); await active(0);
  await page.evaluate(() => window.finishLogs());
  await page.getByText(/追従停止中/).waitFor();
  assert.equal(await page.locator(".instance-logs").getAttribute("aria-busy"), "false");

  await show(); await active(1);
  await page.evaluate(() => {
    Object.defineProperty(document, "hidden", { configurable: true, value: true });
    document.dispatchEvent(new Event("visibilitychange"));
  }); await active(0);
  await page.evaluate(() => {
    Object.defineProperty(document, "hidden", { configurable: true, value: false });
    document.dispatchEvent(new Event("visibilitychange"));
  }); await active(1);
  await page.evaluate(() => { window.edit.specRevision = 2; });
  await page.waitForFunction(() => window.calls.some((c) => c.command === "subscribe_logs" && c.request.expectedSpecRevision === 2));
  await active(1);

  for (const mode of ["logs_lost", "logs_wrong_id", "logs_wrong_target", "logs_wrong_revision", "logs_oversized", "logs_byte_budget", "logs_line_budget", "logs_bad_count", "logs_poll_failure"]) {
    await show(mode);
    await page.getByRole("alert").waitFor(); await active(0);
    assert.equal((await page.locator(".instance-logs").textContent()).includes(await page.evaluate(() => window.secret)), false);
    assert.equal(await output.textContent(), mode === "logs_poll_failure" ? "actual ******** log\n<script>literal text</script>" : "表示できるログはありません。");
    await page.evaluate(() => { window.mode = "normal"; });
    await page.getByRole("button", { name: "再読込み", exact: true }).click(); await active(1);
    await output.filter({ hasText: "actual ******** log" }).waitFor();
  }
  await show("normal", []); await active(1);
  await output.filter({ hasText: "表示できるログはありません。" }).waitFor();
  await page.evaluate(() => { window.logFinished = true; window.logFailed = true; });
  await page.getByRole("alert").waitFor(); await active(0);
  await page.getByText(/購読終了/).waitFor();
  await mkdir("test-results", { recursive: true });
  for (const theme of ["light", "dark"]) for (const width of [1280, 390]) {
    await page.setViewportSize({ width, height: 900 });
    await show("normal", Array.from({ length: 2000 }, (_, n) => `2026-10-07 line ${n} ******** ${"x".repeat(200)}`));
    await page.evaluate((theme) => { document.documentElement.dataset.theme = theme; window.logDropped = 17; window.logTruncated = 3; }, theme);
    await page.getByText(/表示上限による欠落 17行 · 長い行の切詰め 3行/).waitFor();
    assert.equal(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth), true);
    assert.equal(await output.evaluate((el) => el.scrollHeight > el.clientHeight && el.scrollWidth > el.clientWidth), true);
    await page.waitForFunction(() => { const el = document.querySelector(".detail-log"); return el.scrollTop + el.clientHeight >= el.scrollHeight - 2; });
    await page.getByRole("button", { name: "追従を停止", exact: true }).click(); await active(0);
    await output.focus(); await page.keyboard.press("Control+Home");
    await page.waitForFunction(() => document.querySelector(".detail-log").scrollTop === 0);
    const colours = await output.evaluate((el) => ({ text: getComputedStyle(el).color, background: getComputedStyle(el).backgroundColor }));
    assert.notEqual(colours.text, colours.background);
    await page.screenshot({ path: `test-results/log-tab-${theme}-${width}.png`, fullPage: true });
  }
  assert.equal((await calls("change_instance")).length, 0, "no log control changes containers");
  console.log("Logs: lifecycle, late/lost responses, identity/budgets, masking boundary, failures, document visibility and both themes/widths passed.");
}
