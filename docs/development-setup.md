# 開発基盤

更新日: 2026-09-22

## 採用版

Issue #2で、次の組合せをWindowsで依存取得・型検査・Rustテスト・フロントエンドビルドに使用した。各依存の解決結果は`Cargo.lock`と`apps/desktop/pnpm-lock.yaml`へ固定する。

| 対象 | 採用版 |
| --- | --- |
| Rust | 1.98.1、edition 2024 |
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
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
pnpm --dir apps/desktop build
cargo run -p composenest-desktop
```

`cargo run -p composenest-desktop`は、Viteで生成した画面を読み、日本語の初期画面を表示する。Issue #2では未使用のTauri JavaScript API／CLIを導入しない。IPC用JavaScript APIとTauri CLIの追加は、型付きIPC契約を扱うIssue #4で検証する。DomainとApplicationはTauriや具体Adapterに依存せず、desktop起動コードが`SystemClock`を組み立てる。

この基盤は業務機能、SQLite、Docker操作、IPC command、Template処理を含まない。それらは後続Issueで追加する。

## 検証記録

2026-09-22にWindowsのNode.js 24.19.0、Rust 1.98.1 GNU toolchainで、依存取得、`pnpm --dir apps/desktop build`、Core 3 crateの単体テストを実行した。React/Viteの型検査とビルド、Domain・Application・Adapterのテストは成功している。

同日にMinGWの`C:\\ProgramData\\mingw64\\mingw64\\bin`を`PATH`へ追加して、`dlltool.exe`を使用する`cargo check -p composenest-desktop`を成功させた。さらに`cargo run -p composenest-desktop`は実行ファイルのビルドと起動まで成功している。Windowsリソース用の`icons/icon.ico`を追加し、Tauriが生成する`src-tauri/gen/`は追跡対象外とした。

2026-09-22に、`rustfmt`を追加したRust 1.98.1 MSVC toolchainで`cargo fmt --all -- --check`を成功させた。`cargo clippy -p composenest-domain -p composenest-application -p composenest-adapters --all-targets -- -D warnings`も成功している。

MSVC Build Toolsの`VsDevCmd.bat`で開発環境を読み込んだ後、`cargo clippy --workspace --all-targets -- -D warnings`も成功した。Windowsデスクトップcrateを含む全ワークスペースをMSVC toolchainで検証する場合は、同開発環境を読み込んでから実行する。
