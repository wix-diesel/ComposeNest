# 開発基盤

Ubuntuの管理ルート初期化と実機検証は[Ubuntu 26.04 権限・実機検証](ubuntu-validation.md)を参照。
Windowsの管理ルート初期化と実機検証は[Windows 管理ルート・権限検証](windows-validation.md)を参照。

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
