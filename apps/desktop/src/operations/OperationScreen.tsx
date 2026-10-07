import { useEffect, useRef, useState } from "react";
import type { RefObject } from "react";
import type { ApplicationClient } from "../ipc/ApplicationClient";
import type { OperationProgressView } from "../generated/template-form";
import { operationPhaseLabel } from "../instances/operationPhase";
import "./operation.css";
import { RecoveryPanel } from "./RecoveryPanel";

const stages = ["設定と保存領域を準備", "イメージを取得", "Composeを検証・コンテナを作成", "起動・準備完了を確認", "接続情報を案内"];
const phases: Record<string, number> = { accepted: 0, prepare: 0, storage: 0, image: 1, artifact: 2, config: 2, create: 2, inspect_created: 2, start: 3, ready: 3 };
const statuses: Record<string, string> = { Accepted: "受付済み", Executing: "処理中", Succeeded: "完了", Failed: "失敗", OutcomeUnknown: "結果不明", AwaitingDecision: "判断待ち", Abandoned: "解決済み" };
const terminal = (status: string) => ["Succeeded", "Failed", "OutcomeUnknown", "AwaitingDecision", "Abandoned"].includes(status);

/** Restores durable progress on open, every notification and periodic read-only reconciliation. */
export function OperationScreen({ client, operationId, instanceId, headingRef }: {
  client: ApplicationClient; operationId: string; instanceId?: string; headingRef: RefObject<HTMLHeadingElement | null>;
}) {
  const [view, setView] = useState<OperationProgressView | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const [notificationFailure, setNotificationFailure] = useState(false);
  const [busy, setBusy] = useState(false);
  const refresh = useRef<() => void>(() => {});
  useEffect(() => {
    let active = true, reading = false, queued = false, finished = false;
    let unlisten: (() => void) | undefined;
    let targetId = instanceId;
    async function read() {
      if (!active) return;
      if (reading) { queued = true; return; }
      reading = true; setBusy(true);
      try {
        const next = await client.getOperation(operationId);
        if (targetId && next.instance.id !== targetId) throw new Error("operation_target_mismatch");
        if (active) {
          targetId = next.instance.id; finished = terminal(next.operation.status);
          setView(next); setNotice(null);
        }
      } catch { if (active) { finished = false; setNotice("処理状況を取得できませんでした。前回の記録がある場合は表示しています。再照会してください。"); } }
      finally {
        reading = false;
        if (active) { setBusy(false); if (queued) { queued = false; void read(); } }
      }
    }
    refresh.current = () => { void read(); };
    // Every hint triggers a read, including sequence gaps and completion at the same sequence.
    void client.subscribeOperation((event) => {
      if (event?.operationId === operationId && (!targetId || event.instanceId === targetId)
        && Number.isSafeInteger(event.sequence) && event.sequence >= 0) void read();
    }).then((release) => { if (active) unlisten = release; else release(); })
      .catch(() => { if (active) setNotificationFailure(true); })
      .finally(() => { if (active) void read(); });
    void read();
    // Also recover a missing final notification, even when no later sequence reveals the gap.
    const timer = window.setInterval(() => { if (!finished) void read(); }, 2000);
    return () => { active = false; window.clearInterval(timer); unlisten?.(); };
  }, [client, operationId, instanceId]);

  const operation = view?.operation;
  const target = view?.instance;
  const creation = operation?.kind === "create" || operation?.kind === "clone";
  const deletion = operation?.kind === "delete";
  const succeeded = operation?.status === "Succeeded" && !notice
    && (!deletion || (target?.lifecycle === "retired" && target.lastOperation?.id === operation.id && !!view?.completedAt));
  const failed = operation?.status === "Failed";
  const stage = phases[operation?.phase ?? ""];
  const ready = succeeded && creation && target?.lastOperation?.id === operation?.id && target?.runtimeStatus === "ready";
  const title = !operation ? "処理状況" : succeeded ? "処理が完了しました" : failed ? "処理に失敗しました"
    : terminal(operation.status) ? "処理結果の確認が必要です" : creation ? "環境を作成中" : "処理状況";
  return <div className="operation-screen" aria-busy={busy}>
    <div className="page-heading"><div><div className="eyebrow">環境一覧 / 処理状況</div>
      <h1 id="screen" ref={headingRef} tabIndex={-1}>{title}</h1><p className="subtitle">{target?.name ?? "対象の環境を照会しています。"}</p>
    </div><div className="actions"><button className="btn" disabled={busy} onClick={() => refresh.current()}>再照会</button>
      <button className="btn" onClick={() => client.navigate({ page: "instances" })}>閉じる</button></div></div>
    {notice && <p className="notice warning" role="alert">{notice}</p>}
    {notificationFailure && <p className="notice" role="status">進捗通知を受信できないため、定期照会で処理状況を確認します。</p>}
    {!view && !notice && <p role="status">処理状況を照会しています…</p>}
    {view && operation && target && <div className="operation-columns"><div><section className="panel" aria-label="処理の進捗">
      <div className="panel-title"><h2>{creation ? "作成・起動の進捗" : operationPhaseLabel(operation.phase)}</h2>
        <span className={`badge ${failed ? "operation-failed" : ""}`}>{statuses[operation.status] ?? "未確認"}{notice ? "（前回の記録）" : ""}</span></div>
      {creation ? <><div className="operation-progress" aria-hidden="true"><div style={{ width: `${succeeded ? 100 : stage === undefined ? 0 : stage * 20}%` }} /></div>
        <ol className="operation-timeline" aria-label="処理段階">{stages.map((label, index) => {
          const done = succeeded || (stage !== undefined && index < stage);
          const current = !succeeded && index === stage;
          const state = done ? "確認済み" : current ? failed ? "失敗" : terminal(operation.status) ? "要確認" : "処理中" : "待機";
          return <li key={label} aria-current={current ? "step" : undefined}><span className={`operation-check ${done ? "done" : ""}`}>{done ? "✓" : index + 1}</span>
            <div><h3>{label}</h3>{current && <p>{operationPhaseLabel(operation.phase)}</p>}</div><span>{state}</span></li>;
        })}</ol></> : <p>保存された処理段階: {operationPhaseLabel(operation.phase)}</p>}
      <p>開始: {operation.startedAt} UTC{view.completedAt && ` · 終了: ${view.completedAt} UTC`}</p>
      <p>処理ID: <code>{operationId}</code> · 記録 sequence: {view.sequence}</p>
    </section><div className="notice" role="status">
      {deletion ? <><strong>{succeeded ? "環境の削除が完了しました。データは残ります" : "環境の削除は完了していません"}</strong>
        <p>{succeeded ? "環境一覧から除外しました。データ・元の設定・Composeの所在は保持データで確認できます。"
          : "受付済み・失敗・結果不明の環境は一覧に残ります。削除完了を確認するまで、別の変更は実行できません。"}</p>
        <button className="btn" onClick={() => client.navigate(succeeded ? { page: "retained" } : { page: "instance-detail", instanceId: target.id })}>{succeeded ? "保持データを確認" : "対象の環境を確認"}</button></>
        : ready ? <><strong>準備完了を確認しました</strong><p>保存されたHealth確認結果に基づく接続情報です。</p>
        {target.connections.map((connection) => <p key={connection.slot}>{connection.label}: <code>{connection.port.hostIp}:{connection.port.hostPort}</code></p>)}
        <button className="btn primary" onClick={() => client.navigate({ page: "instance-detail", instanceId: target.id })}>環境の詳細・接続設定を確認</button></>
        : <><strong>{notice ? "現在の結果は未確認です" : succeeded ? "処理の完了記録があります" : failed ? "処理は完了していません" : terminal(operation.status) ? "実際の結果を確認してください" : "準備完了の確認を待っています"}</strong>
          <p>{failed || terminal(operation.status) ? "対象の環境と保存データは保持されます。現在の状態は環境の詳細で確認できます。" : "起動や時間の経過だけでは利用可能と表示しません。"}</p>
          <button className="btn" onClick={() => client.navigate({ page: "instance-detail", instanceId: target.id })}>対象の環境を確認</button></>}
    </div>
    {view.lastFailureStatus && <p className="notice">直前の失敗・処理結果: {statuses[view.lastFailureStatus] ?? "要確認"}。現在の処理結果: {statuses[operation.status] ?? "未確認"}。</p>}
    {target.lifecycle !== "retired" && <RecoveryPanel client={client} instanceId={target.id} operationId={operationId} status={operation.status} onReconciled={() => refresh.current()} />}
    </div><aside><section className="panel"><h2>対象の環境</h2><p>現在の保存設定</p><dl className="operation-summary">
      <div><dt>サービス</dt><dd>{target.templateId} {target.selectedVersion}</dd></div>
      <div><dt>接続ポート</dt><dd>{target.ports.map((port) => <div key={port.slot}><code>{port.hostIp}:{port.hostPort}</code></div>)}</dd></div>
      <div><dt>保存方式</dt><dd>{target.storageMethod === "bind" ? "bind mount" : target.storageMethod === "volume" ? "named volume" : "未確認"}</dd></div>
      <div><dt>処理開始</dt><dd>{operation.startedAt} UTC</dd></div>
    </dl></section><div className="notice"><strong>閉じても確定済みの処理は続きます</strong><p>画面を閉じても処理は取り消されません。同じ処理IDで状況を再確認できます。</p></div></aside></div>}
  </div>;
}
