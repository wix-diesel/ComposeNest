import { useEffect, useRef, useState, type Ref } from "react";
import type { InstanceActionView, InstanceDetailView } from "../generated/template-form";
import type { ApplicationClient } from "../ipc/ApplicationClient";
import { operationPhaseLabel } from "./operationPhase";
import { InstanceEdit } from "./InstanceEdit";
import { InstanceDetailTabs } from "./InstanceDetailTabs";
import { Icon } from "../shell/Icon";
import "./instance-actions.css";

const runtime: Record<string, string> = { ready: "利用可能", stopped: "停止中", absent: "コンテナ不在", preparing: "準備中", unhealthy: "異常", unknown: "未確認" };
const operation: Record<string, string> = { Accepted: "受付済み", Executing: "実行中", Succeeded: "完了", Failed: "失敗・未解決", OutcomeUnknown: "結果不明", AwaitingDecision: "判断待ち", Abandoned: "解決済み" };
const actionLabel: Record<string, string> = { start: "起動", stop: "停止", restart: "再起動", create: "作成", clone: "複製", rename: "名前変更" };
function failure(error: unknown): string {
  const code = typeof error === "object" && error !== null && "code" in error ? error.code : null;
  switch (code) {
    case "NAME_OR_REQUEST_CONFLICT": return "同じ環境名が使用されています。別の名前を入力してください。";
    case "INSTANCE_RENAME_UNCONFIRMED": return "名前変更の確定を確認できませんでした。入力内容を保持しています。受付を再確認してください。";
    case "INSTANCE_STALE": return "環境が更新されています。現在の状態を再確認し、変更内容を確認し直してください。";
    case "INSTANCE_MISSING": return "対象の環境が見つかりません。環境一覧へ戻ってください。";
    case "INSTANCE_INPUT_INVALID": return "環境名は空白だけにせず、制御文字を含まない100文字以内で入力してください。";
    case "INSTANCE_ACTION_UNAVAILABLE": return "この操作は現在利用できません。未解決の処理と実行状態を確認してください。";
    default: return "接続または受付結果を確認できませんでした。受付を再確認してください。";
  }
}

/** Routes edit requests to the stopped-port editor and retains the detail header actions. */
export function InstanceActions({ client, instanceId, edit = false, headingRef, onAbout }: { client: ApplicationClient; instanceId: string; edit?: boolean; headingRef?: Ref<HTMLHeadingElement>; onAbout?: () => void }) {
  return edit ? <InstanceEdit client={client} instanceId={instanceId} /> : <InstanceHeader client={client} instanceId={instanceId} headingRef={headingRef} onAbout={onAbout} />;
}

