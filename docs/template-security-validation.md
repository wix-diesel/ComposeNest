# Template・IPC・秘密保護の適合性（Issue #49）

2026-10-08。採用済みSchema 1を対象に、構造・意味検証、ファイル受付、登録、Snapshot、実行の証拠を分ける。自動テストの成功はImageの無害性や全OSでのDocker実機受入を保証しない。

## TS-T01〜36の追跡

表の参照先はテストファイル。`単体`はパーサー・意味検証・生成器、`登録`は実ファイル／SQLite／計画の統合、`実行`はDockerまたはWebViewを必要とする受入を表す。各行は主な回帰テスト名と残る実機条件を示す。

| ID | 境界 | 主な検証・証拠 |
| --- | --- | --- |
| TS-T01 | 登録 | [template_package.rs](../crates/adapters/tests/template_package.rs) `registers_only_validated_captured_bytes_without_execution_or_partial_updates`：inputs／port／storageなしの完全定義を登録し共通フォームを取得 |
| TS-T02 | 単体・実行 | [compose.rs](../crates/domain/tests/compose.rs) `each_snapshot_version_uses_its_own_storage_target_and_preserves_values`。17／18の実Docker受入は[PostgreSQL記録](postgresql-validation.md) |
| TS-T03 | 単体・登録・実行 | [compose.rs](../crates/domain/tests/compose.rs) `redis_secret_is_preserved_in_environment_and_exec_argv`、[redis_template.rs](../crates/adapters/tests/redis_template.rs) `redis_connection_uses_the_committed_secret_and_masks_normal_views`。[Redis実機記録](redis-validation.md) |
| TS-T04 | 単体・登録 | [template_semantics.rs](../crates/domain/tests/template_semantics.rs) `select_options_and_false_zero_defaults_keep_their_values`、[template-form.test.mjs](../apps/desktop/scripts/template-form.test.mjs)：5型の共通フォーム・Validation |
| TS-T05 | 単体 | [template_structure.rs](../crates/domain/tests/template_structure.rs) `rejects_numeric_version_and_unknown_manifest_key` |
| TS-T06 | 単体・登録 | [template_structure.rs](../crates/domain/tests/template_structure.rs) `rejects_unsafe_yaml_before_structural_validation`、[template_catalog_view.rs](../crates/adapters/tests/template_catalog_view.rs) `version_and_manifest_failures_show_japanese_corrections_with_source_positions` |
| TS-T07 | 単体 | [template_structure.rs](../crates/domain/tests/template_structure.rs) `rejects_unsafe_yaml_before_structural_validation`／`enforces_file_and_depth_limits` |
| TS-T08 | 単体 | [template_semantics.rs](../crates/domain/tests/template_semantics.rs) `rejects_missing_and_non_default_invalid_versions`：非既定Versionの削除済み入力参照も全体拒否 |
| TS-T09 | 単体 | [template_semantics.rs](../crates/domain/tests/template_semantics.rs) `rejects_optional_command_and_requires_omit_in_environment` |
| TS-T10 | 単体 | [compose.rs](../crates/domain/tests/compose.rs) `optional_environment_omits_unset_input_but_preserves_empty_string` |
| TS-T11 | 単体 | [clone_policy.rs](../crates/domain/tests/clone_policy.rs) `cp_t01_copy_preserves_empty_string_zero_and_false` |
| TS-T12 | 単体 | [template_structure.rs](../crates/domain/tests/template_structure.rs) `rejects_host_privileges_inheritance_and_distributed_secrets`、[template_semantics.rs](../crates/domain/tests/template_semantics.rs) `validates_defaults_options_and_secret_generator` |
| TS-T13 | 単体・登録 | [clone_policy.rs](../crates/domain/tests/clone_policy.rs) `cp_t02_t03_clear_and_absent_copy_never_apply_a_default`、[clone_plan.rs](../crates/adapters/tests/clone_plan.rs) `null_input_is_rejected_and_clear_omits_optional_value` |
| TS-T14 | 単体 | [template_structure.rs](../crates/domain/tests/template_structure.rs) `rejects_host_privileges_inheritance_and_distributed_secrets`：host／volume実名／cloneを拒否 |
| TS-T15 | 単体 | [template_semantics.rs](../crates/domain/tests/template_semantics.rs) `validates_ports_storage_and_connections` |
| TS-T16 | 単体・登録 | [template_semantics.rs](../crates/domain/tests/template_semantics.rs) `resolves_each_complete_version_and_preserves_order_and_presence`、[compose.rs](../crates/domain/tests/compose.rs) `removed_version_fields_are_never_inherited` |
| TS-T17 | 単体・実行 | [compose.rs](../crates/domain/tests/compose.rs) 秘密・固定値のYAML往復。`docker_config_and_container_preserve_actual_values`はDocker専用CIで実行 |
| TS-T18 | 登録 | [state_store.rs](../crates/adapters/src/state_store.rs) `same_meaning_keeps_first_bytes_and_origin_but_changed_meaning_conflicts` |
| TS-T19 | 登録 | 同上。異なる意味hashで同じID／版を上書きしない |
| TS-T20 | 登録 | [clone_plan.rs](../crates/adapters/tests/clone_plan.rs) `clone_version_switch_uses_snapshot_and_shows_removed_fields_and_slots`：カタログ削除後に全VersionとPolicyを維持、原文3文書も残存 |
| TS-T21 | 登録 | [template_package.rs](../crates/adapters/tests/template_package.rs) `registers_only_validated_captured_bytes_without_execution_or_partial_updates`：composenest ID・作者を名乗ってもlocal。[同時重複拒否](../crates/adapters/src/state_store.rs)も確認 |
| TS-T22 | 登録 | 同テスト：検証済みrevisionを用意→元ファイル変更→検証済みバイト列のみ登録。将来のインポートUIは追加しない |
| TS-T23 | 登録 | 同テスト：instances／Snapshot／Image／storage／operationsの全テーブルが空で、データ／Artifactディレクトリも作成しない |
| TS-T24 | 登録・実行 | [clone_plan.rs](../crates/adapters/tests/clone_plan.rs) `clone_storage_switch_keeps_source_allocation_and_uses_new_method`、[clone_session.rs](../crates/adapters/tests/clone_session.rs) `clone_confirmation_guards_source_and_candidates_and_restores_only_clone_receipts`：独立割当て。実データを入れたbind／named Cloneの3OS実機受入は未実施 |
| TS-T25 | 単体・実行境界 | [image_resolution.rs](../crates/application/src/image_resolution.rs) `refuses_unresolved_image_and_uncovered_or_mismatched_volumes`、[docker_create.rs](../crates/adapters/tests/docker_create.rs) `unknown_mount_or_foreign_owner_blocks_start`：偽Adapterで拒否確認。3OSの実Image不一致受入は未実施 |
| TS-T26 | 単体・実行 | [template_structure.rs](../crates/domain/tests/template_structure.rs) 権限拡張拒否、[template-list.test.mjs](../apps/desktop/scripts/template-list.test.mjs) 出所と「実機検証状態：未確認」。任意Imageの無害性を検証済みとは表示しない |
| TS-T27 | 登録 | [template_package.rs](../crates/adapters/tests/template_package.rs) `scans_only_direct_packages_and_isolates_missing_versions`、[template_catalog.rs](../crates/application/tests/template_catalog.rs) `missing_invalid_or_unlisted_non_default_version_rejects_whole_package` |
| TS-T28 | 登録 | [template_catalog_view.rs](../crates/adapters/tests/template_catalog_view.rs) `projects_packages_and_retains_old_revision_without_counting_failed_reload`：欠損／不正を全体拒否、他Templateと既存版を維持 |
| TS-T29 | 登録 | [template_package.rs](../crates/adapters/tests/template_package.rs) `rejects_unsafe_references_and_duplicate_physical_files`：絶対／親／URL／UNC／ADS／予約名／重複参照・hard link。同じ受付テストを3OS CIで実行 |
| TS-T30 | 登録 | 同ファイルのUnix symlink／Windows symlink・junctionテスト、[template_package.rs unit](../crates/adapters/src/template_package.rs) `discards_collection_when_a_file_changes_before_final_verification`／`discards_collection_when_opened_file_or_directory_is_replaced`：同内容での実体差替えも拒否 |
| TS-T31 | 単体・登録 | [template_package.rs](../crates/adapters/tests/template_package.rs) `rejects_oversized_documents_before_parsing`／`rejects_package_when_cumulative_size_exceeds_eight_mebibytes`／`rejects_more_than_thirty_two_versions_and_non_regular_documents` |
| TS-T32 | 単体・登録 | [template_catalog.rs](../crates/application/tests/template_catalog.rs) `comments_whitespace_and_file_names_do_not_change_meaning`／`display_order_and_effective_fields_change_meaning`。登録時の原文SHA-256は捕捉バイト列のテストで照合 |
| TS-T33 | 登録 | [clone_plan.rs](../crates/adapters/tests/clone_plan.rs) `clone_version_switch_uses_snapshot_and_shows_removed_fields_and_slots`：新カタログ版にVersion 3を追加しても旧Snapshotは1／2のみ。3選択も拒否 |
| TS-T34 | 単体 | [template_structure.rs](../crates/domain/tests/template_structure.rs) `rejects_host_privileges_inheritance_and_distributed_secrets`：共通設定／include／extends／旧単一ファイル形式を拒否 |
| TS-T35 | 単体 | [template_semantics.rs](../crates/domain/tests/template_semantics.rs) `rejects_changed_input_type_across_versions` |
| TS-T36 | 登録・実行UI | [create_plan.rs](../crates/adapters/tests/create_plan.rs) `version_switch_preserves_candidates_and_revalidates_only_active_definition`、[clone_plan.rs](../crates/adapters/tests/clone_plan.rs) `clone_version_change_requires_answers_and_commits_only_active_inputs`。新規DefaultとClone入力待ちを区別 |

