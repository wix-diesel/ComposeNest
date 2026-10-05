import type { InstanceListItem, InstanceSort } from "./listModel";
import { instanceStateLabels, sortInstances } from "./listModel";
import { Icon } from "../shell/Icon";

const columns = { name: "環境名", state: "状態", port: "接続先" };

/** Renders the shared list projection as a sortable, keyboard-scrollable table. */
export function InstanceGrid({ items, sort, changeSort, showDetail }: {
  items: InstanceListItem[]; sort: InstanceSort | null;
  changeSort: (sort: InstanceSort) => void; showDetail: (id: string) => void;
}) {
  function heading(key: InstanceSort["key"]) {
    const direction = sort?.key === key ? sort.direction : "none";
    return <th scope="col" aria-sort={direction}><button type="button" onClick={() => changeSort({
      key, direction: direction === "ascending" ? "descending" : "ascending",
    })}>{columns[key]}<span aria-hidden="true">{direction === "ascending" ? " ↑" : direction === "descending" ? " ↓" : " ↕"}</span></button></th>;
  }
  return <div className="grid-panel" role="region" aria-label="環境一覧の表（縦・横スクロール可能）" tabIndex={0}>
    <table aria-label="環境一覧">
      <thead><tr>{heading("name")}<th scope="col">サービス / バージョン</th>{heading("state")}{heading("port")}
        <th scope="col">データ保存</th><th scope="col">構成</th><th scope="col">操作</th></tr></thead>
      <tbody>{sortInstances(items, sort).map((item) => <tr key={item.source.id}>
        <td className="environment-name">{item.source.name}</td><td>{item.service} / {item.source.selectedVersion}</td>
        <td><span className={`badge ${item.state === "attention" ? "warn" : item.state}`}>{instanceStateLabels[item.state]}</span></td>
        <td className="mono">{item.endpoints}</td><td>{item.storage}</td><td>{item.configuration}</td>
        <td><button type="button" className="btn small" aria-label={`${item.source.name}の詳細`}
          onClick={() => showDetail(item.source.id)}>詳細 <Icon name="arrow" /></button></td>
      </tr>)}</tbody>
    </table>
  </div>;
}
