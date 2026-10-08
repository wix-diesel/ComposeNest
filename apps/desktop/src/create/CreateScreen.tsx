import { useEffect, useMemo, useRef, useState } from "react";
import type { ApplicationClient } from "../ipc/ApplicationClient";
import { API_VERSION } from "../generated/ipc";
import type { CloneEdit, ClonePlanView, ConfirmCloneRequest, ConfirmCreateRequest, CreatePlanView, CreateReceipt, PlanEdit } from "../generated/template-form";
import { ConfirmDialog } from "../shell/ConfirmDialog";
import { CloneDiff } from "../templates/CloneDiff";
import { TemplateForm } from "../templates/TemplateForm";
import "./create.css";

type PlanView = CreatePlanView | ClonePlanView;
type Confirmation = ConfirmCreateRequest | ConfirmCloneRequest;
type Draft = {
  preparation: Promise<PlanView> | null; plan: PlanView | null; confirmation: Confirmation | null; receipt: CreateReceipt | null;
  owner: object | null; release: Promise<void> | null; cleanupError: string | null;
};
// Preserve an uncertain request across local navigation, never in persistent browser storage.
const drafts = new WeakMap<ApplicationClient, Map<string, Draft>>();
function draftFor(client: ApplicationClient, templateId: string): Draft {
  let entries = drafts.get(client);
  if (!entries) { entries = new Map(); drafts.set(client, entries); }
  let draft = entries.get(templateId);
  if (!draft) { draft = { preparation: null, plan: null, confirmation: null, receipt: null, owner: null, release: null, cleanupError: null }; entries.set(templateId, draft); }
  return draft;
}
const origin = (value: string) => value === "bundled" ? "同梱テンプレート" : value === "local" ? "ローカルテンプレート" : "出所不明";
const reason = (error: unknown) => error !== null && typeof error === "object" && "reason" in error && typeof error.reason === "string"
  ? error.reason : "接続を確認できませんでした。同じプランの受付結果を再確認してください。";

