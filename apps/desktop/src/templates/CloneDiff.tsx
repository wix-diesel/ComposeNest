import type { ReactNode } from "react";
import type { ClonePlanView, JsonValue } from "../generated/template-form";
import { concernMessage, type FormDraft } from "./formModel";

const methodLabel = (method: string) => method === "bind" ? "ホストフォルダー" : "Docker管理";
const show = (value: JsonValue | undefined): string => value == null ? "未設定" : value === "" ? "空文字" : String(value);
const origins = { inherited: "元の値を引継ぎ", generated: "自動生成", user_input: "ユーザー入力", unset: "未設定" };

/** Secret-safe source/candidate differences, optionally with controls in active rows. */
export function CloneDiff({ plan, draft, renderCandidate }: {
  plan: ClonePlanView; draft?: FormDraft; renderCandidate?: (path: string) => ReactNode;
}) {
  const status = (path: string, labels: string[]) => <>
    {labels.map((label) => <span className="badge" key={label}>{label}</span>)}
    {plan.concerns.filter((concern) => concern.fieldPath === path || (path.startsWith("ports.") && concern.fieldPath === "ports"))
      .map((concern) => <p className="error-text" key={concern.code}>{concernMessage(concern.code)}</p>)}
    {labels.includes("削除") ? <small>複製先では対象外</small> : labels.some((label) => label.includes("未反映")) ? <small>未反映・未検証</small> : !plan.concerns.some((concern) => concern.fieldPath === path || (path.startsWith("ports.") && concern.fieldPath === "ports")) && <small>検証済みの候補 · 確定時に再検証</small>}
  </>;
  const row = (path: string, label: string, source: ReactNode, candidate: ReactNode, labels: string[], editable = true) => <tr key={path} data-diff-path={path}>
    <th scope="row">{label}</th><td>{source}</td><td>{editable ? renderCandidate?.(path) ?? candidate : candidate}</td><td>{status(path, labels)}</td>
  </tr>;
  return <div className="table-wrap clone-diff" role="region" aria-label="複製元と複製先の設定差分（横スクロール可能）" tabIndex={0}><table><caption className="sr-only">複製元と複製先の設定差分</caption>
    <thead><tr><th>設定項目</th><th>複製元</th><th>複製先</th><th>変更内容</th></tr></thead><tbody>
      {row("displayName", "環境名・用途名", plan.sourceName, draft?.displayName ?? plan.displayName, [draft?.displayName !== undefined ? "ユーザー入力・未反映" : "用途名を指定"])}
      {row("version", "バージョン", plan.sourceVersion, plan.version, [plan.version === plan.sourceVersion ? "引継ぎ" : "変更", "保存済みSnapshot内のVersion"])}
      {plan.inputs.map((input) => {
        const form = plan.templateForm.inputs.find((item) => item.key === input.key);
        const secret = form?.inputType === "secret";
        const edited = draft !== undefined && Object.hasOwn(draft.inputs, input.key);
        const value = edited ? draft.inputs[input.key] : input.candidate;
        const hasSecret = edited ? value !== null : input.hasSecret;
        return row(`inputs.${input.key}`, form?.label ?? input.sourceLabel ?? input.key,
          input.added ? "—" : input.sourceHasSecret ? "••••••••（非表示）" : show(input.source),
          input.removed ? "—" : secret ? hasSecret ? "••••••••（設定済み）" : "未設定" : show(value),
          [input.removed ? "削除" : input.added ? "追加" : input.changed ? "変更" : "同じ値",
            edited ? "ユーザー入力・未反映" : origins[input.origin], ...(input.needsAnswer ? ["入力待ち"] : []),
            ...(input.needsSecretConfirmation ? ["秘密の引継ぎ確認待ち"] : [])], !input.removed);
      })}
      {[...plan.templateForm.ports.map((slot) => slot.key), ...plan.removedPorts].map((key) => row(`ports.${key}`,
        plan.templateForm.ports.find((slot) => slot.key === key)?.label ?? key, show(plan.sourcePorts[key]),
        plan.removedPorts.includes(key) ? "—" : draft?.ports[key] !== undefined ? show(draft.ports[key]) : show(plan.ports[key]),
        [plan.removedPorts.includes(key) ? "削除" : plan.explicitPorts.includes(key) ? "ユーザー入力" : plan.addedPorts.includes(key) ? "追加・自動候補" : "自動候補", ...(draft && Object.hasOwn(draft.ports, key) ? ["ユーザー入力・未反映"] : [])], !plan.removedPorts.includes(key)))}
      {row("storageMethod", "保存方式", methodLabel(plan.sourceStorageMethod), methodLabel(draft?.storageMethod ?? plan.storageMethod),
        [draft?.storageMethod ? "ユーザー入力・未反映" : plan.storageMethod === plan.sourceStorageMethod ? "方式を引継ぎ" : "方式を変更"], false)}
      {[...plan.storageSlots, ...plan.removedStorage].map((key) => row(`storage.${key}`,
        plan.templateForm.storage.find((slot) => slot.key === key)?.label ?? key,
        plan.addedStorage.includes(key) ? "—" : "元環境の専用領域",
        plan.removedStorage.includes(key) ? "—" : "新規の専用領域（データなし）",
        [plan.removedStorage.includes(key) ? "削除" : plan.addedStorage.includes(key) ? "追加" : "新規割当て"], false))}
    </tbody></table></div>;
}
