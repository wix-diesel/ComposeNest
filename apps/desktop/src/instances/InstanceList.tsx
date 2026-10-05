import { useEffect, useState } from "react";
import type { ApplicationClient } from "../ipc/ApplicationClient";
import { ja } from "../messages";
import { ListViewChoices } from "../shell/DisplayChoices";
import { Icon } from "../shell/Icon";
import { listItem, selectInstances, type InstanceFilter, type InstanceListItem } from "./listModel";
import "./instance-list.css";

const filters: InstanceFilter[] = ["all", "ready", "stopped", "attention"];
const labels = { all: "すべて", ready: "利用可能", stopped: "停止中", attention: "要確認" };
const stats = { ...labels, all: "すべての環境", attention: "確認が必要" };

/** Renders one saved-state snapshot and distinguishes loading, failures and empty results. */
export function InstanceList({ client, refresh, loadingChanged, listView, changeView }: {
  client: ApplicationClient; refresh: number; loadingChanged: (loading: boolean) => void;
  listView: "cards" | "grid"; changeView: (view: "cards" | "grid") => void;
}) {
  const [items, setItems] = useState<InstanceListItem[]>([]);
  const [status, setStatus] = useState<"loading" | "ready" | "failed">("loading");
  const [search, setSearch] = useState("");
  const [filter, setFilter] = useState<InstanceFilter>("all");
  useEffect(() => {
    let active = true;
    setStatus("loading"); loadingChanged(true);
    void client.listInstances().then((result) => {
      if (active) { setItems(result.map(listItem)); setStatus("ready"); }
    }).catch(() => { if (active) setStatus("failed"); })
      .finally(() => { if (active) loadingChanged(false); });
    return () => { active = false; };
  }, [client, refresh, loadingChanged]);
  const shown = selectInstances(items, search, filter);
  const counts = { all: items.length, ready: 0, stopped: 0, attention: 0 };
  for (const item of items) counts[item.state]++;
  return <section className="instance-list" aria-label={ja.pages.instances} data-view={listView} aria-busy={status === "loading"}>
    <div className="stats">{filters.map((key) => <div className="stat" key={key}><div>
      <div className="stat-label">{stats[key]}</div><div className="stat-value">{status === "ready" ? counts[key] : "—"}<small>環境</small></div>
    </div><span className="stat-icon"><Icon name={key === "all" ? "instances" : key === "ready" ? "play" : key === "stopped" ? "stop" : "info"} /></span></div>)}</div>
    <div className="toolbar"><div className="tabs" role="group" aria-label="環境の絞り込み">{filters.map((key) => <button key={key}
      className={filter === key ? "active" : ""} aria-pressed={filter === key} onClick={() => setFilter(key)}>{labels[key]}<span className="tab-count">{status === "ready" ? counts[key] : "—"}</span></button>)}</div>
      <div className="search"><Icon name="search" /><input type="search" aria-label="環境を検索" placeholder="環境名・サービスで検索" value={search} onChange={(event) => setSearch(event.target.value)} /></div></div>
    <div className="view-toolbar"><span role="status">{status === "ready" ? `${shown.length} / ${items.length} 環境` : status === "loading" ? "環境を読み込み中…" : "環境一覧の取得に失敗しました。更新して再確認してください。"}</span>
      <ListViewChoices value={listView} onChange={changeView} /></div>
    {status === "ready" && <>
      {listView === "grid" && <p className="list-note">DataGridは準備中です。現在はカードで表示します。</p>}
      {items.length === 0 ? <div className="empty">環境はまだありません。「環境を作成」から追加してください。</div>
        : shown.length === 0 ? <div className="empty">条件に一致する環境はありません。</div>
          : <div className="cards">{shown.map((item) => <article className="instance-card" key={item.source.id}>
            <div className="card-body"><div className="card-top"><span className={`service-icon ${item.source.templateId === "redis" ? "redis" : "pg"}`}><Icon name="templates" /></span>
              <span className={`badge ${item.state === "attention" ? "warn" : item.state}`}>{labels[item.state]}</span></div>
              <h3>{item.source.name}</h3><div className="service-description">{item.service} / {item.source.selectedVersion}</div>
              <dl className="card-meta"><dt>接続先</dt><dd className="mono">{item.endpoints}</dd><dt>データ保存</dt><dd>{item.storage}</dd><dt>構成</dt><dd>{item.configuration}</dd></dl></div>
            <div className="card-footer"><small>最終観測 {item.observed}</small><button className="btn small" aria-label={`${item.source.name}の詳細`}
              onClick={() => client.navigate({ page: "instance-detail", instanceId: item.source.id })}>詳細 <Icon name="arrow" /></button></div>
          </article>)}</div>}
    </>}
    <p className="footnote">更新は保存状態を再取得します。Dockerの再観測は行いません。構成の適用済み表示は、外部変更がないことを保証しません。停止した環境のデータとポート割当ては保持されます。</p>
  </section>;
}
