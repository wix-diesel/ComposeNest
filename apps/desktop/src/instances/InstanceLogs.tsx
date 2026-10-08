import { useEffect, useRef, useState } from "react";
import type { LogsView } from "../generated/template-form";
import type { ApplicationClient } from "../ipc/ApplicationClient";
import { Icon } from "../shell/Icon";

/** Owns a log session only while its panel and document are visible. */
export function InstanceLogs({ client, instanceId, revision }: { client: ApplicationClient; instanceId: string; revision: number }) {
  const [following, setFollowing] = useState(true);
  const [reload, setReload] = useState(0);
  const [visible, setVisible] = useState(!document.hidden);
  const [view, setView] = useState<LogsView | null>(null);
  const [busy, setBusy] = useState(false);
  const [message, setMessage] = useState<string | null>(null);
  const output = useRef<HTMLPreElement>(null);
  const mounted = useRef(false);
  const cleanup = useRef<Promise<void>>(Promise.resolve());
  const cleanupFailed = useRef(false);
  const pendingReleases = useRef(new Set<string>());

  useEffect(() => {
    mounted.current = true;
    const visibility = () => { setVisible(!document.hidden); if (document.hidden) setBusy(false); };
    document.addEventListener("visibilitychange", visibility);
    return () => { mounted.current = false; document.removeEventListener("visibilitychange", visibility); };
  }, []);
  useEffect(() => {
    if (!following || !visible) return;
    let disposed = false;
    let timer: ReturnType<typeof setTimeout> | undefined;
    let session: ReturnType<ApplicationClient["subscribeLogs"]> | undefined;
    setBusy(true); setMessage(null);
    function release(id: string) {
      pendingReleases.current.add(id);
      const result = client.unsubscribeLogs(id).then(() => { pendingReleases.current.delete(id); }).catch(() => {
        cleanupFailed.current = true;
        if (mounted.current) setMessage("購読解除を確認できませんでした。再読込みしてください。環境は停止しません。未取得の購読は自動解除されます。");
      });
      cleanup.current = result;
      return result;
    }
    async function read(initial?: LogsView) {
      try {
        const snapshot = initial ?? await client.getLogs(session!.id, instanceId, revision);
        if (disposed) return;
        setView(snapshot); setBusy(false);
        if (snapshot.failed) setMessage("ログの取得に失敗しました。現在の状態を確認して再読込みしてください。");
        if (!snapshot.finished) timer = setTimeout(() => void read(), 500);
        else await release(session!.id);
      } catch {
        if (disposed) return;
        setBusy(false); setMessage("ログを取得できませんでした。対象・構成・Dockerの接続を確認して再読込みしてください。");
        if (session) await release(session.id);
      }
    }
    void (async () => {
      await cleanup.current;
      if (disposed) return;
      for (const id of pendingReleases.current) await release(id);
      if (disposed) return;
      cleanupFailed.current = pendingReleases.current.size > 0;
      if (cleanupFailed.current) { setBusy(false); setMessage("購読解除を確認できませんでした。再読込みしてください。環境は停止しません。未取得の購読は自動解除されます。"); return; }
      session = client.subscribeLogs(instanceId, revision);
      try {
        const initial = await session.ready;
        if (disposed) { await release(session.id); return; }
        await read(initial);
      } catch {
        if (!disposed) { setBusy(false); setMessage("ログを取得できませんでした。対象・構成・Dockerの接続を確認して再読込みしてください。"); }
        await release(session.id);
      }
    })();
    return () => { disposed = true; clearTimeout(timer); if (session) void release(session.id); };
  }, [client, instanceId, revision, following, visible, reload]);
  useEffect(() => { if (following && output.current) output.current.scrollTop = output.current.scrollHeight; }, [view, following]);

  const restart = () => { cleanupFailed.current = false; setView(null); setFollowing(true); setReload((n) => n + 1); };
  const status = !visible ? "画面非表示のため購読停止" : !following ? "追従停止中" : busy ? "ログを取得中" : view?.finished ? "購読終了" : message ? "取得できません" : "追従中";
  return <section className="panel instance-logs" aria-busy={busy}>
    <div className="panel-title"><h2>サービスログ</h2><div className="actions">
      <button className="btn" aria-pressed={following} onClick={() => { setBusy(false); setFollowing((value) => !value); }}>{following ? "追従を停止" : "追従を再開"}</button>
      <button className="btn" onClick={restart}><Icon name="refresh" />再読込み</button>
    </div></div>
    <p role="status" className="detail-note">{status} · 購読停止では環境を停止しません。</p>
    {message && <p role="alert" className="notice">{message}</p>}
    <pre ref={output} className="detail-log" aria-label="マスク済みサービスログ" tabIndex={0}>{view?.lines.join("\n") || (busy ? "ログを取得中…" : "表示できるログはありません。")}</pre>
    <p className="detail-note">表示上限 2,000行 / 2 MiB · 1行16 KiB · 既知の秘密情報はマスキングして表示</p>
    <p className="detail-note">表示上限による欠落 {view?.droppedLines ?? 0}行 · 長い行の切詰め {view?.truncatedLines ?? 0}行 · 最新2,000行から取得します。表示ログは保存しません。</p>
  </section>;
}
