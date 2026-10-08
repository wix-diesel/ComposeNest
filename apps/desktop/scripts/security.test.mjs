import assert from "node:assert/strict";
import { readFile, readdir } from "node:fs/promises";
import { test } from "node:test";

const read = (path) => readFile(new URL(path, import.meta.url), "utf8");

test("only local main-window application commands and explicit clipboard writes are permitted", async () => {
  const capability = JSON.parse(await read("../src-tauri/capabilities/default.json"));
  assert.deepEqual(capability.windows, ["main"]);
  assert.equal(capability.remote, undefined);
  assert.equal(capability.webviews, undefined);
  const commands = (await read("../src-tauri/build.rs")).match(/"[a-z_]+"/g).map((value) => value.slice(1, -1));
  const allowed = new Set(commands.map((command) => `allow-${command.replaceAll("_", "-")}`));
  for (const permission of capability.permissions) {
    assert.equal(typeof permission, "string");
    assert.ok(allowed.has(permission) || ["core:event:allow-listen", "core:event:allow-unlisten", "clipboard-manager:allow-write-text"].includes(permission), permission);
  }
  const config = JSON.parse(await read("../src-tauri/tauri.conf.json"));
  assert.ok(config.app.security.csp.includes("default-src 'self'"));
  assert.ok(!config.app.security.csp.includes("unsafe-eval"));
});

test("production code treats external text as text and persists only display preferences", async () => {
  const files = await readdir(new URL("../src/", import.meta.url), { recursive: true });
  for (const file of files.filter((path) => /\.[jt]sx?$/.test(path))) {
    const source = await read(`../src/${file}`);
    assert.doesNotMatch(source, /dangerouslySetInnerHTML|\.innerHTML\s*=|mock\.js|ui-mockups/);
    if (file !== "appearance.ts") assert.doesNotMatch(source, /localStorage|sessionStorage|indexedDB/);
  }
  assert.match(await read("../src/appearance.ts"), /const keys = \{ theme: "composenest-theme", listView: "composenest-list-view" \} as const/);
});
