import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import { test } from "node:test";
import { candidate, emptyDraft, integerCandidate, planEdit } from "../src/templates/formModel.ts";

const views = JSON.parse(await readFile("test-results/template-form-plans.json", "utf8"));
test("create edits preserve unset, false, zero, empty string and invalid integer text", () => {
  const plan = { kind: "create", view: structuredClone(views.create) };
  for (const key of ["zero", "empty"]) plan.view.templateForm.inputs.push({ key });
  const edit = planEdit(plan, { ...emptyDraft(), displayName: "", inputs: { zero: 0, empty: "", removed: false, retained: null } });
  assert.equal(edit.displayName, "");
  assert.deepEqual(edit.inputs, { zero: 0, empty: "", removed: false, retained: null });
  for (const [text, expected] of [["", null], ["0", 0], ["-10", -10], ["1.5", "1.5"], ["1e2", "1e2"], ["9007199254740992", "9007199254740992"]]) assert.equal(integerCandidate(text), expected);
});
test("Version changes submit only active fields and slots with the Core revision", () => {
  const plan = { kind: "create", view: views.createNext };
  const edit = planEdit(plan, { ...emptyDraft(), inputs: { removed: false, retained: "longer", added: "blue" }, ports: { db: "5432", api: "9001" } });
  assert.equal(edit.expectedRevision, plan.view.planRevision);
  assert.deepEqual(edit.inputs, { retained: "longer", added: "blue" });
  assert.deepEqual(edit.ports, { api: "9001" });
  assert.deepEqual(planEdit(plan, emptyDraft(), { version: "1" }).inputs, {});
});
test("Clone candidates keep Core input-wait instead of using create defaults", () => {
  const create = { kind: "create", view: views.createNext };
  const clone = { kind: "clone", view: views.cloneNext };
  assert.equal(candidate(create, "added").value, "green");
  assert.deepEqual(candidate(clone, "added"), { value: null, hasSecret: false, needsAnswer: true });
  const edit = planEdit(clone, { ...emptyDraft(), inputs: { added: "blue", removed: false, retained: null } });
  assert.deepEqual(edit.inputs, { added: { action: "input", value: "blue" }, retained: { action: "clear" } });
  assert.deepEqual(edit.confirmSecrets, [], "common editing must not silently confirm source secrets");
});
test("explicit generation never conflicts with a pending secret answer", () => {
  for (const kind of ["create", "clone"]) {
    const plan = { kind, view: views[kind] };
    const edit = planEdit(plan, { ...emptyDraft(), inputs: { password: "entered" } }, { generate: "password" });
    if (kind === "create") { assert.deepEqual(edit.regenerateSecrets, ["password"]); assert.deepEqual(edit.inputs, {}); }
    else assert.deepEqual(edit.inputs, { password: { action: "generate" } });
    assert.equal(candidate(plan, "password").value, null);
    assert.equal(candidate(plan, "password").hasSecret, true);
  }
});
