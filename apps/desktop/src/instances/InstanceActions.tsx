import { useEffect, useRef, useState } from "react";
import type { InstanceActionView } from "../generated/template-form";
import type { ApplicationClient } from "../ipc/ApplicationClient";
import { InstanceEdit } from "./InstanceEdit";
import "./instance-actions.css";

const runtime: Record<string, string> = { ready: "利用可能", stopped: "停止中", absent: "コンテナ不在", preparing: "準備中", unhealthy: "異常", unknown: "未確認" };
const operation: Record<string, string> = { Accepted: "受付済み", Executing: "実行中", Succeeded: "完了", Failed: "失敗・未解決", OutcomeUnknown: "結果不明", AwaitingDecision: "判断待ち", Abandoned: "解決済み" };
const phase: Record<string, string> = { inspect: "状態確認", storage: "データ確認", artifact: "設定確認", recreate: "コンテナ再作成", start: "起動中", stop: "停止中", restart: "再起動中", ready: "利用可能か確認", running: "実行状態を確認", stopped: "停止を確認", absent: "不在を確認", reconcile: "結果の確認が必要", start_required: "起動を選択", done: "完了" };
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
export function InstanceActions({ client, instanceId, edit = false }: { client: ApplicationClient; instanceId: string; edit?: boolean }) {
  return edit ? <InstanceEdit client={client} instanceId={instanceId} /> : <InstanceHeader client={client} instanceId={instanceId} />;
}

function InstanceHeader({ client, instanceId }: { client: ApplicationClient; instanceId: string }) {
  const [view, setView] = useState<InstanceActionView | null>(null);
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
        const current = await client.getInstanceActions(instanceId);
        if (!active) return;
        if (readingEpoch !== epoch.current || gate.current) { timer = setTimeout(read, 1000); return; }
        setView(current);
        if (draftRevision.current === null) draftRevision.current = current.revision;
        else if (current.revision !== draftRevision.current) setStale(true);
      } catch (error) { if (active && readingEpoch === epoch.current && !gate.current) { setMessage(failure(error)); setView(null); } }
      if (active) timer = setTimeout(read, 1000);
    }
    void read();
    return () => { active = false; mounted.current = false; clearTimeout(timer); };
  }, [client, instanceId]);

  async function submit(action: string) {
    if (gate.current || !view || client.hasInstanceChange(instanceId) || stale) return;
    gate.current = true; epoch.current++; setBusy(true); setMessage(null);
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
    gate.current = true; epoch.current++; setBusy(true);
    try {
      const current = await client.retryInstanceChange(instanceId);
      if (!mounted.current) return;
      setView(current); draftRevision.current = current.revision;
      setStale(false); setUncertain(false); setMessage(null);
    } catch (error) { if (mounted.current) { setMessage(failure(error)); setUncertain(client.hasInstanceChange(instanceId)); } }
    finally { gate.current = false; if (mounted.current) setBusy(false); }
  }
  const locked = busy || uncertain || stale || !view;
  const available = (action: string) => !locked && !!view?.actions.includes(action);
  const unresolved = view?.operationStatus !== null && view?.operationStatus !== undefined && !["Succeeded", "Abandoned"].includes(view.operationStatus);
  return <div className="instance-actions">
    <div className="page-heading"><div><h2>{view?.name ?? "環境を確認中"}</h2></div>
      <div className="actions">
        <button className="btn" disabled={locked || unresolved} onClick={() => client.navigate({ page: "instance-clone", instanceId })}>設定を複製</button>
        <button className="btn" disabled={!available("rename")} onClick={() => client.navigate({ page: "instance-edit", instanceId })}>編集</button>
        <button className="btn" disabled={!available(view?.actions.includes("start") ? "start" : "stop")} onClick={() => void submit(view?.actions.includes("start") ? "start" : "stop")}>{view?.actions.includes("start") ? "起動" : "停止"}</button>
        <button className="btn" disabled={!available("restart")} onClick={() => void submit("restart")}>再起動</button>
      </div>
    </div>
    {message && <p role="alert" className="notice">{message}</p>}
    {(uncertain || client.hasInstanceChange(instanceId)) && <p className="notice">受付結果の確認が必要です。別の変更は実行できません。</p>}
    {stale && <p className="notice">表示していた版が古くなりました。現在の状態を再確認してください。</p>}
    {(view?.runtimeStatus === "absent" || view?.operationPhase === "start_required") && <p className="notice">コンテナが不在です。再起動ではなく「起動」を選んでください。未解決の処理がある場合は先に解決してください。</p>}
    {unresolved && <p className="notice">未解決の処理があるため、別の変更は実行できません。</p>}
    <section className="panel" aria-label="環境の状態"><div className="panel-title"><h2>環境の状態</h2><small>最終確認 {view?.observedAt ?? "未確認"}</small></div>
      <dl className="instance-state"><div><dt>実行状態</dt><dd><span className="badge" data-runtime={view?.runtimeStatus}>{runtime[view?.runtimeStatus ?? "unknown"] ?? "未確認"}</span></dd></div>
        <div><dt>直前の処理</dt><dd aria-live="polite">{view?.operationId ? `${actionLabel[view.operationKind ?? ""] ?? "処理"} · ${operation[view.operationStatus ?? ""] ?? "未確認"}` : "なし"}</dd></div>
        <div><dt>版</dt><dd>{view?.revision ?? "未確認"}</dd></div></dl>
      {view?.operationId && <p className="operation-link">処理ID: <code>{view.operationId}</code> · 段階: {phase[view.operationPhase ?? ""] ?? "確認中"}</p>}
      <button className="btn small" disabled={busy} onClick={() => void refresh()}>{uncertain ? "受付を再確認" : "現在の状態を再確認"}</button>
    </section>

  </div>;
}