## IPC・秘密・UIの境界

- [ipc_security.rs](../crates/application/tests/ipc_security.rs)：既知の有効リクエストに未知キー、scope／target／endpoint／Docker設定／host path／command／Composeを注入し拒否を確認。context・edit・CloneAnswerの内側も拒否。CloneAnswerにも`deny_unknown_fields`を追加した。
- [docker_target.rs](../crates/adapters/tests/docker_target.rs) `fixed_target_rejects_change_after_engine_replacement`、[docker_cli.rs](../crates/adapters/tests/docker_cli.rs) `caller_cannot_override_the_fixed_target`：保存したEngine IDと固定接続先を確認。
- [query_service.rs](../crates/adapters/tests/query_service.rs) `saved_list_and_detail_use_committed_ports_and_mask_secrets`／`operation_lookup_is_scoped_historical_masked_and_read_only`、[logs.rs](../crates/application/src/logs.rs) 全チャンク境界の秘密マスキング、[template_diagnostics.rs](../crates/application/src/template_diagnostics.rs) parser原文の非公開を確認。実値取得は明示的な秘密／Composeリクエストに限定。
- [security.test.mjs](../apps/desktop/scripts/security.test.mjs)：mainのローカルcapability、登録済みアプリコマンドと書込みclipboardのみの権限、外部HTML・mockへの依存禁止、localStorageを表示設定に限定。[template-list.test.mjs](../apps/desktop/scripts/template-list.test.mjs)は外部HTMLのプレーンテキスト描画と未検証表示を確認する。

