import { useState } from "react";
import type { InstanceDetailView } from "../generated/template-form";
import { ConfirmDialog } from "../shell/ConfirmDialog";

/** Confirms the displayed target revision and its actual retained storage locations. */
export function DeleteDialog({ detail, disabled, onClose, onConfirm }: {
  detail: InstanceDetailView; disabled: boolean; onClose: () => void; onConfirm: () => void;
}) {
  const [confirmed, setConfirmed] = useState(false);
  const locationsKnown = detail.instance.storage.every((storage) => detail.locations.some((location) => location.slot === storage.slot && location.location.trim()));
  return <ConfirmDialog title="環境を削除。データは残ります" onClose={onClose} onConfirm={onConfirm}
    confirmLabel="データを残して削除" confirmDisabled={disabled || !locationsKnown || !confirmed}>
    <p>対象：<strong>{detail.state.name}</strong></p>
    <p><code>{detail.instance.id}</code> · {detail.instance.templateId} {detail.instance.selectedVersion} · 対象版 r{detail.state.revision}</p>
    <p>コンテナと専用ネットワークを削除し、完了後に環境一覧から外します。</p>
    <p>保持するデータ：</p>
    {!detail.instance.storage.length && <p>保存領域の割当てはありません。</p>}
    {detail.instance.storage.map((storage) => <div key={storage.slot}><p>{storage.slot} · {storage.method === "bind" ? "bind mount" : "named volume"}</p>
      <code className="detail-path">{detail.locations.find((location) => location.slot === storage.slot)?.location || "保存先は未取得"}</code></div>)}
    <p>元の設定とComposeも残します。データの存在は削除処理で確認し、見つからない領域は再作成しません。</p>
    <label><input type="checkbox" checked={confirmed} onChange={(event) => setConfirmed(event.target.checked)} />データ・元の設定・Composeを残して環境を削除することを確認しました</label>
    {disabled && <p role="alert">現在の状態を再確認し、削除内容を確認し直してください。</p>}
    <p>「取消し」は要求を送信しません。確定後に画面を閉じても処理は取り消されません。</p>
  </ConfirmDialog>;
}
