# 開発基盤

Ubuntuの管理ルート初期化と実機検証は[Ubuntu 26.04 権限・実機検証](ubuntu-validation.md)を参照。

## 採用版

Issue #2で、次の組合せをWindowsで依存取得・型検査・Rustテスト・フロントエンドビルドに使用した。各依存の解決結果は`Cargo.lock`と`apps/desktop/pnpm-lock.yaml`へ固定する。

| 対象 | 採用版 |
| --- | --- |
| Rust | 1.98.1、edition 2024（GNU toolchainとMSVC toolchain） |
| Node.js | 24.19.0 |
| React／React DOM | 19.3.0 |
| TypeScript | 6.0.3 |
| Vite | 8.3.0 |
| Tauri Rust | 2.11.6 |
| tauri-build | 2.6.3 |

Node.js 24.19.0はこの開発環境の実測値である。利用者の実行環境にはNode.jsを要求しない。Windows以外のTauri起動と配布物は、それぞれのOS検証Issueで確認する。

## 実行手順

```text
pnpm --dir apps/desktop install --frozen-lockfile
pnpm --dir apps/desktop build
cargo test --workspace
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo run -p composenest-desktop
```

`cargo run -p composenest-desktop`は、Viteで生成した画面を読み、日本語の初期画面を表示する。Issue #2では未使用のTauri JavaScript API／CLIを導入しない。IPC用JavaScript APIとTauri CLIの追加は、型付きIPC契約を扱うIssue #4で検証する。DomainとApplicationはTauriや具体Adapterに依存せず、desktop起動コードが`SystemClock`を組み立てる。

MSVC toolchainで`cargo clippy`を実行する場合は、Visual Studioの`VsDevCmd.bat`で開発環境を読み込んでから実行する。

この基盤は業務機能、SQLite、Docker操作、IPC command、Template処理を含まない。それらは後続Issueで追加する。

## 検証記録

Windowsで最終検証を実行し、次のコマンドが成功した。

- `pnpm --dir apps/desktop install --frozen-lockfile`
- `pnpm --dir apps/desktop build`
- `cargo test -p composenest-domain -p composenest-application -p composenest-adapters`
- `cargo fmt --all -- --check`
- `cargo clippy --workspace --all-targets -- -D warnings`（MSVC Build Tools環境）
- `cargo check -p composenest-desktop`（MinGWの`dlltool.exe`を使用）
- `cargo run -p composenest-desktop`

## SQLite 起動基盤（Issue #10）

起動時に管理ルートの `locks/backend.lock` をOSロックし、`state/composenest.sqlite` を開く。Linuxでは先に `scripts/linux/initialize-management-root.sh` で管理ルートを用意する。起動処理は schema version を確認し、未知の新版を拒否してから `foreign_keys=ON`、WAL、`synchronous=FULL` を設定する。migration は専用DB workerで順に適用し、失敗した場合はトランザクション全体をロールバックする。書込みは専用キュー、読取りは別接続の短いトランザクションで行う。Docker CLI の実行はDBトランザクション外で行う。

既存DBを更新するときは `state/migration-backups/` にSQLite Backup APIで整合した退避を作り、`quick_check` と版を確認する。退避は自動削除しない。更新が成功し、アプリの状態を確認できた後に、配布・運用手順で不要な退避を整理する。復旧時はアプリを停止し、DBとWALの整合を確認した上で退避を扱う。DB、WAL、SHM、退避は秘密を含み得る。Unixでは管理ディレクトリを0700、各ファイルを0600相当に制限する。Windowsでは管理ルートのACL設定を配布検証で確認する必要がある。

`rusqlite 0.40.2` の `bundled` 機能を使用し、同梱SQLiteは `libsqlite3-sys 0.38.2` の 3.53.2 である。設計書の3.53.4は未確定の候補版である。
