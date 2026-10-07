import { useRef, useState } from "react";
import type { CloneEdit, JsonValue, PlanEdit } from "../generated/template-form";
import type { ErrorDto } from "../generated/ipc";
import { candidate, concernMessage, emptyDraft, planEdit, type FormAction, type FormPlan } from "./formModel";
import { CloneDiff } from "./CloneDiff";
import { TemplateInput } from "./TemplateInput";
import "./template-form.css";

const storage = {
  bind: { label: "ホストフォルダー", detail: "bind mount · 管理ルート内に保存", description: "管理ルート配下に新しい専用フォルダーを割り当てます。" },
  volume: { label: "Docker管理", detail: "named volume · 専用領域に保存", description: "Dockerが管理する新しい専用volumeを割り当てます。" },
};

/** Shared create/clone input surface. Remount with a new plan ID when opening another plan. */
export function TemplateForm({ initialPlan, onUpdate, onReview, locked = false }: {
  initialPlan: FormPlan;
  onUpdate: (plan: FormPlan, edit: PlanEdit | CloneEdit) => Promise<FormPlan>;
  onReview?: (plan: FormPlan, trigger: HTMLButtonElement) => Promise<FormPlan>;
  locked?: boolean;
}) {
  const [plan, setPlan] = useState(initialPlan);
  const [draft, setDraft] = useState(emptyDraft);
  const [busy, setBusy] = useState(false);
  const [failure, setFailure] = useState<Pick<ErrorDto, "reason" | "fieldPath"> | null>(null);
  const inFlight = useRef(false);
  const view = plan.view;
  const form = view.templateForm;
  const method = draft.storageMethod ?? view.storageMethod;
  const dirty = draft.displayName !== undefined || draft.storageMethod !== undefined || Object.keys(draft.inputs).length > 0 || Object.keys(draft.ports).length > 0;
  const error = (path: string) => failure?.fieldPath === path ? failure.reason : view.concerns.filter((item) => item.fieldPath === path).map((item) => concernMessage(item.code)).join(" ");
  const value = (key: string): JsonValue => Object.hasOwn(draft.inputs, key) ? draft.inputs[key] : candidate(plan, key).value;
  const secretSet = (key: string) => Object.hasOwn(draft.inputs, key) ? draft.inputs[key] !== null : candidate(plan, key).hasSecret;
  function summaryValue(key: string): string {
    const input = form.inputs.find((item) => item.key === key);
    if (input?.inputType === "secret") {
      if (secretSet(key)) return "••••••••（設定済み）";
      return "未設定";
    }
    const currentValue = value(key);
    if (currentValue === null) return "未設定";
    if (currentValue === "") return "空文字";
    return String(currentValue);
  }
  const fieldLabel = (path: string) => form.inputs.find((input) => path === `inputs.${input.key}`)?.label
    ?? form.ports.find((slot) => path === `ports.${slot.key}`)?.label
    ?? ({ displayName: "環境名", version: "バージョン", storageMethod: "保存方式", ports: "接続ポート" } as Record<string, string>)[path] ?? "設定";

  async function apply(action: FormAction = {}, reviewTrigger?: HTMLButtonElement) {
    if (inFlight.current) return;
    inFlight.current = true; setBusy(true); setFailure(null);
    let next = plan;
    async function update(edit: PlanEdit | CloneEdit) {
      const response = await onUpdate(next, edit);
      if (response.kind !== next.kind || response.view.planId !== next.view.planId || response.view.planRevision <= next.view.planRevision) throw new Error("invalid_plan_response");
      next = response; setPlan(next); setDraft(emptyDraft());
    }
    try {
      // Flush edits against their original Version before requesting a switch.
      if (dirty) await update(planEdit(next, draft));
      if (action.version !== undefined || action.generate !== undefined || action.copy !== undefined || action.confirmSecret !== undefined) await update(planEdit(next, emptyDraft(), action));
      if (reviewTrigger && onReview) {
        const refreshed = await onReview(next, reviewTrigger);
        if (refreshed.kind !== next.kind || refreshed.view.planId !== next.view.planId || refreshed.view.planRevision < next.view.planRevision) throw new Error("invalid_plan_response");
        setPlan(refreshed);
      }
    } catch (caught) {
      const safe = caught !== null && typeof caught === "object" && "code" in caught && "reason" in caught && typeof caught.reason === "string";
      setFailure(safe ? { reason: caught.reason as string, fieldPath: "fieldPath" in caught && typeof caught.fieldPath === "string" ? caught.fieldPath : null }
        : { reason: "設定を反映できませんでした。再確認してください。", fieldPath: null });
    } finally { inFlight.current = false; setBusy(false); }
  }
  const nameField = <div className="field"><label htmlFor="display-name">環境名<span className="required">必須</span></label>
          <input id="display-name" placeholder={plan.kind === "clone" ? "例：機能検証用DB" : undefined} value={draft.displayName ?? view.displayName} aria-required="true" aria-invalid={!!error("displayName")}
            aria-describedby="name-error" onChange={(event) => setDraft({ ...draft, displayName: event.target.value })} />
          <p id="name-error" className="error-text">{error("displayName")}</p></div>;
  const versionField = <div className="field"><label htmlFor="service-version">バージョン</label>
          <select id="service-version" value={view.version} aria-invalid={!!error("version")} aria-describedby="version-error"
            onChange={(event) => { void apply({ version: event.target.value }); }}>{view.versions.map((version) => <option key={version}>{version}</option>)}</select>
          <p id="version-error" className="error-text">{error("version")}</p>
          {view.concerns.some((item) => item.code === "VERSION_NEEDS_ANSWER") && <button type="button" className="btn small" onClick={() => { void apply({ version: view.version }); }}>バージョン選択を確認</button>}</div>;
  const portField = (slot: typeof form.ports[number]) => <div key={slot.key} className="field" data-field-path={`ports.${slot.key}`}>
          <label htmlFor={`port-${slot.key}`}>{slot.label}</label><input id={`port-${slot.key}`} inputMode="numeric"
            value={Object.hasOwn(draft.ports, slot.key) ? draft.ports[slot.key] ?? "" : view.ports[slot.key] ?? ""}
            aria-invalid={!!error(`ports.${slot.key}`)} aria-describedby={`port-${slot.key}-help`}
            onChange={(event) => setDraft({ ...draft, ports: { ...draft.ports, [slot.key]: event.target.value || null } })} />
          <small id={`port-${slot.key}-help`}>127.0.0.1 にのみ公開します。候補は確定時に再確認します。{error(`ports.${slot.key}`)}</small>
        </div>;
  const inputField = (input: typeof form.inputs[number]) => <div key={input.key}><TemplateInput key={`${view.planRevision}-${input.key}`} input={input}
          {...candidate(plan, input.key)} value={value(input.key)} error={error(`inputs.${input.key}`)}
          onChange={(answer) => setDraft({ ...draft, inputs: { ...draft.inputs, [input.key]: answer } })}
          onGenerate={() => { void apply({ generate: input.key }); }} />{plan.kind === "clone" && <div className="actions">
    {plan.view.inputs.some((item) => item.key === input.key && item.canCopy) && <button type="button" className="btn small" onClick={() => { void apply({ copy: input.key }); }}>元の値を使用</button>}
    {plan.view.inputs.find((item) => item.key === input.key)?.needsSecretConfirmation && <button type="button" className="btn small" onClick={() => { void apply({ confirmSecret: input.key }); }}>{input.label}の引継ぎを確認</button>}
  </div>}</div>;
  function cloneControl(path: string) {
    if (path === "displayName") return nameField;
    if (path === "version") return versionField;
    const input = form.inputs.find((item) => path === `inputs.${item.key}`);
    if (input) return inputField(input);
    const port = form.ports.find((item) => path === `ports.${item.key}`);
    return port ? portField(port) : undefined;
  }
  return <form className="template-form" noValidate onSubmit={(event) => { event.preventDefault(); void apply(); }}>
    {plan.kind === "clone" && <div className="notice"><strong>設定だけを複製します。データは複製しません。</strong><p>複製元のデータや稼働状態には影響しません。複製先は新しい専用領域から始まります。</p></div>}
    {failure && <p role="alert" className="notice error-text">{failure.reason}</p>}
    <fieldset disabled={busy || locked} aria-busy={busy}><div className="two-column"><div>
      {plan.kind === "clone" ? <section className="panel"><h2>変更内容の確認</h2>
        <CloneDiff plan={plan.view} draft={draft} renderCandidate={cloneControl} />
      </section> : <><section className="panel"><h2>基本設定</h2>
        {nameField}
        {versionField}
        <div className="two-equal">{form.ports.map(portField)}</div>
      </section>
      {form.inputs.length > 0 && <section className="panel"><h2>接続情報・入力設定</h2><div className="two-equal">
        {form.inputs.map(inputField)}
      </div><p className="notice">認証情報は端末内の設定と生成されるComposeファイルに平文で保存されます。</p></section>}
      </>}
      <section className="panel"><h2>{plan.kind === "clone" ? "複製先の保存方式" : "データの保存先"}</h2><div className="radio-options" role="radiogroup" aria-label="保存方式" aria-describedby="storage-help">
        {(["bind", "volume"] as const).map((choice) => <label key={choice} className="radio-option"><input type="radio" name="storage-method" checked={method === choice}
          onChange={() => setDraft({ ...draft, storageMethod: choice })} /><span>{storage[choice].label}<small>{storage[choice].detail}</small></span></label>)}
      </div><p id="storage-help">{storage[method].description}</p>
        {plan.kind === "clone" && <p>複製元の方式を初期選択しています。方式を変えてもデータの移行は行いません。</p>}
        {view.concerns.some((item) => item.code === "STORAGE_NEEDS_ANSWER") && <button className="btn small" type="button" onClick={() => { setDraft({ ...draft, storageMethod: method }); }}>現在の保存方式を確認</button>}<p className="error-text">{error("storageMethod")}</p>
        {form.storage.map((slot) => <p key={slot.key}>{slot.label}: <code>{String(slot.container)}</code></p>)}
        {form.storage.length === 0 && <p>このバージョンに保存領域はありません。</p>}
      </section>
      <div className="form-footer"><small>{busy ? "設定を確認しています…" : "確定前にポートと設定を再確認します。"}</small><div className="actions">
        <button className="btn" type="submit" disabled={!dirty}>入力を反映</button>
        {onReview && <button className="btn primary" type="button" onClick={(event) => { void apply({}, event.currentTarget); }}>{plan.kind === "clone" ? "複製内容を確認" : "作成内容を確認"}</button>}
      </div></div>
    </div><aside><section className="panel"><h2>{plan.kind === "clone" ? "複製元の環境" : "設定する環境"}</h2><strong className="summary-title">{plan.kind === "clone" ? plan.view.sourceName : form.name}</strong><p>{form.description}</p>
      <dl className="summary-list"><div><dt>テンプレート</dt><dd>{form.templateVersion}</dd></div><div><dt>環境名</dt><dd>{plan.kind === "clone" ? plan.view.sourceName : draft.displayName ?? view.displayName}</dd></div>
        <div><dt>バージョン</dt><dd>{plan.kind === "clone" ? plan.view.sourceVersion : view.version}</dd></div><div><dt>保存方式</dt><dd>{storage[plan.kind === "clone" ? plan.view.sourceStorageMethod : method].label}</dd></div></dl>
      {plan.kind === "clone" && <><p>元環境の版: {plan.view.sourceRevision}</p><p>元のテンプレートを使用します。選択肢は保存済みSnapshot内だけで、カタログの追加Versionは含みません。</p><dl className="summary-list">{Object.entries(plan.view.sourcePorts).map(([key, port]) => <div key={key}><dt>{key}</dt><dd>127.0.0.1:{port}</dd></div>)}</dl></>}
      {plan.kind === "create" && form.connections.map((connection) => <div key={connection.key} className="connection-summary"><strong>{connection.label}</strong>
        <p>{view.ports[connection.port] === undefined ? "ポート未確定" : `127.0.0.1:${view.ports[connection.port]}（候補）`}</p>
        {connection.inputs.map((key) => {
          const input = form.inputs.find((item) => item.key === key);
          return input && <p key={key}>{input.label}: {summaryValue(key)}</p>;
        })}
      </div>)}
      {view.concerns.length > 0 && <div className="notice" role="status">{view.concerns.map((item) => <p key={`${item.fieldPath}-${item.code}`}>{fieldLabel(item.fieldPath)}: {concernMessage(item.code)}</p>)}</div>}
    </section></aside></div></fieldset>
  </form>;
}
