import { useEffect, useRef, useState } from "react";
import type { InstanceEditView } from "../generated/template-form";
import type { ApplicationClient } from "../ipc/ApplicationClient";
import { operationPhaseLabel } from "./operationPhase";
import { ConfirmDialog } from "../shell/ConfirmDialog";
import "../templates/template-form.css";
import "./instance-actions.css";

const runtime: Record<string, string> = { ready: "利用可能", stopped: "停止中", absent: "コンテナ不在", preparing: "準備中", unhealthy: "異常", unknown: "未確認" };
const operation: Record<string, string> = { Accepted: "受付済み・未適用", Executing: "適用中", Succeeded: "適用完了", Failed: "失敗・未解決", OutcomeUnknown: "結果不明", AwaitingDecision: "判断待ち", Abandoned: "解決済み" };
const reservation: Record<string, string> = { committed: "確定予約を保持", held: "候補予約を保持", released: "解放済み", unknown: "予約状態未確認" };
function failure(error: unknown, kind: "rename" | "ports" | null): string {
  const code = typeof error === "object" && error !== null && "code" in error ? String(error.code) : "";
  const messages: Record<string, string> = {
    NAME_OR_REQUEST_CONFLICT: kind === "ports" ? "要求IDまたはポート予約が競合しています。現在の状態を再確認してください。" : "同じ環境名が使用されています。別の名前を入力してください。",
    INSTANCE_STALE: "環境が更新されています。現在の状態を再確認し、変更内容を確認し直してください。",
    INSTANCE_MISSING: "対象の環境が見つかりません。環境一覧へ戻ってください。",
    INSTANCE_INPUT_INVALID: "入力値を確認してください。名前は100文字以内、ポートは1024〜65535の重複しない整数で指定します。",
    INSTANCE_ACTION_UNAVAILABLE: "この操作は現在利用できません。未解決の処理と実行状態を確認してください。",
    INSTANCE_RENAME_UNCONFIRMED: "名前変更の確定を確認できませんでした。入力内容を保持しています。受付を再確認してください。",
    PORT_EDIT_REJECTED: "停止またはコンテナ不在を新しく確認できなかったため、ポート変更は受け付けていません。",
    PORT_EDIT_CONFLICT: "新しいポートを利用できません。ポートとDockerの接続を確認してください。",
    PORT_EDIT_NOT_ACCEPTED: "他の処理またはアプリの終了により、ポート変更は受け付けていません。",
  };
  return messages[code] ?? "接続または受付結果を確認できませんでした。受付を再確認してください。";
}

