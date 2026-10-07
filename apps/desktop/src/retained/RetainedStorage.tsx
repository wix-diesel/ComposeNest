import { useEffect, useState, type Ref } from "react";
import type { RetainedInstance, RetainedLocation } from "../generated/template-form";
import type { ApplicationClient } from "../ipc/ApplicationClient";
import { ja } from "../messages";
import { Icon } from "../shell/Icon";
import "./retained.css";

function presence(location: RetainedLocation) {
  const state = location.storage.presence === "present" && (!location.ownershipVerified || location.ownership !== "retained")
    ? "unverified" : location.storage.presence;
  return ({ present: ["保持済み (Present)", "ready"], missing: ["見つかりません (Missing)", "danger"],
    unverified: ["確認できません (Unverified)", "warn"], not_materialized: ["未作成 (NotMaterialized)", ""] } as Record<string, string[]>)[state]
    ?? ["確認できません (Unverified)", "warn"];
}

const observed = (time: string | null) => time ? `${time} UTC` : "未観測";
const placement = (value: string) => ({ staged: "未公開", published: "公開済み", retained: "保持" } as Record<string, string>)[value] ?? "所在未確認";

/** Shows saved retained references and explicitly requested physical rechecks. */
export function RetainedStorage({ client, headingRef, onAbout }: {
  client: ApplicationClient; headingRef: Ref<HTMLHeadingElement>; onAbout: () => void;
}) {
  const [items, setItems] = useState<RetainedInstance[]>([]);
  const [loaded, setLoaded] = useState(false);
  const [loading, setLoading] = useState(true);
  const [refresh, setRefresh] = useState(0);
  const [error, setError] = useState<string | null>(null);
  const [failedIds, setFailedIds] = useState<string[]>([]);
  useEffect(() => {
    let active = true;
    setLoading(true); setError(null); setFailedIds([]);
    void (async () => {
      const saved = await client.listRetainedStorage();
      if (!active) return;
      setItems(saved); setLoaded(true);
      if (!refresh) return;
      const results = await Promise.allSettled(saved.map((item) => client.refreshRetainedStorage(item.instance.id)));
      if (!active) return;
      setItems(results.map((result, index) => result.status === "fulfilled" ? result.value : saved[index]));
      const failed = results.flatMap((result, index) => result.status === "rejected" ? [saved[index].instance.id] : []);
      setFailedIds(failed);
      if (failed.length) setError("一部の状態を確認できませんでした。対象の環境には保存された前回の観測を表示しています。Dockerの準備状況などを確認し、再確認してください。");
    })().catch(() => {
      if (active) setError("保持データ一覧を取得できませんでした。再確認してください。表示がある場合は前回の取得結果です。");
    }).finally(() => { if (active) setLoading(false); });
    return () => { active = false; };
  }, [client, refresh]);
  const count = items.reduce((total, item) => total + item.locations.length, 0);
  return <div className="retained-storage" aria-busy={loading}>
    <div className="page-heading"><div><div className="eyebrow">{ja.workspace}</div>
      <h1 id="screen" ref={headingRef} tabIndex={-1}>{ja.pages.retained}</h1><p className="subtitle">{ja.subtitles.retained}</p></div>
      <div className="actions"><button className="btn" disabled={loading} onClick={() => setRefresh((value) => value + 1)}><Icon name="refresh" />状態を確認</button>
        <button className="btn" onClick={onAbout}>{ja.about}</button></div></div>
    <div className="notice"><Icon name="info" /><div><strong>環境を削除しても、データは残ります</strong>
      <p>ここに表示される領域は新しい環境で自動的に再利用されません。v1では復元やデータの完全削除は提供しません。</p></div></div>
    {error && <p className="notice warning" role="alert">{error}</p>}
    {loading && <p role="status">{refresh ? "保持データの状態を確認中…" : "保持データを読み込み中…"}</p>}
    <section className="panel" aria-label="削除した環境の保存領域">
      <div className="panel-title"><h2>削除した環境の保存領域</h2><small>{loaded ? `${count} 件` : "—"}</small></div>
      {loaded && (items.length === 0 ? <p>保持データはありません。</p> : count === 0 ? <p>削除した環境に保存領域の割当てはありません。元の設定の所在を確認できます。</p> :
        <div className="table-wrap" role="region" aria-label="保持データの表（横スクロール可能）" tabIndex={0}><table><thead><tr>{["元の環境", "保存方式", "保存先・識別名", "環境の削除日", "状態・最終観測"].map((label) => <th key={label} scope="col">{label}</th>)}</tr></thead>
          <tbody>{items.flatMap((item) => item.locations.map((location) => {
            const [label, style] = presence(location);
            return <tr key={`${item.instance.id}/${location.storage.slot}`}><td><strong>{item.instance.name}</strong><small>{item.serviceName} {item.instance.selectedVersion}</small><small>環境ID: <code>{item.instance.id}</code></small></td>
              <td><span className="badge">{location.storage.method === "bind" ? "bind mount" : location.storage.method === "volume" ? "named volume" : "方式未確認"}</span><small>slot: {location.storage.slot}</small></td>
              <td><code>{location.location}</code><small>{location.storage.method === "bind" ? "ホストの専用フォルダー" : location.storage.method === "volume" ? "Dockerが管理する保存領域" : "方式未確認"}</small></td>
              <td>{observed(item.deletedAt)}</td><td><span className={`badge ${style}`}>{label}</span><small>最終観測: {observed(location.observedAt)}</small>
                {failedIds.includes(item.instance.id) && <small className="recheck-failed">再確認失敗・前回の観測</small>}</td></tr>;
          }))}</tbody></table></div>)}
      <p className="footnote">表示は保存された観測です。「状態を確認」で存在と所有情報を再確認します。Missingは再作成せず、Unverifiedは保持済みと判断しません。</p>
    </section>
    <div className="two-equal"><section className="panel" aria-label="元の設定・Compose"><div className="panel-title"><h2>元の設定・Compose</h2></div>
      <p>元の設定は保護されたデータベース、生成物は記録された所在に残しています。認証情報の内容は表示しません。</p>
      {loaded && items.map((item) => <article className="retained-config" key={item.instance.id}><h3>{item.instance.name}</h3><dl>
        <dt>環境ID / 設定リビジョン</dt><dd><code>{item.instance.id}</code> / {item.instance.specRevision}</dd>
        <dt>設定データベース</dt><dd><code className="path">{item.settingsDatabase}</code></dd>
        <dt>Snapshot ID</dt><dd><code>{item.snapshotId}</code></dd>
        <dt>Compose生成物</dt><dd>{item.artifacts.length === 0 ? "記録なし" : item.artifacts.map((artifact) => <div key={artifact.id}>
          <small><code>{artifact.id}</code> / 設定リビジョン {artifact.specRevision} / {placement(artifact.placement)}</small>
          <code className="path">{artifact.directory ?? "公開先の記録なし"}</code></div>)}</dd></dl></article>)}
    </section><section className="panel"><div className="panel-title"><h2>Docker管理のデータ</h2></div>
      <p>named volumeはDocker側で管理されています。通常のホストフォルダーとして開くことはできません。</p></section></div>
  </div>;
}
