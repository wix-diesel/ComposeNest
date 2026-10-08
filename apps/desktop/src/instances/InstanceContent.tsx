import { useEffect, useRef, useState } from "react";
import type { InstanceComposeView } from "../generated/template-form";
import type { ApplicationClient } from "../ipc/ApplicationClient";

/** Keeps revealed values local to a mounted row; copy never reveals the value on screen. */
export function ConnectionValue({ client, instanceId, revision, label, slot, text }: {
  client: ApplicationClient; instanceId: string; revision: number; label: string; slot?: string; text?: string;
}) {
  const [revealed, setRevealed] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [message, setMessage] = useState<{ error: boolean; text: string } | null>(null);
  const epoch = useRef(0);
  const gate = useRef(false);
  function hide() { epoch.current++; gate.current = false; setBusy(false); setRevealed(null); setMessage(null); }
  useEffect(() => {
    window.addEventListener("blur", hide);
    return () => { epoch.current++; window.removeEventListener("blur", hide); };
  }, []);
  async function act(copy: boolean) {
    if (gate.current) return;
    gate.current = true; setBusy(true); setMessage(null);
    const current = ++epoch.current;
    try {
      const value = slot ? await client.getInstanceSecret(instanceId, revision, slot) : text ?? "";
      if (current !== epoch.current) return;
      if (copy) {
        await client.copyText(value);
        if (current === epoch.current) setMessage({ error: false, text: "コピーしました。" });
      } else setRevealed(value);
    } catch {
      if (current === epoch.current) setMessage({ error: true, text: copy ? "コピーできませんでした。もう一度お試しください。" : "秘密情報を取得できませんでした。もう一度お試しください。" });
    } finally { if (current === epoch.current) { gate.current = false; setBusy(false); } }
  }
  return <>
    <span className="connection-value">{slot ? revealed ?? "••••••••（非表示）" : text}</span>
    {slot && <button className="btn small" aria-label={`${label}を${revealed === null ? "表示" : "隠す"}`} aria-pressed={revealed !== null}
      disabled={busy} onClick={() => revealed === null ? void act(false) : hide()}>{revealed === null ? "表示" : "隠す"}</button>}
    <button className="btn small" aria-label={`${label}をコピー`} disabled={busy} onClick={() => void act(true)}>コピー</button>
    {message && <small className="content-message" role={message.error ? "alert" : "status"}>{message.text}</small>}
  </>;
}

/** Shows the real selected file, discarding raw text on remask, blur, or panel departure. */
export function InstanceCompose({ client, instanceId, revision }: { client: ApplicationClient; instanceId: string; revision: number }) {
  const [view, setView] = useState<InstanceComposeView | null>(null);
  const [busy, setBusy] = useState(true);
  const [failed, setFailed] = useState(false);
  const epoch = useRef(0);
  const alive = useRef(false);
  async function read(reveal: boolean) {
    const current = ++epoch.current;
    setView(null); setBusy(true); setFailed(false);
    try {
      const result = await client.getInstanceCompose(instanceId, revision, reveal);
      if (alive.current && current === epoch.current) setView(result);
    } catch { if (alive.current && current === epoch.current) setFailed(true); }
    finally { if (alive.current && current === epoch.current) setBusy(false); }
  }
  useEffect(() => {
    alive.current = true; void read(false);
    const hide = () => { void read(false); };
    window.addEventListener("blur", hide);
    return () => { alive.current = false; epoch.current++; window.removeEventListener("blur", hide); };
  }, [client, instanceId, revision]);
  return <>
    <div className="notice"><strong>{view && !view.masked ? "秘密情報を含む原文を表示しています" : "秘密情報を隠して表示します"}</strong>
      <p>原文にはパスワードが含まれる場合があります。共有する際は内容を確認してください。秘密はSQLiteと生成物に平文で保存されます。</p></div>
    <section className="panel"><div className="panel-title"><h2>compose.yaml</h2><div className="actions">
      <button className="btn" disabled={busy || !view} aria-pressed={view ? !view.masked : false} onClick={() => void read(view?.masked ?? false)}>{view && !view.masked ? "再マスク" : "原文を表示"}</button>
      <button className="btn" disabled={busy} onClick={() => void read(false)}>再読込み</button>
    </div></div>
      {busy && <p role="status">Composeを取得しています…</p>}
      {failed && <p role="alert">Composeを取得できませんでした。生成物の状態を確認し、再読込みしてください。</p>}
      {view && <><pre className="detail-compose">{view.content}</pre><code className="detail-path">{view.path}</code></>}
    </section>
  </>;
}
