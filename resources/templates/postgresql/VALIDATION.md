# PostgreSQL 17/18の実Docker受入（Issue #46）

`template.yaml`と`versions/17.yaml`・`versions/18.yaml`を一つのSchema 1パッケージとして登録する。Versionごとの完全定義は候補の`docs/template-examples/postgresql`と同じで、共通ファイルからの継承・置換を行わない。配布用のIDは`composenest.postgresql`、Template版は`1.0.0`、既定Versionは`18`。秘密の固定defaultは持たない。

## 実測環境・証跡

2026-10-08、GitHub ActionsのUbuntu 24.04.5 LTS x86_64（runner image `20261004.327.1`）で実行した。
Docker CLI/Engine **28.0.4**、Compose **2.38.2**、Rust **1.98.1**、psql **16.15**。

候補定義の検証はcommit `bb255c0204a77a17ab0789c97ac8e5eb4f0e9c28`の[受入ジョブ](https://github.com/wix-diesel/ComposeNest/actions/runs/37707947804/job/113086730356)で4通りとも成功した。最終PRでは同じテストをresourceのパッケージに対して再実行する。通常テストでも候補とのVersion定義の一致と、全Versionの同梱カタログ登録を確認する。

| Version | 実行platform | 保存方式 | マウント先 | 初回作成・TCP接続・Health | 停止起動・再作成後のデータ |
| --- | --- | --- | --- | --- | --- |
| 17 | linux/amd64 | bind | `/var/lib/postgresql/data` | 成功 | 保持 |
| 17 | linux/amd64 | named | `/var/lib/postgresql/data` | 成功 | 保持 |
| 18 | linux/amd64 | bind | `/var/lib/postgresql` | 成功 | 保持 |
| 18 | linux/amd64 | named | `/var/lib/postgresql` | 成功 | 保持 |

検証時に各タグをpullして解決したRepoDigestは次のとおり。生成Composeはタグではなく、このdigestを使い、再起動・再作成で再pullしない。

- 17: `postgres@sha256:2d2b8998d31037bf721cfdf764d76ba74171b4fab3431b7f72c27c56ddbdf9e3`
- 18: `postgres@sha256:74935e72241653ca55e0414067e6d8763aceb8a810eb51b452253ec3dcfc4336`

タグは更新され得る。再検証ログにその回のdigestとEngine・Compose版を出力する。この記録を別digest・別OS・別アーキテクチャでの検証済み証拠として使わない。

## 検証した内容

- 全パッケージを既存パーサー・意味検証へ通し、確定設定から既存の標準Compose生成器を使う。`compose config --quiet`で実行前に構成を検証する。
- `database`・`username`は既定値と異なる値、`password`はドル・引用符・空白・日本語を含む値で、実コンテナの環境変数とHealth argvの全入力参照を確認する。秘密をinspect/config出力や失敗メッセージへ流さない。
- `127.0.0.1`の公開TCPポートへホストのpsqlで接続する。初期DB・ユーザー、正しいパスワードの成功、パスワード未入力と誤入力の拒否を確認する。ローカルUnixソケットのtrust成功では代用しない。
- 実際のイメージID・OS/architectureと全`Mounts`を照合する。唯一の書込み可能マウントが宣言したbindのSourceまたはnamed volumeのNameに一致し、匿名・余分なマウントがないことを確認する。
- `SHOW data_directory`が宣言した保存slot内にあることとサーバーmajor版を確認する。テーブルと日本語の行を作り、停止起動でコンテナIDが維持され、強制再作成でIDが変わることと、両段階でHealth・認証・Mounts・行データが保持されることを確認する。
- テスト資源だけに一意なproject・volume・一時ディレクトリを使用する。cleanupだけがテストvolumeを除去する。bindはImageのUIDによる初期化をそのまま検証し、全員書込み権限を使った起動回避は行わない。

## 未検証の範囲

WindowsのDocker Desktop、macOSのDocker Desktop、Ubuntu **26.04**の実機、`linux/arm64`の実行は未検証。`platforms: [linux/amd64, linux/arm64]`はコンテナの対応宣言であり、ホストOSの検証済み表示ではない。Template一覧の説明にも検証環境と未検証OSを記載する。製品配布の前に各対象OSで下記テストを実行し、OS/Desktop/Engine/Compose版、digest、4通りの結果を追記する。

このCIのDocker版は設計上の候補基準29.8.1/5.5.1より古い。基準版Dockerでの配布受入、DBアップグレード、データClone、アプリGUI全経路の受入を完了したことは意味しない。

## 再実行

ローカルEngine、Docker Compose、PATH上のpsqlが必要。psqlの場所は`COMPOSENEST_TEST_PSQL`でも指定できる。

```text
cargo test -p composenest-adapters --test postgresql_template --locked
cargo test -p composenest-adapters --test postgresql_template --locked docker_postgresql_versions_preserve_authenticated_data_in_both_storage_modes -- --ignored --exact --nocapture
```

通常テストはDockerに接続しない。CIの`PostgreSQL 17/18 bind and named acceptance`ジョブだけが上のignoredテストを明示的に実行する。
