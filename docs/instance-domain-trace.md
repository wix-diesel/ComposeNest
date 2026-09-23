# Instance Domain と不変条件の対応

この表は [domain-model.md §15](domain-model.md#15-不変条件) の INV-01〜16 を、今回追加した純粋 Domain の型と、後続の Application／Adapter が確定時に守る規則へ対応付ける。型だけで横断的な一意性や外部実体の証拠を保証したとは扱わない。

| ID | Domain の型・規則 | 後続の確定・外部検証 |
| --- | --- | --- |
| INV-01 | `InstanceId`、`Instance` の非公開 ID／target、ID 由来の Project | 保存領域所有を Instance 確定時に固定 |
| INV-02 | `InstanceId`、`SlotId`、`StorageMethod` | Clone 確定時に新 ID／全 slot の新割当てを検証 |
| INV-03 | `SlotId` と確定 `SpecRevision` | 競合範囲別の有効 Port 予約を台帳で一意化 |
| INV-04 | `OperationStatus::is_unresolved`、`Lifecycle` | Stop／失敗／再起動で予約を解放しない |
| INV-05 | 再試行で維持する `Operation::id`、`StorageMethod` | 確定秘密値と割当てを再利用 |
| INV-06 | `StoragePresence::Missing` と `NotMaterialized` を区別 | 以前存在した領域の自動再作成を拒否 |
| INV-07 | `PendingChange` と確定 `SpecRevision`、`ConfigurationStatus` | 生成物・観測値を確定設定へ自動採用しない |
| INV-08 | 確定 `SpecRevision` | TemplateSnapshot を Instance に専有保存 |
| INV-09 | `OperationStatus::is_unresolved` | Instance ごとの未解決 Operation を一意化 |
| INV-10 | `RuntimeStatus::Preparing` と `Ready`、`Instance::finish_retirement` の不在確認 | Ready／削除完了を実体観測で確認 |
| INV-11 | `Lifecycle::Retiring/Retired`、`StorageOwnership::Retained` | Retired 確定時も保存領域を維持 |
| INV-12 | `SpecRevision` は秘密値を公開 DTO に含めない | 秘密値の保存・表示・通知の制御 |
| INV-13 | `RuntimeStatus::Unknown`、`StoragePresence::Unverified` | 所有証拠と実行対象の一致を Adapter で検証 |
| INV-14 | `DisplayName` と独立した `InstanceId`／Project、安定 `SlotId` | 表示言語や通信 DTO を識別子に使わない |
| INV-15 | 確定 `SpecRevision::storage_method` | 新規作成の既定方式変更を既存 Spec に反映しない |
| INV-16 | `PendingChange::previous/candidate`、`ConfigurationStatus` | 旧予約を維持し、新構成確認後に予約と Spec を一括切替え |

Domain は OS、時刻、乱数、Docker を参照しない。ID と観測時刻は Application／Adapter から受け取る。SQLite、CLI、UI および表の「後続」欄にある横断的な確定処理はこの Issue の対象外である。
