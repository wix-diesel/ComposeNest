# 中断境界・復旧回帰の検証（Issue #48）

作成・Clone・ポート編集・外部編集復帰・Deleteを対象に、既存の仕様別テストへ不足境界だけを追加した。製品の復旧手順・API・依存パッケージは変更していない。

## 障害注入と確認範囲

| 対象 | 新しい境界 | 再起動後の確認 |
| --- | --- | --- |
| 作成・Clone | 保存準備、Image解決、Artifact公開、config検証、停止状態作成、作成後inspect、start、Ready観測の直前・直後、DB確定直前・直後、DB容量不足。各19ケース | 子プロセスをexit(86)で終了し、同じ管理ルートを再開。同じReceipt・Plan・Operation・Project・秘密・保存割当てを維持し、予約を保持。Readyの観測証跡と未完了journalを確認。実APIでの確定・保留は別行列で検証 |
| ポート編集 | Compose再作成の直前・直後でアプリへSIGKILL、DB容量不足。3ケース | PendingChangeの旧／新revisionと両予約を保持。旧実体なら保留、新実体なら照合で確定し、再作成・startを増やさない |
| 外部編集復帰 | 既存の退避後・再作成応答不明にDB容量不足を追加。各状態でDB Workerを閉じて再開 | 元の復帰Operation・退避先・秘密を維持し、実体が反映済みなら追加の再作成をしない |
| Delete | コンテナ削除・network削除・保存領域照会・最終不在照会の直前・直後、DB確定直前・直後、DB容量不足。11ケース | 子プロセスをexit(86)で終了。Retiring中はCommitted予約・秘密・データを保持。容量回復後は同じOperationでRetiredを確定 |
| Ready復旧API | 作成／Clone × Ready観測直後・DB確定直前・DB容量不足の6ケースで子プロセス終了 | `recover_operation`へCoreが許可する`reconcile_ports`を送信。同じOperationを外部操作なしで確定。観測stepが未完了で前CLI終了未確認ならAPIも拒否して保留 |
| Engine断 | 既存の所有・構成不明行列へinfo失敗を追加 | Docker変更・予約解放を行わず保留 |

`support/interruption.rs`は子プロセスと実SQLiteを使う。`PRAGMA max_page_count`を両DB接続に設定し、確定トランザクション内のTriggerから1 MiBを書き込む。`SQLITE_FULL`をエラーコードで確認してから、トランザクション全体のロールバックと予約保持を検証する。テスト専用DBのみを制限し、ホストのディスクは埋めない。LinuxのCLI Fakeを使うSIGKILL試験はUnix限定。作成・Clone・Deleteのexit(86)試験は通常のAdapterテストにも含む。

CLI生存・終了未確認は既存の`inherited_unfinished_cli_and_missing_storage_hold_every_change`と`uncertain_prerequisites_do_not_archive_stop_or_recreate`を再利用する。Fake Adapterの観測と実Dockerの受入結果を区別し、実Engineの停止やDocker Desktopの試験を実施済みとは扱わない。

## 仕様との対応

| 基準 | 回帰テスト（`crates/adapters/tests`、Journalは`src`） | 検証結果 |
| --- | --- | --- |
| AC-14・REC-03〜08 | `create_state::create_interruption::create_and_clone_crashes_preserve_confirmed_identity_and_ready_evidence`、`port_recovery::port_edit_crash_and_full_database_keep_old_and_new_reservations_until_reconciled`、`port_recovery::ready_create_and_clone_crashes_recover_through_the_public_api_without_effects`、`delete_state::delete_crashes_never_release_reservations_before_atomic_retirement`、`external_recovery::interrupted_archive_and_unknown_recreate_resume_the_original_operation` | ローカル成功。強制終了58ケースと復帰状態3ケース |
| CP-T27 | `port_recovery::creation_conflict_proposes_without_changes_and_rechecks_confirmation`、`failed_start_retries_the_same_operation_and_retains_the_failed_step` | ローカル成功。同じOperation・確定値、試行番号だけ進行 |
| CP-T28 | `port_recovery::creation_conflict_proposes_without_changes_and_rechecks_confirmation`、`confirming_original_create_ports_uses_the_new_pending_change`、`port_edit_state` | ローカル成功。確認前は変更せず、旧Committed＋新Heldを保持 |
| CP-T29 | ポート編集の新しい中断行列、既存`interrupted_application_completes_without_another_create_or_start` | ローカル成功。反映直後・確定失敗でも早期解放なし |
| CP-T30 | `port_recovery::inherited_unfinished_cli_and_missing_storage_hold_every_change`、`ambiguous_ownership_artifact_and_storage_never_release_reservations`、`external_recovery::uncertain_prerequisites_do_not_archive_stop_or_recreate` | ローカル成功。Missingは保留し、保存領域を作り直さない |
| CP-T31 | `create_session::cancel_and_confirmation_guards_never_allocate_resources`、`clone_session::clone_confirmation_guards_source_and_candidates_and_restores_only_clone_receipts`、`clone_plan::clone_commit_is_idempotent_and_source_revision_is_guarded`、作成／Cloneの確定直後中断 | ローカル成功。取消しは資源なし、受付後はPlanで元Receiptを照会 |

## 再実行と実機受入

```sh
cargo test -p composenest-adapters --locked --test create_state --test delete_state --test port_recovery --test external_recovery
cargo test -p composenest-domain -p composenest-application -p composenest-adapters --locked
cargo fmt --all -- --check
cargo clippy -p composenest-domain -p composenest-application -p composenest-adapters --all-targets --locked -- -D warnings
```

CIの`recovery-faults`でFake／SQLite行列を実行し、`recovery-fault-results`にログを保存する。別ジョブ`recovery-docker`は既存の実Docker試験を再利用する。`external_recovery::actual_docker_restores_owned_drift_and_keeps_data_and_secrets`と`docker_delete::docker_delete_preserves_bind_and_volume_data_and_rejects_foreign_network_endpoints`がランダムな専用scope・Projectで保存データ、秘密、bind／named volume、管理外network endpointを検証する。後始末はその試験が所有する資源だけを対象とし、共有Engineのpruneは行わない。

2026-10-08の作業環境: Ubuntu 24.04.3 LTS x86_64、Rust 1.98.1。Docker CLI／Engineはないため実Docker受入はローカル未実施。CIのLinux Engine結果はPRで追跡する。Windows／macOS Docker Desktopでの異常終了受入は未実施。

レビュー対応: Ready確定をテストから直接呼ぶ処理と未完了stepを成功へ書き換える処理を削除した。Create／Cloneの復旧はCoreが返す`reconcile_ports`を使い、Start／Stop／Restart用の`complete`（`LifecycleState::complete`）は使わない。

既存コミット`025d40e`の[CI実行388](https://github.com/wix-diesel/ComposeNest/actions/runs/37716098821)は全12ジョブ成功。Ubuntu 24.04上のworkspace test／Clippy、Windows／macOSのAdapter・管理ルート権限、Docker Engine 28.0.4（linux/amd64）／Compose v2.38.2での専用scopeの復帰／Delete、PostgreSQL／Redisのbind・named受入を確認した。これはDocker Desktopでの異常終了検証とは区別する。