## 実施環境と結果

ローカル：Ubuntu 24.04.3 x86_64コンテナ、Rust 1.98.1、Node.js 24.19.0。Docker Engine／Windows／macOS実機は利用できない。

```text
cargo test -p composenest-domain -p composenest-application -p composenest-adapters --locked
cargo fmt --all -- --check
cargo clippy -p composenest-domain -p composenest-application -p composenest-adapters --all-targets --locked -- -D warnings
pnpm --dir apps/desktop run test:security
pnpm --dir apps/desktop run check:contracts
pnpm --dir apps/desktop run test:navigation
pnpm --dir apps/desktop run build
```

上記は成功。Rustは334件成功、Docker必須の5件はignoredで未実施。Tauriを含むworkspaceのテスト／ClippyはGTK・WebKit開発ライブラリを持つ既存CIに委ねる。

画面受入の`test:templates`／`test:appearance`／`test:content`／`test:logs`はローカル起動を試したが、Chrome 140.0.7339.207が`socket() failed: Operation not permitted`で終了し、検証未実施。テスト失敗を画面成功として数えない。これらの画面テストは既存frontend CIで実行する。

追加した`Template and IPC acceptance` CIはUbuntu 24.04／Windows 2025／macOS 15で同じDomain・Application・ファイル受付・SQLite・Snapshotテストを実行し、OS別ログを`template-security-<OS>` Artifactへ保存する。Windows symlinkテストは作成権限の有無で対象が変わる既存テストであり、無条件にsymlink成功と数えない。junctionは権限不要のfixture作成に失敗するとテストを失敗させる。

このPRのCI結果はPRのChecksとOS別Artifactが正本。Docker Engineの3OS実機受入、TS-T24の実データClone、TS-T25の実Image不一致、Tauri実WebViewでの画面確認は上表の未実施項目として残す。既存のPostgreSQL／Redisの実機記録や採用済み仕様を、このPRで実施した結果へ読み替えない。
