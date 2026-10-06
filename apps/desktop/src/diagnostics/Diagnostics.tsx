import { useEffect, useState, type RefObject } from "react";
import type { RuntimeDiagnosis } from "../generated/template-form";
import type { ApplicationClient } from "../ipc/ApplicationClient";
import { Icon } from "../shell/Icon";
import "./diagnostics.css";

const labels: Record<string, string> = { cli: "Docker CLI", compose: "Docker Compose", engine: "Docker Engine", linux: "Linuxコンテナ", platform: "アーキテクチャ", endpoint: "ローカル接続", root: "管理データの保存先" };
const statuses: Record<string, string> = { ready: "確認済み", missing: "未導入", unavailable: "確認不能", unsupported: "非対応", changed: "対象不一致", permission_denied: "権限不足" };
const guidance: Record<string, string> = { cli: "Docker CLIを導入し、実行権限と対応版を確認してください。", compose: "対応版のComposeプラグインを導入してください。", engine: "Docker DesktopまたはEngineを起動し、接続先とアクセス権限を確認してください。", linux: "Linuxコンテナモードを使用してください。", platform: "このコンピューターに対応するEngineのアーキテクチャを使用してください。", endpoint: "v1はローカルのUnix socket・Windows named pipeを対象とします。", root: "管理ルートの初期設定と読書き権限を確認してください。権限は自動変更しません。" };

function checkDescription(name: string, status: string, platform: string | null | undefined) {
  if (status === "ready") return ["linux", "platform"].includes(name) ? platform ?? "未確認" : "実際の状態を確認しました";
  if (status === "changed") return "保存済みEngine IDと異なります。元の接続先を確認してください。対象は自動変更しません。";
  if (name === "engine" && status === "unavailable") return "Engine停止・接続不能。起動と接続先を確認してください。";
  return guidance[name];
}

/** Explains the container lifetime on first launch and on the diagnosis screen. */
export function RuntimeNotice() {
  return <div className="runtime-notice"><Icon name="diagnostics" /><div><strong>アプリを閉じても環境は停止しません</strong><p>起動済みコンテナは動き続けます。停止したい場合は、各環境から停止してください。</p></div></div>;
}

/** Renders actual observations without exposing CLI output or changing Docker resources. */
export function Diagnostics({ client, headingRef, summaryChanged }: { client: ApplicationClient; headingRef: RefObject<HTMLHeadingElement | null>; summaryChanged: (summary: string) => void }) {
  const [report, setReport] = useState<RuntimeDiagnosis | null>(null);
  const [busy, setBusy] = useState(true);
  const [failed, setFailed] = useState(false);
  const [retry, setRetry] = useState(0);
  useEffect(() => {
    let active = true;
    setBusy(true); setReport(null); setFailed(false);
    void client.diagnoseRuntime().then((result) => { if (active) setReport(result); })
      .catch(() => { if (active) setFailed(true); }).finally(() => { if (active) setBusy(false); });
    return () => { active = false; };
  }, [client, retry]);
  const ready = !busy && report && ["verified", "unregistered"].includes(report.targetStatus)
    && Object.keys(labels).every((name) => report.checks.find((check) => check.name === name)?.status === "ready");
  useEffect(() => {
    summaryChanged(busy ? "Docker 診断中" : ready ? "Docker 最終診断: 準備完了" : "Docker 最終診断: 要確認");
    return () => summaryChanged("Docker 未確認");
  }, [busy, ready, summaryChanged]);
  const [os, arch] = report?.platform?.split("/") ?? [];
  const value = (text: string | null | undefined) => text || "未確認";
  return <div className="diagnostics" aria-busy={busy}>
    <div className="page-heading"><div><div className="eyebrow">ワークスペース</div><h1 id="screen" ref={headingRef} tabIndex={-1}>Docker診断</h1><p className="subtitle">環境の作成・起動に必要な準備状況を確認します。</p></div>
      <button className="btn primary" disabled={busy} onClick={() => setRetry((current) => current + 1)}><Icon name="refresh" />{busy ? "確認中…" : "再確認"}</button></div>
    <div className={`runtime-notice ${ready ? "" : "warning"}`} role={failed ? "alert" : "status"}><Icon name="diagnostics" /><div>
      <strong>{busy ? "Dockerの準備状況を確認しています" : failed ? "診断結果を取得できませんでした" : ready ? "Dockerを利用できます" : "Dockerの準備を確認してください"}</strong>
      <p>{ready ? "確認したローカル接続先を使用します。Context名だけでは対象の一致を判定しません。" : "接続不能でも保存済みの環境を確認できます。過去の観測は現在の稼働状態を示しません。"}</p>
      {!ready && <button className="btn small" onClick={() => client.navigate({ page: "instances" })}>保存済みの環境を見る</button>}
    </div></div>
    <div className="diagnostic-columns"><section className="panel"><div className="panel-title"><h2>実行環境のチェック</h2><small>最終確認 {report ? new Date(report.observedAt * 1000).toLocaleString("ja-JP") : "未確認"}</small></div>
      {Object.entries(labels).map(([name, label]) => {
        const check = report?.checks.find((check) => check.name === name);
        const status = check?.status ?? "unavailable";
        return <div className="diagnostic-row" key={name}><span className={`check-icon ${status === "ready" ? "ready" : ""}`} aria-hidden="true">{status === "ready" ? "✓" : "!"}</span><div><h3>{label}</h3>
          <p>{check?.version && `${check.version} · `}{busy ? "確認中" : !check ? "未確認" : checkDescription(name, status, report?.platform)}</p></div>
          <span className={`diagnostic-badge ${status === "ready" ? "ready" : ""}`}>{busy || !check ? "未確認" : statuses[status] ?? "確認不能"}</span></div>;
      })}</section>
      <aside><section className="panel"><div className="panel-title"><h2>接続先</h2></div><dl className="diagnostic-summary">
        {[["接続方式", report?.endpoint ? "ローカル（固定接続先）" : "未確認"], ["接続先", report?.endpoint], ["現在のContext（参考）", report?.contextName], ["OS", os], ["アーキテクチャ", arch], ["Engine ID", report?.engineId], ["保存済みEngine ID", report?.registeredEngineId], ["対象の照合", report?.targetStatus === "verified" ? "Engine ID一致" : report?.targetStatus === "changed" ? "対象不一致" : report?.targetStatus === "unregistered" ? "未登録（対象一致は未確認）" : "未確認"], ["管理ルート", report?.managementRoot]].map(([label, text]) => <div key={label}><dt>{label}</dt><dd>{value(text)}</dd></div>)}
      </dl><p>Contextは参考情報です。操作には保存済み接続先とEngine IDを使用します。導入・設定変更後はアプリも再起動してください。</p></section><RuntimeNotice /></aside>
    </div>
  </div>;
}
