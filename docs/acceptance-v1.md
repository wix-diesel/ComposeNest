# v1 受入マトリクス（Issue #50）

基準: [要件 §12](requirements-v1.md#12-受入シナリオと追跡)、[Clone Policy §17](clone-policy-spec.md#17-受入境界値シナリオ)、[アーキテクチャ §10・13](architecture-v1.md)。基準コードは main `c39d007` とこのPRの変更。機能追加・Templateの配布昇格・性能保証は行わない。

## 判定と証跡

- **自動成功**: 指定したテスト・環境・commitで成功。fake IPC／CLIを使う範囲を明記し、実機合格へ拡張しない。
- **失敗**: 実行して期待結果を満たさなかった。再実行成功だけで以前の失敗・理由を消さない。
- **未実施**: 実行環境なし、参加者なし、または証跡なし。テストの存在、ignore、skip、過去の別commitのCI成功を合格に数えない。
- **部分確認**: 下位層または一部の組合せだけ成功。残りのOS・Template・方式・実操作を併記する。

以下の表は実施先の追跡表であり、テスト実装の存在だけを合格としない。このPRの自動実行結果はPRのCIと末尾の記録に残す。3OS実機と初心者観察は未実施のため、**v1全体の受入は未完了**。Issue #50は実機・観察の証跡を追記するまで開いたままにする。

各実機記録には日時・commit・実施者・OS/build/CPU・通常利用者/昇格・アプリ配布形式・Docker Desktop/Engine/CLI/Compose版・Image digest・Template/Version・保存方式・手順・期待/実測・判定・未達理由・証跡を付ける。秘密、DB内容、秘密入りComposeを公開証跡に含めない。候補Templateの所在と既存実測は[PostgreSQL](template-candidates/postgresql/VALIDATION.md)・[Redis](template-candidates/redis/VALIDATION.md)を参照する。

## AC-01〜20

Rustの略号 `A` は `crates/adapters/tests/`、`D` は `crates/domain/tests/`、`P` は `crates/application/src/`。UIは `apps/desktop` で `pnpm run <入口>`。結果欄の「実機未実施」は下位テストが成功しても残る。

| ID | 実施先・自動確認の入口 | 実機で追加する確認・現在の結果 |
| --- | --- | --- |
| AC-01 | A/docker_target.rs、test:diagnostics（未導入/不足/停止/不一致） | 各OSで実際にCLI不足・Engine停止から日本語で再確認。実機未実施 |
| AC-02 | A/postgresql_template.rs、test:create、test:forms | 初期bindで作成→Operation→詳細の接続情報からホストTCP接続。実機未実施 |
| AC-03 | A/create_plan.rs `default_storage_setting_affects_future_plans_only`、test:settings、A/redis_template.rs | named設定→Redis新規作成、既存PostgreSQL不変。実機未実施 |
| AC-04 | PostgreSQL/Redis Docker受入（下記コマンド） | 両方式の認証・stop/start・強制再作成後の元データ。対象3OS未実施 |
| AC-05 | A/clone_plan.rs `candidate_templates_confirm_independent_clones_in_all_storage_combinations`、両Docker受入の同方式Clone | 元markerなし→先へ別marker書込み→元marker不変。対象3OS未実施 |
| AC-06 | 上記Core確認・両Docker受入の方式間Clone、test:clone | bind→named/named→bindの全書込みslot・元データ不変。対象3OS未実施 |
| AC-07 | P/host_ports.rs `stopped_source_reservation_and_external_conflict_advance_without_wrap`、test:clone | 停止元5432の予約を保持し先と同時起動。実機未実施 |
| AC-08 | P/host_ports.rs、A/clone_session.rs、A/port_recovery.rs | 外部占有と同時要求、新候補を再確認後に適用。各OSのポート検査は未実施 |
| AC-09 | A/create_state.rs、A/clone_session.rs、A/port_recovery.rs、test:clone、test:recovery | 両Template×両方式の取得/Ready失敗後、元・ID・秘密・保存先を保持。実機未実施 |
| AC-10 | Aのstorage/storage_path/volumes moduleテスト、A/clone_plan.rs | 別表記・symlink/junction・親子パス・既存volumeの拒否。3OS実機未実施 |
| AC-11 | storage module `missing_previous_storage_is_not_recreated`、A/port_recovery.rs、test:retained | 両Template×両方式で保存先消失→無断空DB化なし。実機未実施 |
| AC-12 | A/query_service.rs、A/port_edit_state.rs、A/port_recovery.rs、test:instances | Stop/Restart/名前/Port変更後の実接続・停止・データ/内部ID保持。実機未実施 |
| AC-13 | A/delete_state.rs、A/docker_delete.rs、test:delete、test:retained | 両Template×両方式のデータ保持削除・予約解放・所在表示。実機未実施 |
| AC-14 | A/create_state.rs、A/delete_state.rs、A/port_recovery.rs、A/external_recovery.rs | #48の子process終了/SQLite満杯は下位層。nativeアプリ異常終了の実機検証は未実施 |
| AC-15 | A/docker_target.rs、A/artifact_store.rs、A/external_recovery.rs、test:recovery | Compose手編集/Context切替えで無断上書き・別Engine操作なし。実機未実施 |
| AC-16 | D/template_structure.rs・template_semantics.rs、A/template_package.rs・template_catalog_view.rs、test:templates | ローカルTemplate追加/再読込み/不正分離/旧定義削除。実機未実施 |
| AC-17 | D/compose.rs・clone_policy.rs、A/query_service.rs、test:security・content・logs・clone | 日本語平文説明・明示表示/コピー・特殊文字で実接続。対象3OS未実施 |
| AC-18 | 下記12組の実機表、全10画面 `test:ui` | 3OSの日本語native主要フロー。実機未実施 |
| AC-19 | D/compose.rs、A/artifact_store.rs、両Template Docker受入、test:security・check:contracts | 保存Composeを内部CLIで起動するnativeフロー・配布責務確認。実機未実施 |
| AC-20 | 下記初心者観察票 | 5〜8名・80%以上・作成5分/Clone2分。参加者0名、未実施 |

## CP-T01〜36

Core/SQLite/fake検査の入口を示す。CP-T20〜22・27〜30のサービス別実データ確認は、Core成功だけで完了とせず、両Template×両方式で実施する。

| ID | 期待の焦点 | 実施先 | 残る確認 |
| --- | --- | --- | --- |
| CP-T01 | 空文字・0・falseをcopy | D/clone_policy.rs `cp_t01_copy_preserves_empty_string_zero_and_false` | nativeフォーム |
| CP-T02 | clearはDefaultで埋めない | D/clone_policy.rs `cp_t02_t03_clear_and_absent_copy_never_apply_a_default` | native未設定表示 |
| CP-T03 | 任意未設定のcopy | 上記、A/clone_plan.rs `null_input_is_rejected_and_clear_omits_optional_value` | native確認 |
| CP-T04 | ask表示だけでは未回答 | D/clone_policy.rs `cp_t04_ask_requires_an_explicit_answer`、test:clone | native未回答 |
| CP-T05 | 保存slotへのcopy拒否 | D/clone_policy.rs `cp_t05_t06_reject_disallowed_policies`、D/template_semantics.rs | native不正定義表示 |
| CP-T06 | 整数regenerate拒否 | 上記 | native不正定義表示 |
| CP-T07 | 秘密copyは確認待ち | D/clone_policy.rs `cp_t07_t08_inherited_secret_requires_confirmation_even_if_typed`、test:clone | native確認/取消し |
| CP-T08 | 元と同じ手入力秘密も確認 | 上記、A/clone_plan.rs `clone_secret_reuse_needs_confirmation_even_when_entered_by_hand` | native確認 |
| CP-T09 | 64文字集合・32文字・再生成 | D/clone_policy.rs `cp_t09_secret_has_fixed_alphabet_and_retries_equality` | 実接続 |
| CP-T10 | 乱数失敗・8衝突で失敗 | D/clone_policy.rs `cp_t10_generation_failure_has_no_fallback` | nativeエラー案内 |
| CP-T11 | 無関係な編集で秘密保持 | D/clone_policy.rs `cp_t11_unrelated_plan_changes_leave_secret_candidate_stable`、A/clone_plan.rs | native再描画 |
| CP-T12 | 停止元5432/外部5433→5434 | P/host_ports.rs `stopped_source_reservation_and_external_conflict_advance_without_wrap` | 各OS実ポート |
| CP-T13 | 65535から巻戻さない | 上記 | 各OS実ポート |
| CP-T14 | 1023/65536/非整数拒否 | P/host_ports.rs `invalid_and_boundary_explicit_inputs`、test:forms | native入力 |
| CP-T15 | 1024/65535の明示許可 | 上記 | 各OS実ポート |
| CP-T16 | 複数slotの候補重複解消 | P/host_ports.rs `explicit_first_then_ascii_slots_and_duplicate_inputs` | 各OS実ポート |
| CP-T17 | 明示重複は自動変更しない | 上記 | native修正案内 |
| CP-T18 | 検査不能はUnknown | P/host_ports.rs `unknown_does_not_count_as_occupied`、A/docker_target.rs | 各OS権限/占有 |
| CP-T19 | 予約変化で再確認 | P/host_ports.rs `newly_reserved_automatic_preview_is_replanned_before_confirmation`、A/clone_session.rs | 同時native要求 |
| CP-T20 | ID/Project/全slot分離 | A/clone_plan.rsの候補Template確認、両Template Docker受入のClone | 3OS実機 |
| CP-T21 | 方式間で新領域、移行なし | 同上、`clone_storage_switch_keeps_source_allocation_and_uses_new_method` | 3OS実機 |
| CP-T22 | リンク別名/親子/既存volume拒否 | storage/storage_path/volumes moduleテスト | 両Template×両方式×3OS実検査 |
| CP-T23 | 元更新/改名/削除でStale | A/clone_plan.rs `clone_commit_is_idempotent_and_source_revision_is_guarded`、`committed_spec_change_stales_plan_even_without_instance_revision_change` | native差分再確認 |
| CP-T24 | 観測変化のみなら計画継続 | A/clone_plan.rs `clone_plan_survives_observation_and_temporary_source_operation` | native元のstop/start |
| CP-T25 | 二重確定は既存結果 | A/clone_plan.rsのidempotent、A/clone_session.rs | native応答断 |
| CP-T26 | 確定後は元変更から独立 | state_store moduleのSnapshot copy、A/clone_plan.rs | 確定後の元更新/削除と先不変を明示する実機シナリオは未実施 |
| CP-T27 | 再試行でもID/秘密/領域保持 | A/create_state.rs、A/port_recovery.rs、#48中断試験 | 両Template×両方式実失敗 |
| CP-T28 | 旧Committed/新Held→Ready | A/port_recovery.rs `creation_conflict_proposes_without_changes_and_rechecks_confirmation` | 両Template×両方式実競合 |
| CP-T29 | 復旧直後終了でも予約保持 | A/port_recovery.rs `port_edit_crash_and_full_database_keep_old_and_new_reservations_until_reconciled` | 両Template×両方式native終了 |
| CP-T30 | 消失はMissing、再初期化禁止 | storage module `missing_previous_storage_is_not_recreated`、A/port_recovery.rs | 両Template×両方式実消失 |
| CP-T31 | 取消し資源なし/応答断照会 | A/create_session.rs・clone_session.rs、test:create・clone | native応答断 |
| CP-T32 | 旧Snapshot維持 | A/clone_plan.rs `clone_version_switch_uses_snapshot_and_shows_removed_fields_and_slots` | native旧定義削除 |
| CP-T33 | Version変更の必須追加 | A/clone_plan.rs `clone_version_change_requires_answers_and_commits_only_active_inputs`、test:forms | native未回答 |
| CP-T34 | 秘密Validation変更で再入力 | A/clone_session.rs `source_secret_stays_masked_and_type_change_blocks_confirmation_after_explicit_answer`、A/clone_plan.rs | native再入力 |
| CP-T35 | 接続情報は先の値から生成 | A/query_service.rs `saved_list_and_detail_use_committed_ports_and_mask_secrets`、A/redis_template.rs、両Docker Clone | native接続情報からの接続 |
| CP-T36 | named既定でもbind元を継承 | A/clone_plan.rs `clone_version_switch_uses_snapshot_and_shows_removed_fields_and_slots` | native設定→Clone |

## 2 Template × 2保存方式 × 3OS

通常利用者で配布アプリを起動する。各行で作成・接続・stop/start/restart・同方式Clone・方式間Clone・編集・復旧・データ保持削除を行い、元/先のmarkerを照会する。PostgreSQLは17/18を記録し、Redisは8.2のAOFを有効にする。候補定義は専用検証環境でローカル追加する。候補は同梱済みと扱わない。

| 対象OS/CPU | Template | 元保存方式 | Clone先 | 現在の実機結果 |
| --- | --- | --- | --- | --- |
| Windows（最低版未決）x86_64 | PostgreSQL 17/18 | bind | bind/named | 未実施 |
| Windows（最低版未決）x86_64 | PostgreSQL 17/18 | named | named/bind | 未実施 |
| Windows（最低版未決）x86_64 | Redis 8.2 | bind | bind/named | 未実施 |
| Windows（最低版未決）x86_64 | Redis 8.2 | named | named/bind | 未実施 |
| macOS 15以上 ARM64 | PostgreSQL 17/18 | bind | bind/named | 未実施 |
| macOS 15以上 ARM64 | PostgreSQL 17/18 | named | named/bind | 未実施 |
| macOS 15以上 ARM64 | Redis 8.2 | bind | bind/named | 未実施 |
| macOS 15以上 ARM64 | Redis 8.2 | named | named/bind | 未実施 |
| Ubuntu 26.04 x86_64 | PostgreSQL 17/18 | bind | bind/named | 未実施 |
| Ubuntu 26.04 x86_64 | PostgreSQL 17/18 | named | named/bind | 未実施 |
| Ubuntu 26.04 x86_64 | Redis 8.2 | bind | bind/named | 未実施 |
| Ubuntu 26.04 x86_64 | Redis 8.2 | named | named/bind | 未実施 |

OS差は[Windows](windows-validation.md)・[macOS](macos-validation.md)・[Ubuntu](ubuntu-validation.md)の権限手順も併用する。空白/日本語/大小文字/区切り文字・symlink/junction・親子パス、管理ルートの通常利用者書込み/第三者拒否、IPv4/IPv6 loopback・停止中予約・外部占有・検査権限を各実機で確認する。macOSのDesktop共有設定、Windows ACLとDocker DesktopのLinuxモード、Ubuntu 26.04のbind所有権を記録する。CIのUbuntu 24.04は採用対象Ubuntu 26.04の代替ではない。

### 実データCloneの再実行

```sh
cargo test -p composenest-adapters --locked --test clone_plan candidate_templates_confirm_independent_clones_in_all_storage_combinations -- --exact
cargo test -p composenest-adapters --locked --test postgresql_template docker_postgresql_versions_preserve_authenticated_data_in_both_storage_modes -- --ignored --exact --nocapture
cargo test -p composenest-adapters --locked --test redis_template docker_redis_preserves_authenticated_aof_data_in_both_storage_modes -- --ignored --exact --nocapture
```

後二つは既存CI `postgresql-template` / `redis-template` で実行する。Docker/Compose、ホストTCP client（psql/redis-cli）、Image取得権限が必要。実CoreのClonePlans→SQLite確定値からComposeを生成し、同方式・方式間の元marker欠如、先の別marker、元marker不変を検証する。専用一時root/project/volumeだけを片付ける。native IPC・Runner全段階・各OS配布検証は含まない。

## 30件の応答性・暫定期限

```sh
cargo test -p composenest-adapters --locked --test query_service thirty_instance_queries_record_response_times_without_exposing_secrets -- --exact --nocapture
pnpm --dir apps/desktop run build
pnpm --dir apps/desktop run test:acceptance-performance
```

前者は実SQLiteに保存した30件を使用し、warm-up後20回の一覧/詳細queryを計測する。JSON行 `ACCEPTANCE_PERFORMANCE` にOS/CPUアーキテクチャ/全sampleを保存し、3OSの `acceptance-performance-<runner>` CI artifactへ残す。コンテナ30件の起動・監視は含まない。

後者はproduction React assetsとfake list IPCを用い、ライト/ダーク×カード/Gridで30件の更新・検索・絞り込み・Gridソートを各10回、受付表示を各1回計測する。IPC待ち中も検索入力できることを確認する。ブラウザ時計から二つのpaint機会までの時間には自動操作の調整時間も含む。中央値/p95/maxと全sample・commit/日時/OS/CPU/Node/browser・実行成功/失敗を `apps/desktop/test-results/acceptance-performance.json` へ保存し、`common-shell-screenshots` artifactへ残す。1秒は受付表示の暫定比較値で、CIホストの固定性能合否にしない。検索/ソートへ新しい保証値を追加しない。

| 項目 | 暫定値 | 自動確認 | 対象実機の測定結果 |
| --- | --- | --- | --- |
| 操作受付 | 1秒 | 上記pending-feedback | native作成/Clone/管理受付は未実施 |
| 診断/inspect | 10秒/呼出し | A/docker_cli.rsの短縮deadline・Unknown/reap | 実際の10秒・画面継続は未実施 |
| Compose検証 | 30秒 | Runner引数/失敗時保留の回帰 | 実際の30秒は未実施 |
| Image取得 | 別枠15分 | 取得失敗・同一確定値保持の回帰 | 実際の15分は未実施 |
| create/stop/restart/remove | 120秒 | A/docker_cli.rs・create_state/delete_state/port_recovery | 実際の120秒は未実施 |
| stop正常終了猶予 | 30秒（120秒内） | 両Template Docker受入のstop/start | 対象3OS未実施 |
| Ready待ち | 180秒 | Docker受入のwait上限180秒、短縮失敗回帰 | timeoutまでの実測・再観測は未実施 |
| 平常観測/操作中 | 5秒/2秒、失敗時最大30秒 | UI一覧の更新は保存状態再取得 | 30実Instanceの監視間隔/CPU/応答は未実施 |
| ログ | 2,000行/2MiB、1行16KiB | test:logs・test:ui、ログbuffer module | native上限到達/欠落表示は未実施 |
| CLI診断出力 | 64KiB | A/docker_cli.rs `large_output_is_drained_and_bounded` | 対象3OS未実施 |

実機では30件を保存し、実際の稼働件数・CPU/メモリー・cold/warm・ストレージ・Image取得時間を記録する。更新/検索/ソート/詳細/作成受付/Clone受付/停止受付を各10回以上測り、中央値/p95/max・1秒超過回数・UI入力応答を残す。期限超過は実時間、結果不明表示、子process回収、実体再観測、秘密/予約/保存先不変を一緒に確認する。短縮deadlineのテスト成功を10秒〜15分の実測として記録しない。

## 全10画面・キーボード・日本語

[全10画面の比較手順と証跡](ui-validation.md)に従い、`test:ui` / `test:shell` / `test:list` / `test:create` / `test:clone` / `test:recovery` / `test:content` / `test:logs` を実行する。ライト/ダーク、カード/Grid、検索/絞り込み/ソート、Template再読込み、診断、既定保存方式、保持一覧、作成/Clone→Operation→詳細、編集/削除/復旧を対象にする。

各native OSでマウスなしにskip link→ナビゲーション→入力→確認→Operation→詳細まで進む。Tab/Shift+Tab、Enter/Space、Escape、矢印/Home/End、dialog初期フォーカス/閉込め/起点復帰、狭幅表スクロールを確認する。IME日本語入力・OS clipboard・native WebViewのフォーカス・画面読み上げはブラウザfake IPC試験の合格へ含めない。

失敗・結果不明・未回答・Stale・所有不明・Missingを実操作で発生させ、日本語で次の行動を読めることを記録する。平文保存、元データ非複製、先ポート変更、削除後保持、閉じても停止しない説明を確認し、技術error原文・秘密を通常表示しない。自動locatorによる文言確認は初心者の理解の証拠ではない。

## 初心者観察（AC-20）

現在0名、未実施。募集・対人連絡はこのPRに含めない。Docker/Compose準備済み・Image取得済みの通常利用者環境で、Docker初心者5〜8名を観察する。匿名IDと事前経験を記録し、手順を教え込まず同じ課題文を渡す。サービスclientの接続確認方法は開始前に用意する。

1. 「開発用のサービスを作り、画面の情報を使って接続してください。」Template選択から接続成功までを作成時間とする。Compose手編集は禁止。
2. 元にmarkerを用意してから「別用途の環境を複製し、接続してください。」Clone開始から先への接続成功までを計測する。先に元markerがなく、先への書込み後に元marker不変を観察者が確認する。
3. 「元データ、接続ポート、削除後のデータはどうなりますか。」誘導せず回答を記録する。データ非複製・独立ポート・削除時保持を説明できることを安全理解として別集計する。

Image取得待ちが発生した場合は実経過と取得待ちを別記し、差引後の作成/Clone時間を残す。エラー復旧・迷い・読解・その他待ち時間は勝手に除外しない。支援した内容と時点、中断・失敗・時間超過・未達理由を記録する。安全説明/確認を時間短縮のために省かない。

| 匿名ID | OS/Template/方式・経験 | 作成実経過/取得待ち/計測時間 | Clone実経過/取得待ち/計測時間 | 支援なし両方完了 | 安全理解3項目 | 支援/未達理由・証跡 |
| --- | --- | --- | --- | --- | --- | --- |
| P01 | 未実施 | — | — | 未実施 | 未実施 | 参加者未確保 |
| P02 | 未実施 | — | — | 未実施 | 未実施 | 参加者未確保 |
| P03 | 未実施 | — | — | 未実施 | 未実施 | 参加者未確保 |
| P04 | 未実施 | — | — | 未実施 | 未実施 | 参加者未確保 |
| P05 | 未実施 | — | — | 未実施 | 未実施 | 参加者未確保 |
| P06〜08（任意） | 未実施 | — | — | 未実施 | 未実施 | 参加者未確保 |

分母は観察開始した全参加者N（中断・失敗も含む）、分子は支援なしで作成とCloneの両方を完了した人数。N=0の達成率は「未実施」とし、0%や100%に置き換えない。暫定80%には5名なら4名、6名なら5名、7名なら6名、8名なら7名が必要。作成≤300秒・Clone≤120秒の個人結果と達成人数、安全理解の結果を別に記録し、完了率だけで時間目標達成と扱わない。

## このPRの実施記録

環境: Ubuntu 24.04.3 x86_64のクラウドコンテナ、Node 24.19.0、pnpm 11.25.0（lockfile固定）、Rust 1.98.1。2026-10-08実行。対象実機Ubuntu 26.04ではない。Docker/通常利用者のnativeデスクトップ/被験者はなし。

| 実施 | 結果・理由 |
| --- | --- |
| frozen lockfile取得・frontend build/型検査・check:contracts | 成功 |
| test:security / test:navigation / formModel・listModel | 成功（個別件数はPR検証欄） |
| Rust fmt | 成功。ローカルRustは通常codegenでゼロ長objectによるarchive生成失敗の後、CARGO_INCREMENTAL=0・CODEGEN_UNITS=1・DEBUG=0で再実行し成功 |
| Rust test/clippy | Domain/Application/Adaptersの336テスト成功、Docker必要5件ignore。3 crateのall-targets clippy（-D warnings）成功。native workspaceはCIで確認する |
| SQLite保存30件のquery測定 | 一覧20回: 中央値5.304ms / p95 6.659ms / max 8.555ms。詳細20回: 中央値0.876ms / p95 1.153ms / max 1.351ms。コンテナ/GUIを除く |
| production UIの30件測定 | ローカルChrome起動がsocket権限エラーで失敗。測定値なし。CIで実行する |
| Docker両Template×両方式Clone | ローカル未実施（Dockerなし）。既存CIの専用受入jobで実行する |
| 対象3OSのnative主要フロー・期限実測・初心者観察 | 未実施。上記票へ環境と結果を追記する |

CIの成功だけで最後の行を合格に変更しない。配布受入とTemplate昇格の未完了は維持する。
