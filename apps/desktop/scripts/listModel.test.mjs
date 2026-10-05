import assert from "node:assert/strict";
import { test } from "node:test";
import { instanceStateLabels, listItem, selectInstances, sortInstances } from "../src/instances/listModel.ts";

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

test("grid name/state sorting follows Japanese display labels in both directions without mutating rows", () => {
  const rows = [listItem(fixture({ name: "要確認", needsAttention: true })), listItem(fixture({ name: "利用可能" })),
    listItem(fixture({ name: "停止中", runtimeStatus: "stopped" }))];
  const original = [...rows];
  for (const key of ["name", "state"]) {
    const value = (item) => key === "name" ? item.source.name : instanceStateLabels[item.state];
    const expected = rows.map(value).sort((a, b) => a.localeCompare(b, "ja"));
    assert.deepEqual(sortInstances(rows, { key, direction: "ascending" }).map(value), expected);
    assert.deepEqual(sortInstances(rows, { key, direction: "descending" }).map(value), [...expected].reverse());
  }
  assert.deepEqual(rows, original);
  assert.deepEqual(sortInstances(rows, null), rows);
  assert.notEqual(sortInstances(rows, null), rows);
});

test("ports compare numerically across IPv4/IPv6 and multiple bindings; unassigned rows stay last", () => {
  const rows = [
    ["none", []], ["large", [10000]], ["multi", [10000, 80]], ["small", [9]], ["http", [80]], ["middle", [443]],
  ].map(([id, ports]) => listItem(fixture({ id, ports: ports.map((hostPort) => ({ hostIp: id === "small" ? "::1" : "127.0.0.1", hostPort })) })));
  assert.deepEqual(sortInstances(rows, { key: "port", direction: "ascending" }).map((item) => item.source.id), ["small", "http", "multi", "middle", "large", "none"]);
  assert.deepEqual(sortInstances(rows, { key: "port", direction: "descending" }).map((item) => item.source.id), ["large", "middle", "multi", "http", "small", "none"]);
  assert.deepEqual(rows[2].source.ports.map((port) => port.hostPort), [10000, 80]);
  const samePorts = [listItem(fixture({ id: "first" })), listItem(fixture({ id: "second" }))];
  assert.deepEqual(sortInstances(samePorts, { key: "port", direction: "descending" }).map((item) => item.source.id), ["first", "second"]);
  assert.deepEqual(sortInstances([], { key: "port", direction: "ascending" }), []);
});