/** Prepares, reviews and accepts one create plan before opening its independent operation. */
export function CreateScreen({ client, templateId, returnTo }: { client: ApplicationClient; templateId: string; returnTo?: "instances" | "templates" }) {
  return <PlanScreen client={client} selectionId={templateId} kind="create" returnTo={returnTo} />;
}
/** Shares confirmation, cancellation and acceptance recovery with new creation. */
export function CloneScreen({ client, sourceId }: { client: ApplicationClient; sourceId: string }) {
  return <PlanScreen client={client} selectionId={sourceId} kind="clone" />;
}
function PlanScreen({ client, selectionId, kind, returnTo = "instances" }: {
  client: ApplicationClient; selectionId: string; kind: "create" | "clone"; returnTo?: "instances" | "templates";
}) {
  const templateId = `${kind}:${selectionId}`;
  const clone = kind === "clone";
  const prepare = () => clone ? client.prepareClone(selectionId) : client.prepareCreate(selectionId);
  const viewPlan = (id: string) => clone ? client.viewClone(id) : client.viewCreate(id);
  async function discard(id: string) {
    try { await (clone ? client.discardClone(id) : client.discardCreate(id)); }
    catch (error) {
      if (error === null || typeof error !== "object" || !("code" in error) || error.code !== "PLAN_NOT_FOUND") throw error;
    }
  }
  const saved = useMemo(() => draftFor(client, templateId), [client, templateId]);
  const [plan, setPlan] = useState(saved.release ? null : saved.plan);
  const [review, setReview] = useState<PlanView | null>(null);
  const [consent, setConsent] = useState(false);
  const [configurationOnly, setConfigurationOnly] = useState(false);
  const [sourceChanged, setSourceChanged] = useState(false);
  const [busy, setBusy] = useState(false);
  const [uncertain, setUncertain] = useState(saved.confirmation !== null);
  const [notice, setNotice] = useState<string | null>(null);
  const inFlight = useRef(false);
  const active = useRef(false);
  const reviewTrigger = useRef<HTMLButtonElement | null>(null);

  function remember(next: PlanView) { saved.plan = next; if (active.current) setPlan(next); }
  function accepted(receipt: CreateReceipt) {
    if (receipt.planId !== saved.plan?.planId || receipt.confirmedRevision !== (saved.confirmation?.revision ?? saved.receipt?.confirmedRevision))
      throw new Error("invalid_create_receipt");
    saved.receipt = receipt;
    if (active.current) {
      drafts.get(client)?.delete(templateId); saved.confirmation = null;
      client.navigate({ page: "operation", operationId: receipt.operationId, instanceId: receipt.instanceId });
    }
  }

  async function receipt(): Promise<boolean> {
    if (!saved.plan) return false;
    const found = saved.receipt ?? await (clone ? client.getCloneReceipt(saved.plan.planId) : client.getCreateReceipt(saved.plan.planId));
    if (found) { accepted(found); return true; }
    return false;
  }

  useEffect(() => {
    active.current = true;
    const owner = {};
    saved.owner = owner;
    async function open() {
      if (saved.release) await saved.release;
      if (!active.current || saved.owner !== owner) return;
      if (saved.cleanupError) setNotice(saved.cleanupError);
      if (saved.confirmation) { void reconcile(); return; }
      saved.preparation ??= prepare().then((next) => { saved.plan = next; return next; });
      try {
        const next = await saved.preparation;
        if (active.current) setPlan(saved.plan ?? next);
      } catch (error) { if (active.current) setNotice(reason(error)); }
    }
    void open();
    return () => {
      active.current = false;
      // StrictMode remounts synchronously; only the final owner releases a draft.
      queueMicrotask(() => {
        if (saved.owner !== owner || drafts.get(client)?.get(templateId) !== saved || saved.confirmation || saved.receipt || saved.release) return;
        saved.release = (async () => {
          let prepared = saved.plan;
          if (!prepared && saved.preparation) {
            try { prepared = await saved.preparation; } catch { /* Failed preparation allocated no plan. */ }
          }
          try {
            if (prepared) await discard(prepared.planId);
            saved.plan = null; saved.preparation = null; saved.cleanupError = null;
            if (saved.owner === owner && drafts.get(client)?.get(templateId) === saved) drafts.get(client)?.delete(templateId);
          } catch (error) { saved.cleanupError = reason(error); }
          finally { saved.release = null; }
        })();
      });
    };
  }, [client, templateId, saved]);

  async function reconcile() {
    if (inFlight.current) return;
    inFlight.current = true; setBusy(true);
    try {
      if (!await receipt() && active.current) setNotice("受付記録はまだありません。同じ内容・同じ要求IDで再送できます。");
    } catch (error) { if (active.current) setNotice(reason(error)); }
    finally { inFlight.current = false; if (active.current) setBusy(false); }
  }

  async function confirm() {
    if (inFlight.current || (!saved.confirmation && (!review || !consent || (clone && !configurationOnly)))) return;
    if (!saved.confirmation && review) saved.confirmation = {
      context: { apiVersion: API_VERSION, requestId: crypto.randomUUID() }, planId: review.planId,
      revision: review.planRevision, confirmedPorts: { ...review.ports }, acceptPlaintext: consent, ...(clone ? { acceptConfigurationOnly: configurationOnly } : {}),
    };
    const request = saved.confirmation;
    if (!request) return;
    inFlight.current = true; setBusy(true); setUncertain(true); setNotice(null);
    try { accepted(await (clone ? client.confirmClone(request as ConfirmCloneRequest) : client.confirmCreate(request))); }
    catch (error) {
      try {
        if (!await receipt()) {
          // A structured backend rejection is definitive; transport failure remains frozen.
          if (error !== null && typeof error === "object" && "code" in error) {
            saved.confirmation = null;
            if (active.current) { setReview(null); setConsent(false); setConfigurationOnly(false); }
            try { remember(await refreshed(request.planId)); }
            finally { if (active.current) setUncertain(false); }
          }
          if (active.current) setNotice(reason(error));
        }
      } catch (lookupError) { if (active.current) setNotice(reason(lookupError)); }
    } finally { inFlight.current = false; if (active.current) setBusy(false); }
  }

  async function cancel() {
    if (inFlight.current || saved.confirmation) return;
    inFlight.current = true; setBusy(true);
    try {
      if (!saved.plan && notice) {
        drafts.get(client)?.delete(templateId);
        client.navigate({ page: returnTo });
        return;
      }
      if (saved.plan) await discard(saved.plan.planId);
      else if (saved.preparation) {
        const prepared = await saved.preparation;
        await discard(prepared.planId);
      }
      drafts.get(client)?.delete(templateId);
      if (active.current) client.navigate({ page: returnTo } );
    } catch (error) { if (active.current) setNotice(reason(error)); }
    finally { inFlight.current = false; if (active.current) setBusy(false); }
  }

  function sourceFailure(error: unknown) {
    if (clone && error !== null && typeof error === "object" && (("fieldPath" in error && error.fieldPath === "sourceId") || ("code" in error && error.code === "PLAN_NOT_FOUND"))) {
      setSourceChanged(true); setReview(null); setConsent(false); setConfigurationOnly(false);
      setNotice("複製元が更新されました。元の設定を読み直し、変更内容を再確認してください。");
    }
  }
  async function refreshed(id: string) {
    try { return await viewPlan(id); } catch (error) { sourceFailure(error); throw error; }
  }
  async function restart() {
    if (inFlight.current || saved.confirmation) return;
    inFlight.current = true; setBusy(true);
    try {
      if (saved.plan) await discard(saved.plan.planId);
      saved.plan = null; saved.preparation = null; setPlan(null); setReview(null); setConfigurationOnly(false); setConsent(false);
      saved.preparation = prepare();
      remember(await saved.preparation); setSourceChanged(false); setNotice(null);
    } catch (error) { saved.preparation = null; setNotice(reason(error)); }
    finally { inFlight.current = false; setBusy(false); }
  }

  return <div className="create-screen" aria-busy={busy}>
    {!clone && <ol className="steps" aria-label="作成手順"><li>✓ テンプレート選択</li><li aria-current="step">2 設定・確認</li><li>3 作成・起動</li></ol>}
    {notice && <p className="notice" role="alert">{notice}</p>}
    {!plan && !notice && <p role="status">作成プランを準備しています…</p>}
    {sourceChanged && <button className="btn" disabled={busy || uncertain} onClick={() => { void restart(); }}>複製元を読み直す</button>}
    {plan && <>
      {"templateOrigin" in plan && <p className="create-source">{origin(plan.templateOrigin)} · 登録版 <code>{plan.templateRevisionId}</code>。
        登録済み定義の初期値とCoreの生成値を使います。出所は実機検証済みを意味しません。</p>}
      <TemplateForm key={`${plan.planId}-${uncertain}`} initialPlan={"sourceId" in plan ? { kind: "clone", view: plan } : { kind: "create", view: plan }} locked={busy || uncertain || sourceChanged}
        onUpdate={async (current, edit) => {
          try {
            const next = current.kind === "clone" ? await client.updateClone(current.view.planId, edit as CloneEdit) : await client.updateCreate(current.view.planId, edit as PlanEdit);
            remember(next); return "sourceId" in next ? { kind: "clone", view: next } : { kind: "create", view: next };
          } catch (error) { sourceFailure(error); throw error; }
        }}
        onReview={async (current, trigger) => {
          reviewTrigger.current = trigger;
          const next = await refreshed(current.view.planId);
          remember(next); setConsent(false); setConfigurationOnly(false);
          if (next.concerns.length === 0) { setNotice(null); setReview(next); }
          else setNotice("未回答・入力エラー・ポートの確認事項を解消してください。");
          return "sourceId" in next ? { kind: "clone", view: next } : { kind: "create", view: next };
        }} />
    </>}
    {uncertain && <section className="panel" aria-label="受付結果の確認"><h2>受付結果を確認しています</h2>
      <p>確定後は画面を閉じても処理は取り消されません。受付済みの場合は同じOperationを開きます。</p>
      <div className="actions"><button className="btn" disabled={busy} onClick={() => { void reconcile(); }}>受付結果を照会</button>
        <button className="btn primary" disabled={busy} onClick={() => { void confirm(); }}>同じ内容で再送</button></div>
    </section>}
    <button className="btn" disabled={busy || uncertain} onClick={() => { void cancel(); }}>作成を取り消す</button>
    {review && <ConfirmDialog title={clone ? "設定を複製して環境を作成しますか？" : "この内容で環境を作成しますか？"} onClose={() => setReview(null)} returnFocus={reviewTrigger.current}
      confirmLabel="作成・起動を確定" confirmDisabled={!consent || (clone && !configurationOnly) || busy} onConfirm={() => { void confirm(); }}>
      <dl className="create-summary"><div><dt>環境名</dt><dd>{review.displayName}</dd></div>
        <div><dt>テンプレート</dt><dd>{review.templateForm.name} / {review.templateForm.templateVersion}</dd></div>
        <div><dt>出所</dt><dd>{"templateOrigin" in review ? origin(review.templateOrigin) : "複製元の保存済み定義"}</dd></div><div><dt>Version</dt><dd>{review.version}</dd></div>
        <div><dt>保存方式</dt><dd>{review.storageMethod === "bind" ? "ホストフォルダー" : "Docker管理"}</dd></div>
        <div><dt>Plan版</dt><dd>{review.planRevision}</dd></div>
        {review.templateForm.ports.map((slot) => <div key={slot.key}><dt>{slot.label}</dt><dd>127.0.0.1:{review.ports[slot.key]} → {String(slot.container)}</dd></div>)}
      </dl>{"sourceId" in review && <CloneDiff plan={review} />}<p>表示したポートとPlan版を確定時に再確認します。受付後は独立した処理状況画面へ進みます。</p>
      {clone && <label className="create-consent"><input type="checkbox" checked={configurationOnly} onChange={(event) => setConfigurationOnly(event.target.checked)} />
        データが複製されず、新しい環境が作られることを確認しました。複製元のデータや稼働状態は変更しません。</label>}
      <label className="create-consent"><input type="checkbox" checked={consent} onChange={(event) => setConsent(event.target.checked)} />
        認証情報が端末内の設定と生成されるComposeファイルに暗号化せず平文で保存されることを確認しました。</label>
    </ConfirmDialog>}
  </div>;
}
