import { useEffect, useRef, useState } from "react";
import type { RecoveryView } from "../generated/template-form";
import type { ApplicationClient } from "../ipc/ApplicationClient";
import { ConfirmDialog } from "../shell/ConfirmDialog";

const labels: Record<string, string> = { complete: "観測結果を確定", retry: "同じ処理を再試行", abandon: "安全に終了",
  retry_ports: "同じポート変更を再試行", restore_ports: "旧ポートに戻す", reconcile_ports: "ポートの反映結果を確定",
  propose_ports: "新しいポート候補を確認", confirm_ports: "候補ポートを適用", restore_external: "外部編集を退避して保存設定に戻す", retry_external: "同じ復帰処理を再試行" };
const reasons: Record<string, string> = { CLI_TERMINATION_UNCONFIRMED: "前のCLIが終了したことを確認できません。生存中または終了不明のため変更を保留します。",
  RUNTIME_TARGET_MISMATCH: "記録済みのDocker接続先・Engineとの一致を確認できません。",
  STORAGE_MISSING: "以前存在した保存領域が見つかりません（Missing）。領域を作り直さず保留します。",
  STORAGE_UNVERIFIED: "保存領域の存在・所有情報を確認できません。",
  OWNERSHIP_UNKNOWN: "実行資源の所有が不明です。名前だけでは変更を許可しません。",
  ARTIFACT_MODIFIED_OR_MISSING: "生成済み設定が外部編集されたか、所在を確認できません。",
  CONFIGURATION_MISMATCH: "コンテナの構成と保存設定が一致していません。",
  OPERATION_EXECUTING: "処理が実行中です。変更を重ねず、終了を待ってください。",
  OPERATION_TARGET_STALE: "対象の環境に別の処理があります。最新の処理状況を確認してください。",
  RECOVERY_HELD: "安全に確定・再試行できる条件を確認できないため保留しています。" };
const runtime: Record<string, string> = { ready: "Ready（利用可能）", stopped: "停止中", absent: "実行資源なし", present: "存在・準備未完了", unknown: "未確認" };
const outcomes: Record<string, string> = { Failed: "失敗", OutcomeUnknown: "結果不明", AwaitingDecision: "判断待ち", Succeeded: "完了", Abandoned: "解決済み" };

