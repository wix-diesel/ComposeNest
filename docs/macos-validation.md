# macOS ARM64 管理ルート・権限検証

Issue #6 の初回セットアップと検証手順。対象は Apple Silicon の macOS 14 以降とする。通常 GUI は昇格せずに起動し、初期化だけを管理者権限で実行する。初期化先と異なるユーザーを所有者にしたい場合でも、昇格したプロセスの UID を所有者として推測しない。

## 初回セットアップ

通常利用者のターミナルでユーザー名を確認し、別の管理者ターミナルから明示して実行する。

```bash
id -un
sudo bash scripts/macos/initialize-management-root.sh '<通常利用者名>'
stat -f '%Su:%Sg %Lp %N' '/Library/Application Support/ComposeNest' '/Library/Application Support/ComposeNest/state'
```

スクリプトは `/Library/Application Support/ComposeNest` と管理用の直下ディレクトリを作成し、明示した一般ユーザーの UID/GID と 0700 を設定する。既存の管理ディレクトリの所有者・モードが異なる場合やリンクの場合は停止し、再帰的な `chown` や `chmod` はしない。Docker Desktop の設定、共有パス、既存データの所有者は変更しない。

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

ローカルまたは CI の macOS ホストで、一般ユーザーから次を実行する。

```bash
sudo bash scripts/macos/test-initialize-management-root.sh
```

テストは一時的な日本語・空白パスで、初期化の再実行、一般ユーザーによる 0600 の状態ファイル作成、Image による bind 子ディレクトリの UID/GID 変更を模した状態、リンク・所有者不一致の拒否を検査する。Docker は不要で、`/Library/Application Support/ComposeNest` は変更しない。bind 子領域が読めなくなっても、親 `data` と `state` を一般ユーザーが管理できることを確認する。

## 実機受入記録

対象は Apple Silicon、Docker Desktop の Linux containers、Docker へのアクセスが既に許可された一般ユーザーである。Docker Desktop の権限や共有設定をこの手順で変更しない。Docker 接続に失敗する場合は診断して中断する。

| 項目 | 手順・期待結果 | 結果 |
| --- | --- | --- |
| 環境 | `sw_vers`, `uname -m`, `docker version`, `docker compose version`, `id`を記録 | 未実施。macOS 14 以降・ARM64・Docker Desktop の実機で記録する |
| Host Adapter | `cargo test -p composenest-adapters`で `/Library/Application Support/ComposeNest` を解決するテストを実行 | macOS CI で確認予定 |
| GUI と一般権限 | 初期化後、通常利用者で `cargo run -p composenest-desktop`を起動し、画面表示・`get_bootstrap`成功・プロセス UID を確認 | 未実施。GUI 実装の受入時に実機で確認する |
| 管理ルート | 上記初期化後、通常利用者が `state` へ書込み、別ユーザーが読めないことを確認 | macOS CI は一時パスで自動検証。実際の管理ルートは未実施 |
| Image UID/GID | 専用の bind 子領域で UID/GID を変更する Image を起動し、親 `data`、`state`、`ownership` の保護が維持されることを確認 | 未実施。Docker Desktop の実機で確認する |
| 日本語・空白パスと bind | 日本語・空白を含む管理ルート配下の一時 bind パスへコンテナから書込み、ホストで内容を確認 | 未実施。Docker Desktop の実機で確認する |
| CLI 子プロセス終了 | Docker CLI の終了後、コンテナ状態を再照合する | 未実施。Docker CLI Adapter 未実装 |

bind 先の所有者が Image の UID/GID に変わりアクセス不能になった場合、全員書込みへ緩めたり既存データを自動 `chown` したりしない。管理ルートと所有証拠の保護を維持し、操作を停止して新規作成時の named volume を案内する。Docker Desktop の管理権限を持つ利用者からの秘匿を 0600 だけで保証するものではない。

macOS CI の成功は Docker Desktop を含む実機受入の代替ではない。実機で確認したら、macOS、Docker Desktop、Engine、Compose、利用者 UID/GID、実際の観測結果をこの表または PR に追記する。

## 2026-09-23 ローカル PC での確認

Apple Silicon の macOS 26.6.1（build 25G76）、利用者 `wixdiesel`（UID/GID 501:20）で確認した。

- `pnpm --dir apps/desktop install --frozen-lockfile`、`pnpm --dir apps/desktop run check:contracts`、`pnpm --dir apps/desktop run build`：成功。
- `cargo fmt --all -- --check`、`cargo test --workspace`、`cargo clippy --workspace --all-targets -- -D warnings`：成功。Host Adapter の `/Library/Application Support/ComposeNest` 解決テストを含む。
- `bash -n scripts/macos/initialize-management-root.sh scripts/macos/test-initialize-management-root.sh`：成功。管理者パスワードが利用できないため、`sudo` が必要な一時パスの初期化試験と実際の管理ルート初期化は未実施である。PR の macOS CI で一時パス試験を実行する。
- Docker CLI は 29.7.2、Compose は v5.3.1。Docker Desktop の daemon は起動しておらず、`~/.docker/run/docker.sock` への接続が失敗したため、bind 書込みと CLI 子プロセス終了は未実施である。
