import { createRoot } from "react-dom/client";
import type { CloneEdit, JsonValue, PlanEdit } from "../src/generated/template-form";
import { TemplateForm } from "../src/templates/TemplateForm";
import type { FormPlan } from "../src/templates/formModel";
import "../src/styles.css";

declare global {
  interface Window {
    formFixture: { initial: FormPlan; next: FormPlan };
    formRequests: Array<PlanEdit | CloneEdit>;
    formFailure: boolean;
    formDelay: number;
  }
}
window.formRequests = [];
window.formDelay = 0;
const root = document.getElementById("root");
if (!root) throw new Error("Missing test root");
createRoot(root).render(<div className="workspace"><TemplateForm initialPlan={window.formFixture.initial} onUpdate={async (plan, edit) => {
  window.formRequests.push(edit);
  await new Promise((resolve) => setTimeout(resolve, window.formDelay));
  if (window.formFailure) throw { code: "INPUT_INVALID", reason: "テスト用の入力エラー", fieldPath: "inputs.retained" };
  const next = structuredClone(edit.version && edit.version !== plan.view.version ? window.formFixture.next : plan);
  next.view.planRevision = plan.view.planRevision + 1;
  if (edit.displayName !== null) next.view.displayName = edit.displayName;
  if (edit.storageMethod !== null) next.view.storageMethod = edit.storageMethod;
  next.view.concerns = next.view.concerns.filter((item) => !(edit.version !== null && item.code === "VERSION_NEEDS_ANSWER") && !(edit.storageMethod !== null && item.code === "STORAGE_NEEDS_ANSWER"));
  if (next.kind === "clone" && "confirmSecrets" in edit) for (const key of edit.confirmSecrets) {
    const input = next.view.inputs.find((item) => item.key === key);
    if (input) input.needsSecretConfirmation = false;
    next.view.concerns = next.view.concerns.filter((item) => !(item.fieldPath === `inputs.${key}` && item.code === "SECRET_REUSE_NEEDS_CONFIRMATION"));
  }
  for (const input of next.view.inputs) {
    const answer = edit.inputs[input.key];
    if (answer === undefined) continue;
    const value: JsonValue = "confirmSecrets" in edit
      ? answer !== null && typeof answer === "object" && "action" in answer && answer.action === "input" && "value" in answer ? answer.value : null
      : answer;
    if ("candidate" in input) { input.candidate = input.definition && next.view.templateForm.inputs.find((item) => item.key === input.key)?.inputType === "secret" ? null : value; input.needsAnswer = false; }
    else input.value = next.view.templateForm.inputs.find((item) => item.key === input.key)?.inputType === "secret" ? null : value;
  }
  return next;
}} /></div>);