function InstanceHeader({ client, instanceId, headingRef, onAbout }: { client: ApplicationClient; instanceId: string; headingRef?: Ref<HTMLHeadingElement>; onAbout?: () => void }) {
  const [view, setView] = useState<InstanceActionView | null>(null);
  const [detail, setDetail] = useState<InstanceDetailView | null>(null);
  const [message, setMessage] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [uncertain, setUncertain] = useState(client.hasInstanceChange(instanceId));
  const [stale, setStale] = useState(false);
  const gate = useRef(false);
  const epoch = useRef(0);
  const mounted = useRef(false);
  const draftRevision = useRef<number | null>(null);

  useEffect(() => {
    mounted.current = true;
    let active = true;
    let timer: ReturnType<typeof setTimeout>;
    async function read() {
      const readingEpoch = epoch.current;
      try {
        const snapshot = await client.getInstanceDetail(instanceId);
        const current = snapshot.state;
        if (!active) return;
        if (readingEpoch !== epoch.current || gate.current) { timer = setTimeout(read, 1000); return; }
        setView(current); setDetail(snapshot);
        if (draftRevision.current === null) draftRevision.current = current.revision;
        else if (current.revision !== draftRevision.current) setStale(true);
      } catch (error) { if (active && readingEpoch === epoch.current && !gate.current) { setMessage(failure(error)); setView(null); setDetail(null); } }
      if (active) timer = setTimeout(read, 1000);
    }
    void read();
    return () => { active = false; mounted.current = false; clearTimeout(timer); };
  }, [client, instanceId]);

  async function submit(action: string) {
    if (gate.current || !view || client.hasInstanceChange(instanceId) || stale) return;
    gate.current = true; epoch.current++; setBusy(true); setMessage(null); setDetail(null);
    try {
      const current = await client.changeInstance(instanceId, view.revision, action);
      if (!mounted.current) return;
      setView(current); draftRevision.current = current.revision; setStale(false);
    } catch (error) {
      if (mounted.current) { setMessage(failure(error)); setUncertain(client.hasInstanceChange(instanceId)); setStale(!client.hasInstanceChange(instanceId)); }
    } finally { gate.current = false; if (mounted.current) setBusy(false); }
  }
  async function refresh() {
    if (gate.current) return;
    gate.current = true; epoch.current++; setBusy(true); setDetail(null);
    try {
      await client.retryInstanceChange(instanceId);
      if (!mounted.current) return;
      const snapshot = await client.getInstanceDetail(instanceId);
      if (!mounted.current) return;
      setView(snapshot.state); setDetail(snapshot); draftRevision.current = snapshot.state.revision;
      setStale(false); setUncertain(false); setMessage(null);
    } catch (error) { if (mounted.current) { setView(null); setMessage(failure(error)); setUncertain(client.hasInstanceChange(instanceId)); } }
    finally { gate.current = false; if (mounted.current) setBusy(false); }
  }
  const locked = busy || uncertain || stale || !view;
  const available = (action: string) => !locked && !!view?.actions.includes(action);
  const unresolved = view?.operationStatus !== null && view?.operationStatus !== undefined && !["Succeeded", "Abandoned"].includes(view.operationStatus);
  return <div className="instance-actions">
    <div className="page-heading"><div><div className="eyebrow">環境一覧 / 環境の詳細</div><h1 id="screen" ref={headingRef} tabIndex={-1}>{view?.name ?? "環境の詳細"}</h1><p className="subtitle">{detail ? `${detail.instance.templateId} ${detail.instance.selectedVersion}` : "詳細を確認中"}</p></div>
      <div className="actions">
        <button className="btn" disabled={locked || unresolved} onClick={() => client.navigate({ page: "instance-clone", instanceId })}><Icon name="copy" />設定を複製</button>
        <button className="btn" disabled={!available("rename")} onClick={() => client.navigate({ page: "instance-edit", instanceId })}><Icon name="edit" />編集</button>
        <button className="btn" disabled={!available(view?.actions.includes("start") ? "start" : "stop")} onClick={() => void submit(view?.actions.includes("start") ? "start" : "stop")}><Icon name={view?.actions.includes("start") ? "play" : "stop"} />{view?.actions.includes("start") ? "起動" : "停止"}</button>
        <button className="btn" disabled={!available("restart")} onClick={() => void submit("restart")}><Icon name="refresh" />再起動</button>
        <button className="btn" onClick={() => client.navigate({ page: "instances" })}>戻る</button>
        {onAbout && <button className="btn" onClick={onAbout}>この画面について</button>}
      </div>
    </div>
    {message && <p role="alert" className="notice">{message}</p>}
    {(uncertain || client.hasInstanceChange(instanceId)) && <p className="notice">受付結果の確認が必要です。別の変更は実行できません。</p>}
    {stale && <p className="notice">表示していた版が古くなりました。現在の状態を再確認してください。</p>}
    {(view?.runtimeStatus === "absent" || view?.operationPhase === "start_required") && <p className="notice">コンテナが不在です。再起動ではなく「起動」を選んでください。未解決の処理がある場合は先に解決してください。</p>}
    {unresolved && <p className="notice">未解決の処理があるため、別の変更は実行できません。</p>}
    <section className="panel" aria-label="環境の状態"><div className="panel-title"><h2>環境の状態</h2><small>最終観測（保存値） {view?.observedAt ? `${view.observedAt} UTC` : "未確認"}{detail?.instance.observation?.freshness !== "fresh" && view?.observedAt ? "（過去の観測）" : ""}</small></div>
      <dl className="instance-state"><div><dt>実行状態</dt><dd><span className="badge" data-runtime={view?.runtimeStatus}>{runtime[view?.runtimeStatus ?? "unknown"] ?? "未確認"}</span></dd></div>
        <div><dt>直前の処理</dt><dd aria-live="polite">{!view ? "未取得" : view.operationId ? `${actionLabel[view.operationKind ?? ""] ?? "処理"} · ${operation[view.operationStatus ?? ""] ?? "未確認"}` : "なし"}</dd></div>
        <div><dt>構成の照合</dt><dd>外部構成は未確認</dd><dd>{detail ? `保存 r${detail.instance.specRevision} / 適用 ${detail.instance.appliedSpecRevision === null ? "未確認" : `r${detail.instance.appliedSpecRevision}`}` : "適用版は未取得"}</dd></div></dl>
      {view?.operationId && <p className="operation-link">処理ID: <code>{view.operationId}</code> · 段階: {operationPhaseLabel(view.operationPhase)} <button className="btn small" onClick={() => client.navigate({ page: "operation", operationId: view.operationId!, instanceId })}>処理状況を開く</button></p>}
      <button className="btn small" disabled={busy} onClick={() => void refresh()}>{uncertain ? "受付を再確認" : "現在の状態を再確認"}</button>
    </section>
    <InstanceDetailTabs detail={detail} />
  </div>;
}
