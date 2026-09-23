# Ubuntu 26.04 x86_64 権限・実機検証

Issue #7の検証手順。初期化スクリプトはインストーラー完成前の初回セットアップ用であり、実行時は明示した既存の一般ユーザー1人だけに管理ルートを割り当てる。通常のGUIは`sudo`で起動しない。

## 初期セットアップ

```bash
sudo bash scripts/linux/initialize-management-root.sh "$(id -un)"
stat -c '%U:%G %a %n' /var/lib/composenest /var/lib/composenest/state
```

引数にrootは指定できない。既存の管理ディレクトリの所有者・モードが異なる場合やリンクの場合は停止し、再帰的な`chown`や`chmod`はしない。Dockerグループ、socket、既存データの所有者は変更しない。親`/var/lib`はOS管理下の通常ディレクトリであることを事前に確認する。作成される管理ディレクトリは0700。今後SQLite・生成Compose・所有証拠を作成する際は通常プロセスで`umask 077`を適用し、秘密を含むファイルとWAL・バックアップを0600相当にする必要がある。現時点ではこれらのファイルをまだ作成しない。

## 自動検証

`sudo bash scripts/linux/test-initialize-management-root.sh`で、空白・日本語パス、初期化の再実行、一般ユーザーによる0600のファイル作成、Imageによるbind子ディレクトリのUID/GID変更を模した状態、リンク・所有者不一致の拒否を一時領域で検査する。テストには`nobody`と`runuser`を使い、Dockerは不要。テストではbind子領域が読めなくなっても、親`data`と`state`を一般ユーザーが管理できることを確認する。実際のImageによる書込みは次の実機試験で別途確認する。

## Ubuntu 26.04 実機記録欄

対象: Ubuntu 26.04 x86_64、ローカルrootful Docker Engine、WebKitGTK 4.1。Dockerアクセスが既に許可された一般ユーザーで検証する。Dockerグループへ自動追加しない。Docker socketの権限不足時は診断して中断する。

| 項目 | 手順・期待結果 | 結果 |
| --- | --- | --- |
| 環境 | `lsb_release -a`, `uname -m`, `docker info`, `docker compose version`, `id`を記録 | Ubuntu 26.04.1 x86_64、rootful Docker Engine 29.6.1、Compose v2.40.3、UID/GID 1000:1000 |
| GUIとWebKitGTK | `cargo run -p composenest-desktop`を一般ユーザーで起動し、画面表示・`get_bootstrap`成功・プロセスUIDとWebKitGTK 4.1の依存を確認 | 成功。WebKitGTK 4.1は2.52.6、GUIプロセスはUID 1000 |
| 管理ルート | 上記初期化後に一般ユーザーが`state`へ書込め、別の一般ユーザーが読めないことを確認 | 成功。全管理ディレクトリは1000:1000、0700。検証ファイルは0600で、`nobody`の読取りを拒否 |
| Image UID/GID | 専用の一時bind領域にrootful Dockerで書き込むImageを起動し、ホストの`stat -c '%u:%g %a'`で子領域の変化を観測。親`data`、DB領域、所有証拠への管理権限を確認 | ホーム配下の一時領域で成功。子領域は12345:12345、0700。親`data`と管理ルートの`state`・`ownership`は1000:1000、0700を維持 |
| 日本語・空白パスとbind | 専用の一時パスをDockerへ`--mount type=bind,src=...,dst=/data`で渡し、コンテナから書込み、ホストで内容を確認 | ホーム配下の一時領域で成功。コンテナ作成ファイルをホストで確認 |
| 子プロセス終了 | 今後Docker CLI Adapterを追加した後、タイムアウト時のCLIと子プロセスの終了、前試行の生存確認とEngine実体の再照合を検証 | 未実施（Adapter未実装） |

bind先の所有者がImageのUID/GIDへ変わりアクセス不能なら、全員書込みへ緩めたり既存データを自動chownしたりしない。管理ルートと所有証拠の保護を維持し、操作を停止して新規作成時のnamed volumeを案内する。rootful Dockerの権限がある利用者はホストに対して強い権限を持つため、0600はDocker管理者からの秘匿を保証しない。

本リポジトリのCIはUbuntu 24.04を使用しており、26.04でのGUI・Docker実機成功を代替しない。実機で検証したらOS、Docker、WebKitGTKの版、利用者UID/GID、実際の観測結果をこの表またはPRへ追記する。

## 2026-09-23 ローカルPCでの確認

Ubuntu 26.04.1 LTS x86_64、利用者`admlocal`（UID/GID 1000:1000）で確認した。Docker CLIは28.5.2、Engineは29.6.1、Composeはv2.40.3、WebKitGTK 4.1は2.52.6。Docker EngineはrootfulのSnap版である。利用者は`docker`グループに所属せず、通常のDocker socket接続は`permission denied`となった。Docker試験では永続的なグループ設定を変更せず、検証プロセスだけに補助グループ`docker`を与えた。

- `unshare --map-auto --map-root-user bash scripts/linux/test-initialize-management-root.sh`とホスト上でのroot実行：成功。所有者変更、一般ユーザーの0600ファイル作成、再実行、リンク拒否を確認した。
- ホストの`/var/lib/composenest`を初期化し、全管理ディレクトリが1000:1000、0700であることを確認した。一般ユーザーが`state`に0600ファイルを作成でき、`nobody`は読めなかった。検証ファイルは削除した。
- `cargo run -p composenest-desktop`：UID 1000で起動し、日本語の初期画面を表示した。`get_bootstrap`の確認時だけ初期タイトルを`IPC 未確認`に変更し、呼び出し後に`ComposeNest`へ更新されることを画面で確認した。ソースは元に戻し、フロントエンドを再ビルドした。
- rootful Dockerの`busybox:1.37`から日本語・空白を含むホーム配下のbindパスに書き込み、ホストで内容を確認した。別のbind子領域をImage側で12345:12345、0700に変更しても、親`data`と管理ルートの`state`・`ownership`の管理権限は維持された。
- Snap版Dockerのdaemonは`/tmp`と`/var/lib/composenest/data`のbind元を参照できず、`bind source path does not exist`を返した。このPCでは管理ルート内のbindを実証できていない。ホーム配下のbind試験と区別し、今後の保存領域実装時にSnap版Dockerのパス制約を扱う。
- `pnpm --dir apps/desktop run check:contracts`と`pnpm --dir apps/desktop build`：成功。
- `cargo fmt --all -- --check`、`cargo test --workspace`、`cargo clippy --workspace --all-targets -- -D warnings`：成功。
