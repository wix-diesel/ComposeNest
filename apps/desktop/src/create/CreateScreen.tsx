import { useEffect, useRef, useState } from "react";
import type { ApplicationClient } from "../ipc/ApplicationClient";
import { API_VERSION } from "../generated/ipc";
import type { ConfirmCreateRequest, CreatePlanView, CreateReceipt, PlanEdit } from "../generated/template-form";
import { ConfirmDialog } from "../shell/ConfirmDialog";
import { TemplateForm } from "../templates/TemplateForm";
import "./create.css";

type Draft = { preparation: Promise<CreatePlanView> | null; plan: CreatePlanView | null; confirmation: ConfirmCreateRequest | null; receipt: CreateReceipt | null };
// Preserve an uncertain request across local navigation, never in persistent browser storage.
const drafts = new WeakMap<ApplicationClient, Map<string, Draft>>();
function draftFor(client: ApplicationClient, templateId: string): Draft {
  let entries = drafts.get(client);
  if (!entries) { entries = new Map(); drafts.set(client, entries); }
  let draft = entries.get(templateId);
  if (!draft) { draft = { preparation: null, plan: null, confirmation: null, receipt: null }; entries.set(templateId, draft); }
  return draft;
}
const origin = (value: string) => value === "bundled" ? "同梱テンプレート" : value === "local" ? "ローカルテンプレート" : "出所不明";
const reason = (error: unknown) => error !== null && typeof error === "object" && "reason" in error && typeof error.reason === "string"
  ? error.reason : "接続を確認できませんでした。同じプランの受付結果を再確認してください。";

/** Prepares, reviews and accepts one create plan before opening its independent operation. */
export function CreateScreen({ client, templateId, returnTo = "instances" }: {
  client: ApplicationClient; templateId: string; returnTo?: "instances" | "templates";
}) {
  const saved = draftFor(client, templateId);
  const [plan, setPlan] = useState(saved.plan);
  const [review, setReview] = useState<CreatePlanView | null>(null);
  const [consent, setConsent] = useState(false);
  const [busy, setBusy] = useState(false);
  const [uncertain, setUncertain] = useState(saved.confirmation !== null);
  const [notice, setNotice] = useState<string | null>(null);
  const inFlight = useRef(false);
  const active = useRef(false);

  function remember(next: CreatePlanView) { saved.plan = next; if (active.current) setPlan(next); }
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
    const found = saved.receipt ?? await client.getCreateReceipt(saved.plan.planId);
    if (found) { accepted(found); return true; }
    return false;
  }

  useEffect(() => {
    active.current = true;
    if (saved.confirmation) {
      void reconcile();
    } else {
      saved.preparation ??= client.prepareCreate(templateId).then((next) => { saved.plan = next; return next; });
      void saved.preparation.then((next) => { if (active.current) setPlan(saved.plan ?? next); })
        .catch((error: unknown) => { if (active.current) setNotice(reason(error)); });
    }
    return () => { active.current = false; };
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
    if (inFlight.current || (!saved.confirmation && (!review || !consent))) return;
    if (!saved.confirmation && review) saved.confirmation = {
      context: { apiVersion: API_VERSION, requestId: crypto.randomUUID() }, planId: review.planId,
      revision: review.planRevision, confirmedPorts: { ...review.ports }, acceptPlaintext: consent,
    };
    const request = saved.confirmation;
    if (!request) return;
    inFlight.current = true; setBusy(true); setUncertain(true); setNotice(null);
    try { accepted(await client.confirmCreate(request)); }
    catch (error) {
      try {
        if (!await receipt()) {
          // A structured backend rejection is definitive; transport failure remains frozen.
          if (error !== null && typeof error === "object" && "code" in error) {
            const current = await client.viewCreate(request.planId);
            remember(current); saved.confirmation = null;
            if (active.current) setUncertain(false);
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
      if (saved.plan) await client.discardCreate(saved.plan.planId);
      else if (saved.preparation) {
        const prepared = await saved.preparation;
        await client.discardCreate(prepared.planId);
      }
      drafts.get(client)?.delete(templateId);
      if (active.current) client.navigate({ page: returnTo } );
    } catch (error) { if (active.current) setNotice(reason(error)); }
    finally { inFlight.current = false; if (active.current) setBusy(false); }
  }

  return <div className="create-screen" aria-busy={busy}>
    <ol className="steps" aria-label="作成手順"><li>✓ テンプレート選択</li><li aria-current="step">2 設定・確認</li><li>3 作成・起動</li></ol>
    {notice && <p className="notice" role="alert">{notice}</p>}
    {!plan && !notice && <p role="status">作成プランを準備しています…</p>}
    {plan && <>
      <p className="create-source">{origin(plan.templateOrigin)} · 登録版 <code>{plan.templateRevisionId}</code>。
        登録済み定義の初期値とCoreの生成値を使います。出所は実機検証済みを意味しません。</p>
      <TemplateForm key={`${plan.planId}-${uncertain}`} initialPlan={{ kind: "create", view: plan }} locked={busy || uncertain}
        onUpdate={async (current, edit) => {
          const next = await client.updateCreate(current.view.planId, edit as PlanEdit);
          remember(next); return { kind: "create", view: next };
        }}
        onReview={async (current) => {
          const next = await client.viewCreate(current.view.planId);
          remember(next); setConsent(false);
          if (next.concerns.length === 0) { setNotice(null); setReview(next); }
          else setNotice("未回答・入力エラー・ポートの確認事項を解消してください。");
          return { kind: "create", view: next };
        }} />
    </>}
    {uncertain && <section className="panel" aria-label="受付結果の確認"><h2>受付結果を確認しています</h2>
      <p>確定後は画面を閉じても処理は取り消されません。受付済みの場合は同じOperationを開きます。</p>
      <div className="actions"><button className="btn" disabled={busy} onClick={() => { void reconcile(); }}>受付結果を照会</button>
        <button className="btn primary" disabled={busy} onClick={() => { void confirm(); }}>同じ内容で再送</button></div>
    </section>}
    <button className="btn" disabled={busy || uncertain} onClick={() => { void cancel(); }}>作成を取り消す</button>
    {review && <ConfirmDialog title="この内容で環境を作成しますか？" onClose={() => setReview(null)}
      confirmLabel="作成・起動を確定" confirmDisabled={!consent || busy} onConfirm={() => { void confirm(); }}>
      <dl className="create-summary"><div><dt>環境名</dt><dd>{review.displayName}</dd></div>
        <div><dt>テンプレート</dt><dd>{review.templateForm.name} / {review.templateForm.templateVersion}</dd></div>
        <div><dt>出所</dt><dd>{origin(review.templateOrigin)}</dd></div><div><dt>Version</dt><dd>{review.version}</dd></div>
        <div><dt>保存方式</dt><dd>{review.storageMethod === "bind" ? "ホストフォルダー" : "Docker管理"}</dd></div>
        <div><dt>Plan版</dt><dd>{review.planRevision}</dd></div>
        {review.templateForm.ports.map((slot) => <div key={slot.key}><dt>{slot.label}</dt><dd>127.0.0.1:{review.ports[slot.key]} → {String(slot.container)}</dd></div>)}
      </dl><p>表示したポートとPlan版を確定時に再確認します。受付後は独立した処理状況画面へ進みます。</p>
      <label className="create-consent"><input type="checkbox" checked={consent} onChange={(event) => setConsent(event.target.checked)} />
        認証情報が端末内の設定と生成されるComposeファイルに暗号化せず平文で保存されることを確認しました。</label>
    </ConfirmDialog>}
  </div>;
}