/** Displays fresh Core evidence and explicit choices, never optimistic mock success. */
export function RecoveryPanel({ client, instanceId, operationId, status, onReconciled }: {
  client: ApplicationClient; instanceId: string; operationId: string; status: string; onReconciled: () => void;
}) {
  const [view, setView] = useState<RecoveryView | null>(null);
  const [busy, setBusy] = useState(false);
  const [message, setMessage] = useState<string | null>(null);
  const [confirmation, setConfirmation] = useState<{ view: RecoveryView; action: string } | null>(null);
  const gate = useRef(false);
  const epoch = useRef(0);
  useEffect(() => {
    epoch.current++; setView(null); setConfirmation(null); setMessage(null);
    return () => { epoch.current++; };
  }, [instanceId, operationId]);
  const locked = busy || client.hasInstanceChange(instanceId) || ["Accepted", "Executing"].includes(status);
  async function inspect() {
    if (gate.current) return;
    gate.current = true; setBusy(true); setConfirmation(null);
    const reading = epoch.current;
    try {
      const next = await client.resolveOperation(instanceId, operationId);
      if (reading !== epoch.current) return;
      if (next.operationId !== operationId) { client.navigate({ page: "operation", operationId: next.operationId, instanceId }); return; }
      setView(next); setMessage(null); onReconciled();
    } catch { if (reading === epoch.current) { setView(null); setMessage("実体を確認できませんでした。変更を実行せず保留しています。処理状況と実体を再確認してください。"); } }
    finally { gate.current = false; if (reading === epoch.current) setBusy(false); }
  }
  async function apply(snapshot: RecoveryView, action: string) {
    if (gate.current || locked || snapshot !== view) return;
    gate.current = true; setBusy(true); setConfirmation(null);
    const sending = epoch.current;
    try {
      const next = await client.recoverOperation(snapshot, action);
      if (sending !== epoch.current) return;
      if (next.operationId !== operationId) { client.navigate({ page: "operation", operationId: next.operationId, instanceId }); return; }
      setView(next); setMessage(action === "propose_ports" ? "候補はまだ変更されていません。差分を確認してから適用してください。" : "復旧結果を処理の記録から再照会します。");
      onReconciled();
    } catch { if (sending === epoch.current) { setView(null); setMessage("復旧結果を確認できません。別の変更を保留し、実体を再確認してください。"); } }
    finally { gate.current = false; if (sending === epoch.current) setBusy(false); }
  }
  return <section className="panel recovery-panel" aria-label="復旧・実体の確認" aria-busy={busy}>
    <div className="panel-title"><h2>復旧・実体の確認</h2><button className="btn" disabled={busy} onClick={() => void inspect()}>実体を再確認</button></div>
    <p>保存された処理結果と、現在の実体を分けて確認します。再確認だけでは設定変更・再試行を実行しません。</p>
    {["Accepted", "Executing"].includes(status) && <p className="notice">処理が実行中です。別の変更は実行できません。</p>}
    {message && <p className="notice warning" role="status">{message}</p>}
    {view && <><dl className="operation-summary"><div><dt>直前の失敗・処理結果</dt><dd>{outcomes[view.previousStatus] ?? "未確認"}</dd></div>
      <div><dt>現在の実体（再確認結果）</dt><dd>{runtime[view.currentRuntime] ?? "未確認"}</dd></div><div><dt>処理ID / 試行</dt><dd>{view.operationId} / {view.attempt}</dd></div></dl>
      {view.holdReasons.length > 0 && <div className="notice warning"><strong>保留・確認が必要な理由</strong><ul>{view.holdReasons.map((reason) => <li key={reason}>{reasons[reason] ?? "安全条件を確認できません。変更は保留します。"}</li>)}</ul></div>}
      <div className="actions">{view.actions.filter((action) => labels[action]).map((action) => <button className="btn" key={action} disabled={locked}
        onClick={() => action === "propose_ports" ? void apply(view, action) : setConfirmation({ view, action })}>{labels[action]}</button>)}
        {view.proposedPorts.length > 0 && <button className="btn primary" disabled={locked || !view.actions.includes("propose_ports")}
          onClick={() => setConfirmation({ view, action: "confirm_ports" })}>候補ポートの差分を確認</button>}</div>
    </>}
    {confirmation && <ConfirmDialog title={labels[confirmation.action]} onClose={() => setConfirmation(null)}
      confirmLabel={labels[confirmation.action]} confirmDisabled={locked || confirmation.view !== view} onConfirm={() => void apply(confirmation.view, confirmation.action)}>
      <p>処理ID: {confirmation.view.operationId} · 試行: {confirmation.view.attempt}</p>
      {["restore_ports", "confirm_ports"].includes(confirmation.action) && <><table className="recovery-diff"><thead><tr><th>接続</th><th>現在の候補</th><th>確認するポート</th></tr></thead><tbody>
        {(confirmation.action === "restore_ports" ? confirmation.view.originalPorts : confirmation.view.proposedPorts).map((p) => <tr key={p.slot}><td>{p.slot}</td><td>{confirmation.view.ports.find((current) => current.slot === p.slot)?.hostPort ?? "未確認"}</td><td>{p.hostPort}</td></tr>)}</tbody></table>
        <p>データを保持し、旧新予約は反映を確認するまで保持します。ポート編集の復帰後も停止状態を維持します。</p></>}
      {confirmation.action === "restore_external" && <><table className="recovery-diff"><thead><tr><th>ファイル</th><th>保存時hash</th><th>現在のhash</th></tr></thead><tbody>
        {confirmation.view.files.map((f) => <tr key={f.path}><td>{f.path}</td><td><code>{f.recordedHash ?? "追加ファイル"}</code></td><td><code>{f.observedHash ?? "削除済み"}</code></td></tr>)}</tbody></table>
        <p>確認した外部編集を保護された復旧領域へ退避し、保存設定から再生成します。構成が異なる場合は所有を確認して停止・停止状態で再作成します。退避ファイルは自動実行しません。</p></>}
      {confirmation.action === "abandon" && <p>実行資源の不在・保存領域・前CLIの終了を再確認できる場合にのみ、この処理を終了します。データは保持します。</p>}
      <p>Coreが安全条件と表示した版を再確認します。時間の経過やボタン操作だけでは完了と表示しません。</p>
    </ConfirmDialog>}
  </section>;
}
