import type { CloneEdit, ClonePlanView, CreatePlanView, JsonValue, PlanEdit, StorageMethod } from "../generated/template-form";

/** A Core preview with its originating use case retained for typed edits. */
export type FormPlan = { kind: "create"; view: CreatePlanView } | { kind: "clone"; view: ClonePlanView };
/** Only user edits are retained locally; omitted fields stay in the Core plan. */
export interface FormDraft {
  displayName?: string;
  storageMethod?: StorageMethod;
  inputs: Record<string, JsonValue>;
  ports: Record<string, string | null>;
}
/** Explicit actions, separate from candidate edits. */
export interface FormAction { version?: string; generate?: string }
/** Returns an empty, process-local edit buffer. */
export const emptyDraft = (): FormDraft => ({ inputs: {}, ports: {} });
/** Preserves invalid integer text so Core can report its field error. */
export function integerCandidate(text: string): JsonValue {
  if (text === "") return null;
  const number = Number(text);
  return /^-?\d+$/.test(text) && Number.isSafeInteger(number) ? number : text;
}
/** Reads Core candidates without applying defaults or inspecting a Template file. */
export function candidate(plan: FormPlan, key: string) {
  if (plan.kind === "create") {
    const input = plan.view.inputs.find((item) => item.key === key);
    return { value: input?.value ?? null, hasSecret: input?.hasSecret ?? false, needsAnswer: false };
  }
  const input = plan.view.inputs.find((item) => item.key === key && !item.removed);
  return { value: input?.candidate ?? null, hasSecret: input?.hasSecret ?? false, needsAnswer: input?.needsAnswer ?? false };
}
/** Builds the matching Core edit and excludes every removed input and port slot. */
export function planEdit(plan: FormPlan, draft: FormDraft, action: FormAction = {}): PlanEdit | CloneEdit {
  const inputs = Object.fromEntries(Object.entries(draft.inputs).filter(([key]) => key !== action.generate && plan.view.templateForm.inputs.some((input) => input.key === key)));
  const ports = Object.fromEntries(Object.entries(draft.ports).filter(([key]) => plan.view.templateForm.ports.some((slot) => slot.key === key)));
  const shared = { expectedRevision: plan.view.planRevision, displayName: draft.displayName ?? null,
    version: action.version ?? null, storageMethod: draft.storageMethod ?? null, ports };
  if (plan.kind === "create") return { ...shared, inputs, regenerateSecrets: action.generate ? [action.generate] : [] };
  return { ...shared, inputs: { ...Object.fromEntries(Object.entries(inputs).map(([key, value]) =>
    [key, value === null ? { action: "clear" as const } : { action: "input" as const, value }])),
    ...(action.generate ? { [action.generate]: { action: "generate" as const } } : {}) }, confirmSecrets: [] };
}
/** Japanese presentation for safe Core concern codes; unknown codes remain visible. */
export function concernMessage(code: string): string {
  const messages: Record<string, string> = {
    INPUT_REQUIRED_OR_INVALID: "入力内容を確認してください。",
    INPUT_TYPE_CHANGED: "入力型が変更されています。入力し直してください。",
    DISPLAY_NAME_INVALID: "環境名を確認してください。",
    VERSION_NEEDS_ANSWER: "バージョンを選択・確認してください。",
    STORAGE_NEEDS_ANSWER: "保存方式を選択してください。",
    SECRET_REUSE_NEEDS_CONFIRMATION: "秘密の引継ぎ確認が必要です。",
    PORT_NEEDS_INPUT: "接続ポートを入力してください。",
    PORT_CHECK_UNAVAILABLE: "接続ポートを確認できません。",
    PORT_CHECK_INCOMPLETE: "接続ポートの確認が完了していません。",
    PORT_CONFLICT: "接続ポートが競合しています。",
    PORT_INVALID: "接続ポートを確認してください。",
  };
  return messages[code] ?? `確認が必要です（${code}）。`;
}