/** Mock-aligned stopped-port editor; name and port changes have separate confirmations/results. */
export function InstanceEdit({ client, instanceId }: { client: ApplicationClient; instanceId: string }) {
  const [view, setView] = useState<InstanceEditView | null>(null);
  const [name, setName] = useState("");
  const [ports, setPorts] = useState<Record<string, string>>({});
  const [busy, setBusy] = useState(false);
  const [uncertain, setUncertain] = useState(client.hasInstanceChange(instanceId));
  const [stale, setStale] = useState(false);
  const [message, setMessage] = useState<string | null>(null);
  const [nameResult, setNameResult] = useState<string | null>(null);
  const [portResult, setPortResult] = useState<string | null>(null);
  const [confirm, setConfirm] = useState<"rename" | "ports" | null>(null);
  const gate = useRef(false);
  const epoch = useRef(0);
  const mounted = useRef(false);
  const revision = useRef<number | null>(null);
  const specRevision = useRef<number | null>(null);

  function drafts(current: InstanceEditView) {
    revision.current = current.state.revision; specRevision.current = current.specRevision;
    setName(client.getPendingInstanceName(instanceId) ?? current.state.name);
    const pending = client.getPendingInstancePorts(instanceId);
    setPorts(Object.fromEntries(current.ports.map((port) => [port.slot, String(pending?.[port.slot] ?? port.committedPort)])));
  }
  useEffect(() => {
    mounted.current = true;
    let active = true;
    let timer: ReturnType<typeof setTimeout>;
    async function read() {
      const readingEpoch = epoch.current;
      try {
        const current = await client.getInstanceEdit(instanceId);
        if (!active) return;
        if (readingEpoch === epoch.current && !gate.current) {
          setView(current);
          if (revision.current === null) drafts(current);
          else if (revision.current !== current.state.revision || specRevision.current !== current.specRevision) setStale(true);
        }
      } catch (error) { if (active && readingEpoch === epoch.current && !gate.current) { setMessage(failure(error, null)); setView(null); } }
      if (active) timer = setTimeout(read, 1000);
    }
    void read();
    return () => { active = false; mounted.current = false; clearTimeout(timer); };
  }, [client, instanceId]);

  const locked = busy || uncertain || stale || !view || client.hasInstanceChange(instanceId);
  const canRename = !locked && !!view?.state.actions.includes("rename");
  const canPorts = !locked && !!view?.canEditPorts && ["stopped", "absent"].includes(view.state.runtimeStatus);
  const validPorts = !!view && view.ports.every((port) => /^\d+$/.test(ports[port.slot] ?? "") && Number(ports[port.slot]) >= 1024 && Number(ports[port.slot]) <= 65535)
    && new Set(Object.values(ports).map(Number)).size === view.ports.length;
  const portsChanged = view?.ports.some((port) => Number(ports[port.slot]) !== port.committedPort);
  async function submit(kind: "rename" | "ports") {
    if (gate.current || !view || (kind === "rename" ? !canRename : !canPorts || !validPorts)) return;
    gate.current = true; epoch.current++; setBusy(true); setMessage(null);
    try {
      // Re-read immediately before sending. Core also performs a fresh Docker observation.
      const fresh = await client.getInstanceEdit(instanceId);
      if (mounted.current) setView(fresh);
      if (fresh.state.revision !== revision.current || fresh.specRevision !== specRevision.current) throw { code: "INSTANCE_STALE" };
      if (kind === "ports" && (!fresh.canEditPorts || !["stopped", "absent"].includes(fresh.state.runtimeStatus))) throw { code: "PORT_EDIT_REJECTED" };
      const state = kind === "rename"
        ? await client.renameInstance(instanceId, fresh.state.revision, name)
        : await client.editInstancePorts(instanceId, fresh.state.revision, fresh.specRevision, Object.fromEntries(Object.entries(ports).map(([slot, port]) => [slot, Number(port)])));
      if (!mounted.current) return;
      revision.current = state.revision;
      if (kind === "rename") { setName(state.name); setNameResult("環境名を変更しました。"); }
      else setPortResult(state.operationId);
      const current = await client.getInstanceEdit(instanceId);
      if (!mounted.current) return;
      setView(current); specRevision.current = current.specRevision; setStale(false);
      if (kind === "ports" && current.state.operationStatus === "Succeeded") setPorts(Object.fromEntries(current.ports.map((port) => [port.slot, String(port.committedPort)])));
    } catch (error) {
      if (mounted.current) { setMessage(failure(error, kind)); setUncertain(client.hasInstanceChange(instanceId)); setStale(typeof error === "object" && error !== null && "code" in error && error.code === "INSTANCE_STALE"); }
    } finally { gate.current = false; if (mounted.current) setBusy(false); }
  }
  async function refresh() {
    if (gate.current) return;
    gate.current = true; epoch.current++; setBusy(true);
    try {
      const pendingName = client.getPendingInstanceName(instanceId);
      const pendingPorts = client.getPendingInstancePorts(instanceId);
      if (client.hasInstanceChange(instanceId)) await client.retryInstanceChange(instanceId);
      const current = await client.getInstanceEdit(instanceId);
      if (!mounted.current) return;
      setView(current); revision.current = current.state.revision; specRevision.current = current.specRevision;
      // Rebase local drafts for a new confirmation; only replace a confirmed request's field.
      if (pendingName !== null) setName(current.state.name);
      if (pendingPorts !== null && current.state.operationStatus === "Succeeded") setPorts(Object.fromEntries(current.ports.map((port) => [port.slot, String(port.committedPort)])));
      setStale(false); setUncertain(false); setMessage(null);
    } catch (error) { if (mounted.current) { setMessage(failure(error, null)); setUncertain(client.hasInstanceChange(instanceId)); } }
    finally { gate.current = false; if (mounted.current) setBusy(false); }
  }
  return <div className="instance-actions template-form instance-edit">
    <h2>{view?.state.name ?? "環境を確認中"}</h2>
    {busy && <p role="status">変更の開始準備中です。</p>}
    {message && <p role="alert" className="notice">{message}</p>}
    {uncertain && <p className="notice">受付結果の確認が必要です。別の変更は実行できません。</p>}
    {stale && <p className="notice">表示していた版が古くなりました。現在の状態を再確認してください。</p>}
    <div className="notice"><strong>{view?.canEditPorts ? "停止中またはコンテナ不在の環境を編集しています" : "ポート変更には停止またはコンテナ不在の確認が必要です"}</strong><p>ポート変更の適用時に状態を再確認し、コンテナを再作成します。適用後も停止状態を維持し、既存のデータを保持します。</p></div>
    <div className="two-column"><div>
      <section className="panel"><h2>変更できる設定</h2>
        <form onSubmit={(event) => { event.preventDefault(); if (canRename) setConfirm("rename"); }}>
          <div className="field"><label htmlFor="edit-name">環境名</label><input id="edit-name" required maxLength={100} value={name} disabled={!canRename} onChange={(event) => setName(event.target.value)} /></div>
          <small>名前変更はポート変更と別の要求です。実行中でも名前のみ変更できます。</small>
          <div className="actions"><button className="btn" disabled={!canRename || !name.trim() || name === view?.state.name}>変更内容を確認</button></div>
          {nameResult && <p role="status">{nameResult}</p>}
        </form>
        <form onSubmit={(event) => { event.preventDefault(); if (canPorts && validPorts) setConfirm("ports"); }}>
          {view?.ports.map((port) => <div className="field" key={port.slot}><label htmlFor={`edit-port-${port.slot}`}>接続ポート · {port.slot}</label><input id={`edit-port-${port.slot}`} type="number" required min={1024} max={65535} step={1} value={ports[port.slot] ?? ""} disabled={!canPorts} onChange={(event) => setPorts({ ...ports, [port.slot]: event.target.value })} /><small>{port.hostIp} → コンテナ {port.containerPort} / TCP</small></div>)}
          <small>停止またはコンテナ不在を新しく確認できた場合にのみ変更できます。ポートは重複しない1024〜65535の整数です。</small>
          <div className="actions"><button className="btn primary" disabled={!canPorts || !validPorts || !portsChanged}>ポート変更内容を確認</button></div>
          {portResult && <p role="status">ポート変更: {view?.state.operationId === portResult ? operation[view.state.operationStatus ?? ""] ?? "結果未確認" : "結果を再確認してください"}。</p>}
        </form>
      </section>
      <section className="panel"><h2>作成時の設定</h2><dl className="summary-list"><div><dt>サービス / Version</dt><dd>{view ? `${view.templateId} ${view.selectedVersion}` : "未確認"}</dd></div>
        {view?.inputs.map((input) => <div key={input.slot}><dt>{input.slot}</dt><dd>{input.secret ? "••••••••（非表示）" : input.value === null ? "未設定" : typeof input.value === "string" ? input.value : JSON.stringify(input.value)}</dd></div>)}
        <div><dt>保存方式</dt><dd>{view?.storageMethod === "bind" ? "bind mount" : view?.storageMethod === "volume" ? "named volume" : "未確認"}</dd></div></dl><small>バージョン・初期化情報・保存方式は変更できません。</small></section>
    </div><aside><section className="panel"><h2>現在の状態</h2><dl className="summary-list">
      <div><dt>実行状態</dt><dd><span className="badge" data-runtime={view?.state.runtimeStatus}>{runtime[view?.state.runtimeStatus ?? "unknown"] ?? "未確認"}</span></dd></div>
      <div><dt>最終観測（保存値）</dt><dd>{view?.state.observedAt ?? "未確認"}</dd></div>
      <div><dt>構成の適用版</dt><dd>{view ? `保存 r${view.specRevision} / 適用 ${view.appliedSpecRevision === null ? "未確認" : `r${view.appliedSpecRevision}`}` : "未確認"}</dd></div>
      <div><dt>外部構成の照合</dt><dd>適用時に再確認</dd></div>
      <div><dt>直前の処理</dt><dd>{view?.state.operationId ? operation[view.state.operationStatus ?? ""] ?? "未確認" : "なし"}</dd></div>
    </dl>{view?.state.operationId && <p className="operation-link">処理ID: <code>{view.state.operationId}</code> · {operationPhaseLabel(view.state.operationPhase)}</p>}
      <button className="btn small" disabled={busy} onClick={() => void refresh()}>{uncertain ? "受付を再確認" : "現在の状態を再確認"}</button>
    </section><section className="panel"><h2>ポート割当て</h2>{view?.ports.map((port) => <dl className="summary-list" key={port.slot}><div><dt>{port.slot} · 確定ポート</dt><dd>{port.committedPort}</dd></div><div><dt>元のポート / 予約</dt><dd>{port.oldPort} · {reservation[port.oldReservation] ?? "未確認"}</dd></div>{port.candidatePort !== null && <div><dt>変更候補 / 予約</dt><dd>{port.candidatePort} · {reservation[port.candidateReservation ?? "unknown"] ?? "未確認"}</dd></div>}</dl>)}</section>
      <div className="notice"><strong>ポート割当ては停止中も保持</strong><p>変更の適用を確認するまでは元の予約も保持します。途中失敗・結果不明の間は旧新予約を保持し、別の変更は実行できません。</p></div>
    </aside></div>
    {confirm && <ConfirmDialog title={confirm === "rename" ? "環境名の変更を確認" : "ポート変更を確認"} onClose={() => setConfirm(null)} onConfirm={() => void submit(confirm)} confirmLabel={confirm === "rename" ? "名前変更を適用" : "ポート変更を適用"} confirmDisabled={confirm === "rename" ? !canRename : !canPorts || !validPorts}>
      {confirm === "rename" ? <><p>{view?.state.name} → {name}</p><p>接続ポートと実行状態は変更しません。</p></> : <><dl className="summary-list">{view?.ports.map((port) => <div key={port.slot}><dt>{port.slot}</dt><dd>{port.committedPort} → {ports[port.slot]}</dd></div>)}</dl><p>適用後も停止状態を維持し、データを保持します。完了するまでは元の予約も保持します。</p><p>名前変更はこの要求に含めません。</p></>}
    </ConfirmDialog>}
  </div>;
}
