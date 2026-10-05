import assert from "node:assert/strict";
import { test } from "node:test";
import { listItem, selectInstances } from "../src/instances/listModel.ts";

const fixture = (overrides = {}) => ({ id: "one", name: "開発用データベース", templateId: "postgresql", selectedVersion: "18",
  ports: [{ hostIp: "127.0.0.1", hostPort: 5432 }], storageMethod: "bind", specRevision: 1, appliedSpecRevision: 1,
  runtimeStatus: "ready", needsAttention: false, observation: { observedAt: "2026-10-05 08:00:00", freshness: "fresh" }, ...overrides });

test("uncertain, unhealthy, absent, unapplied and flagged instances never enter the stopped bucket", () => {
  for (const override of [{ observation: null }, { observation: { freshness: "stale", observedAt: "old" } },
    ...["unknown", "unhealthy", "absent", "preparing"].map((runtimeStatus) => ({ runtimeStatus })),
    { needsAttention: true }, { appliedSpecRevision: null }, { appliedSpecRevision: 2 }]) {
    const item = listItem(fixture(override));
    assert.equal(item.state, "attention");
    assert.equal(selectInstances([item], "", "stopped").length, 0);
  }
  assert.equal(listItem(fixture()).state, "ready");
  assert.equal(listItem(fixture({ runtimeStatus: "stopped" })).state, "stopped");
});

test("name and service searches compose with mutually exclusive status filters", () => {
  const rows = [listItem(fixture()), listItem(fixture({ id: "two", name: "キャッシュ", templateId: "redis", runtimeStatus: "stopped" }))];
  assert.equal(selectInstances(rows, "開発", "all").length, 1);
  assert.equal(selectInstances(rows, " ＰＯＳＴＧＲＥＳＱＬ ", "ready").length, 1);
  assert.equal(selectInstances(rows, "rEdIs", "stopped").length, 1);
  assert.equal(selectInstances(rows, "Redis", "ready").length, 0);
  assert.equal(selectInstances(rows, "  ", "all").length, 2);
});

test("projection uses committed endpoints and saved observation timestamps", () => {
  const item = listItem(fixture({ ports: [{ hostIp: "::1", hostPort: 15432 }], observation: { observedAt: "2026-10-01 01:02:03", freshness: "stale" } }));
  assert.equal(item.endpoints, "[::1]:15432");
  assert.equal(item.observed, "2026-10-01 01:02:03 UTC（過去の観測）");
  assert.equal(item.configuration, "保存構成は適用済み");
  assert.equal(listItem(fixture({ templateId: "custom", storageMethod: "volume", ports: [] })).storage, "named volume");
});
