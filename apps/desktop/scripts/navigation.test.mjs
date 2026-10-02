import assert from "node:assert/strict";
import test from "node:test";
import { mainPages, parentRoute, parseRoute, routeHash, selectedNavigation } from "../src/navigation.ts";

test("all ten screens preserve identities and select their own section", () => {
  const routes = [
    ...mainPages.map((page) => ({ page })),
    { page: "instance-create", templateId: "postgres/日本語 ?&" },
    ...["instance-detail", "instance-clone", "instance-edit"].map((page) => ({ page, instanceId: "instance/日本語 ?&" })),
    { page: "operation", operationId: "operation/日本語 ?&", instanceId: "instance" },
  ];
  for (const route of routes) {
    assert.deepEqual(parseRoute(routeHash(route)), route);
    assert.equal(selectedNavigation(route), route.page === "instance-create" ? "templates" : mainPages.includes(route.page) ? route.page : "instances");
  }
});

test("deep-linked nested screens return to the same target without prior history", () => {
  for (const page of ["instance-clone", "instance-edit", "operation"]) {
    const parent = parentRoute({ page, instanceId: "target", operationId: "operation" });
    assert.deepEqual(parent, { page: "instance-detail", instanceId: "target" });
    assert.deepEqual(parentRoute(parent), { page: "instances" });
  }
  assert.deepEqual(parentRoute({ page: "instance-create", templateId: "template" }), { page: "templates" });
  assert.deepEqual(parentRoute({ page: "operation", operationId: "operation" }), { page: "instances" });
  assert.equal(parentRoute({ page: "settings" }), null);
});

test("invalid links do not select a target or interpret extra business parameters", () => {
  for (const hash of ["", "#/missing", "#/instance-detail", "#/instance-edit?instanceId=", "#/operation?operationId=%00", "#/instance-create?templateId=" + "x".repeat(201)])
    assert.deepEqual(parseRoute(hash), { page: "instances" });
  assert.deepEqual(parseRoute("#/settings?password=secret&execute=delete"), { page: "settings" });
});
