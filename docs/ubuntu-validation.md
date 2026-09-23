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
| 環境 | `lsb_release -a`, `uname -m`, `docker info`, `docker compose version`, `id`を記録 | 未実施 |
| GUIとWebKitGTK | `cargo run -p composenest-desktop`を一般ユーザーで起動し、画面表示・`get_bootstrap`成功・プロセスUIDとWebKitGTK 4.1の依存を確認 | 未実施 |
| 管理ルート | 上記初期化後に一般ユーザーが`state`へ書込め、別の一般ユーザーが読めないことを確認 | 未実施 |
| Image UID/GID | 専用の一時bind領域にrootful Dockerで書き込むImageを起動し、ホストの`stat -c '%u:%g %a'`で子領域の変化を観測。親`data`、DB領域、所有証拠への管理権限を確認 | 未実施 |
| 日本語・空白パスとbind | 専用の一時パスをDockerへ`--mount type=bind,src=...,dst=/data`で渡し、コンテナから書込み、ホストで内容を確認 | 未実施 |
| 子プロセス終了 | 今後Docker CLI Adapterを追加した後、タイムアウト時のCLIと子プロセスの終了、前試行の生存確認とEngine実体の再照合を検証 | 未実施（Adapter未実装） |

bind先の所有者がImageのUID/GIDへ変わりアクセス不能なら、全員書込みへ緩めたり既存データを自動chownしたりしない。管理ルートと所有証拠の保護を維持し、操作を停止して新規作成時のnamed volumeを案内する。rootful Dockerの権限がある利用者はホストに対して強い権限を持つため、0600はDocker管理者からの秘匿を保証しない。

本リポジトリのCIはUbuntu 24.04を使用しており、26.04でのGUI・Docker実機成功を代替しない。実機で検証したらOS、Docker、WebKitGTKの版、利用者UID/GID、実際の観測結果をこの表またはPRへ追記する。
