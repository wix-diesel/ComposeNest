# macOS ARM64 管理ルート・権限検証

Issue #6 の初回セットアップと検証手順。対象 CPU は Apple Silicon で、macOS 14 を最低版の検証候補とする。通常 GUI は昇格せずに起動し、初期化だけを管理者権限で実行する。初期化先と異なるユーザーを所有者にしたい場合でも、昇格したプロセスの UID を所有者として推測しない。

## 初回セットアップ

通常利用者のターミナルでユーザー名を確認し、別の管理者ターミナルから明示して実行する。

```bash
id -un
sudo bash scripts/macos/initialize-management-root.sh '<通常利用者名>'
stat -f '%Su:%Sg %Lp %N' '/Library/Application Support/ComposeNest' '/Library/Application Support/ComposeNest/state'
```

スクリプトは `/Library/Application Support/ComposeNest` と管理用の直下ディレクトリを作成し、明示した一般ユーザーの UID/GID と 0700 を設定する。既存の管理ディレクトリの所有者・モードが異なる場合、リンクや ACL がある場合は停止し、再帰的な `chown` や `chmod` はしない。Docker Desktop の設定、共有パス、既存データの所有者は変更しない。

通常利用者のターミナルから、確認後に削除する状態ファイルを作成する。

```bash
root='/Library/Application Support/ComposeNest'
probe="$root/state/permission-probe.txt"
umask 077
printf %s ok > "$probe"
cat "$probe"
rm "$probe"
```

今後 SQLite・生成 Compose・所有証拠を作成する通常プロセスでは `umask 077` を適用し、秘密を含むファイル、WAL、バックアップを 0600 相当にする必要がある。現時点ではアプリによるこれらのファイル作成は未実装である。

## 自動検証

ローカルでは一般ユーザーのまま一時パスの主要な保護動作を確認できる。CI では昇格して所有者不一致と Image 所有 UID の模擬も確認する。

```bash
bash scripts/macos/test-initialize-management-root.sh
sudo bash scripts/macos/test-initialize-management-root.sh
```

テストは一時的な日本語・空白パスで、初期化の再実行、一般ユーザーによる 0600 の状態ファイル作成、bind 子領域だけのアクセス拒否、リンク・モード・ACL 不一致の拒否を検査する。昇格した実行では、Image による bind 子領域の UID/GID 変更と、所有者不一致の拒否も検査する。Docker は不要で、`/Library/Application Support/ComposeNest` は変更しない。bind 子領域が読めなくなっても、親 `data` と `state` を一般ユーザーが管理できることを確認する。

## 実機受入記録

対象は Apple Silicon、Docker Desktop の Linux containers、Docker へのアクセスが既に許可された一般ユーザーである。Docker Desktop の権限や共有設定をこの手順で変更しない。Docker 接続に失敗する場合は診断して中断する。

| 項目 | 手順・期待結果 | 結果 |
| --- | --- | --- |
| 環境 | `sw_vers`, `uname -m`, `docker version`, `docker compose version`, `id`を記録 | macOS 26.6.1 の環境記録は後述。最低版候補 macOS 14 と Docker Desktop の実機照合は未実施 |
| Host Adapter | `cargo test -p composenest-adapters`で `/Library/Application Support/ComposeNest` を解決するテストを実行 | macOS 26.6.1 ARM64 で成功。macOS 14 CI でも確認する |
| GUI と一般権限 | 初期化後、通常利用者で `cargo run -p composenest-desktop`を起動し、画面表示・`get_bootstrap`成功・プロセス UID を確認 | 通常利用者 UID 501 でプロセス起動を確認。管理ルートを使う GUI 操作と画面表示・IPC 成功は未確認 |
| 管理ルート | 上記初期化後、通常利用者が `state` へ書込み、別ユーザーが読めないことを確認 | 一時パスの非昇格テストは成功。実際の管理ルートは管理者認証待ち |
| Image UID/GID | 専用の bind 子領域で UID/GID を変更する Image を起動し、親 `data`、`state`、`ownership` の保護が維持されることを確認 | 一時パスの所有者変更は macOS CI で確認する。Docker Desktop の管理ルート配下では未実施 |
| 日本語・空白パスと bind | 日本語・空白を含む管理ルート配下の一時 bind パスへコンテナから書込み、ホストで内容を確認 | ユーザー所有の一時パスで成功。管理ルート配下は初期化後に確認する |
| CLI 子プロセス終了 | Docker CLI の終了後、コンテナ状態を再照合する | 検証用 CLI は SIGTERM 後も残り、SIGKILL 後に終了。コンテナは稼働を継続し、確認後に削除した。Adapter は未実装 |

bind 先の所有者が Image の UID/GID に変わりアクセス不能になった場合、全員書込みへ緩めたり既存データを自動 `chown` したりしない。管理ルートと所有証拠の保護を維持し、操作を停止して新規作成時の named volume を案内する。Docker Desktop の管理権限を持つ利用者からの秘匿を 0600 だけで保証するものではない。

macOS CI の成功は Docker Desktop を含む実機受入の代替ではない。実機で確認したら、macOS、Docker Desktop、Engine、Compose、利用者 UID/GID、実際の観測結果をこの表または PR に追記する。

## 2026-09-23 ローカル PC での確認

Apple Silicon の macOS 26.6.1（build 25G76）、利用者 `wixdiesel`（UID/GID 501:20）で確認した。

- `pnpm --dir apps/desktop install --frozen-lockfile`、`pnpm --dir apps/desktop run check:contracts`、`pnpm --dir apps/desktop run build`：成功。
- `cargo fmt --all -- --check`、`cargo test --workspace`、`cargo clippy --workspace --all-targets -- -D warnings`：成功。Host Adapter の `/Library/Application Support/ComposeNest` 解決テストを含む。
- `bash -n scripts/macos/initialize-management-root.sh scripts/macos/test-initialize-management-root.sh` と、非昇格の `bash scripts/macos/test-initialize-management-root.sh`：成功。管理者認証が必要な所有者変更試験と実際の管理ルート初期化は保留。PR の macOS CI で所有者変更試験を実行する。
- `cargo run -p composenest-desktop`：通常利用者 UID 501 のプロセス起動を確認して終了した。画面表示と IPC の目視確認は未実施。
- Docker Desktop 4.86.0、Docker CLI／Engine 29.7.2、Compose v5.3.1、Linux VM は ARM64。既存のローカル `postgres:17-alpine` Image から、日本語・空白を含む一時 bind パスへ書き込み、ホストで内容を確認した。
- 同じ Image の検証用コンテナで、ホスト側 `docker run` の CLI へ SIGTERM を送ったが終了しなかった。SIGKILL で CLI が終了した後もコンテナは稼働を継続した。状態確認後、検証用コンテナを削除した。今後の Docker CLI Adapter は終了後に Engine を再照合する必要がある。
