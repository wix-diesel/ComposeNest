import { useRef, useState, type KeyboardEvent } from "react";
import type { InstanceDetailView, JsonValue } from "../generated/template-form";
import "./instance-detail.css";

const tabs = ["概要・接続情報", "ログ", "Compose"];
const storageLabels: Record<string, string> = { bind: "bind mount", volume: "named volume" };
const presenceLabels: Record<string, string> = { present: "存在を確認済み", missing: "見つかりません", not_materialized: "未作成", unverified: "未確認" };
const valueText = (value: JsonValue | null) => value === null ? "未設定" : typeof value === "string" ? value : JSON.stringify(value);

/** Keyboard-accessible detail panels; unconnected features remain explicitly unavailable. */
export function InstanceDetailTabs({ detail }: { detail: InstanceDetailView | null }) {
  const [tab, setTab] = useState(0);
  const buttons = useRef<Array<HTMLButtonElement | null>>([]);
  function keyboard(event: KeyboardEvent<HTMLButtonElement>, index: number) {
    const next = event.key === "ArrowRight" ? (index + 1) % tabs.length
      : event.key === "ArrowLeft" ? (index + tabs.length - 1) % tabs.length
        : event.key === "Home" ? 0 : event.key === "End" ? tabs.length - 1 : null;
    if (next === null) return;
    event.preventDefault(); setTab(next); buttons.current[next]?.focus();
  }
  return <div className="instance-detail">
    <div className="detail-tabs" role="tablist" aria-label="環境の詳細">
      {tabs.map((label, index) => <button key={label} className={tab === index ? "active" : ""} role="tab"
        id={`detail-tab-${index}`} aria-controls={`detail-panel-${index}`} aria-selected={tab === index} tabIndex={tab === index ? 0 : -1}
        ref={(element) => { buttons.current[index] = element; }} onClick={() => setTab(index)} onKeyDown={(event) => keyboard(event, index)}>{label}</button>)}
    </div>
    {tabs.map((label, index) => <div key={label} role="tabpanel" id={`detail-panel-${index}`} aria-labelledby={`detail-tab-${index}`} hidden={tab !== index} tabIndex={0}>
      {index === 0 ? detail ? <Overview detail={detail} /> : <section className="panel"><p>詳細情報を取得できていません。現在の状態を再確認してください。</p></section>
        : index === 1 ? <section className="panel"><div className="panel-title"><h2>サービスログ</h2><div className="actions"><button className="btn" disabled>追従を開始</button><button className="btn" disabled>再読込み</button></div></div><p>ログ購読は未接続です。ログは取得していません。</p></section>
          : <section className="panel"><div className="panel-title"><h2>Compose</h2><button className="btn" disabled>原文を表示</button></div><p>Compose閲覧は未接続です。ファイルの内容は取得していません。</p></section>}
    </div>)}
  </div>;
}

function Overview({ detail }: { detail: InstanceDetailView }) {
  const view = detail.instance;
  const localOnly = view.connections.length > 0 && view.connections.every(({ port }) => port.hostIp === "127.0.0.1" || port.hostIp === "::1");
  return <>
    <div className="detail-columns"><div>
      <section className="panel"><h2>接続情報</h2>
        {!view.connections.length && <p>接続情報は登録されていません。</p>}
        {view.connections.map((connection) => <div className="detail-connection" key={connection.slot}><h3>{connection.label}</h3><dl>
          <div><dt>ホスト</dt><dd><code>{connection.port.hostIp}</code></dd></div>
          <div><dt>ポート</dt><dd><code>{connection.port.hostPort}</code></dd></div>
          {connection.inputSlots.map((slot) => {
            const input = view.inputs.find((item) => item.slot === slot);
            return <div key={slot}><dt>{slot}</dt><dd><span>{!input ? "未取得" : input.secret ? "••••••••（非表示）" : valueText(input.value)}</span>
              {input?.secret && <button className="btn small" disabled>表示</button>}</dd></div>;
          })}
        </dl></div>)}
        <p className="detail-note">確定済みの接続設定です。接続の可否は実行状態を確認してください。秘密情報の表示・コピーは未接続です。</p>
      </section>
      <section className="panel"><h2>データの保存先</h2>
        {!view.storage.length && <p>保存領域は登録されていません。</p>}
        {view.storage.map((storage) => <div className="detail-storage" key={storage.slot}><h3>{storage.slot} · {storageLabels[storage.method] ?? storage.method}</h3>
          <code className="detail-path">{detail.locations.find((location) => location.slot === storage.slot)?.location ?? "保存先は未取得"}</code>
          <p>{presenceLabels[storage.presence] ?? "未確認"}（保存値）</p>
        </div>)}
        <p className="detail-note">環境を停止・削除してもデータは保持されます。保存先の存在を今回確認したものではありません。</p>
      </section>
    </div><aside><section className="panel"><h2>環境情報</h2><dl>
      <div><dt>サービス</dt><dd>{view.templateId} {view.selectedVersion}</dd></div>
      <div><dt>作成受付日時</dt><dd>{detail.creationStartedAt ? `${detail.creationStartedAt} UTC` : "未記録"}</dd></div>
      <div><dt>テンプレート版</dt><dd>{view.templateVersion}</dd></div>
      <div><dt>環境ID</dt><dd><code>{view.id}</code></dd></div>
      <div><dt>複製元</dt><dd>{detail.cloneSourceId ?? "なし"}</dd></div>
    </dl></section>
      <div className="notice"><strong>{localOnly ? "接続できるのは、この端末のみ" : "接続先アドレスを確認してください"}</strong><p>ホストとポートをお使いのアプリに設定してください。</p></div>
    </aside></div>
    <div className="detail-danger"><div><h3>環境を削除</h3><p>削除操作は未接続です。</p></div><button className="btn" disabled>環境を削除</button></div>
  </>;
}
