# Redis 8.2の実Docker受入（Issue #47）

`template.yaml`と`versions/8.2.yaml`を一つのSchema 1候補パッケージとして検証する。1 Versionでもパッケージ形式を省略しない。完全定義は`docs/template-examples/redis/versions/8.2.yaml`と同じで、固定の秘密default、共通ファイルからの継承・置換は持たない。候補IDは`composenest.redis`、Template版は`1.0.0`、既定Versionは`8.2`。

対象OSでの配布受入は未完了。この候補は`docs/template-candidates`に置き、`resources/templates`・Tauri resource・同梱カタログへは登録しない。説明文の未検証表記は実行制限の代わりにならない。各対象OSでの受入を完了してから配布へ昇格する。Issue #47全体を完了した記録ではない。

## 実測環境・証跡

2026-10-08、GitHub ActionsのUbuntu **24.04.5 LTS x86_64**（runner image `20260927.320.1`）で実行した。Docker CLI/Engine **28.0.4**、Compose **2.38.2**、Rust **1.98.1**、ホストredis-cli **7.0.15**。

commit `c55f690e56e5f651b70df7693bab2c07db73de1e`の[Redis受入ジョブ](https://github.com/wix-diesel/ComposeNest/actions/runs/37713872152/job/113105772695)で両保存方式とも成功した。分割YAMLの構文確認ではなく、下記の全項目を同じ完全定義から生成したComposeで実行した。

| Version | 実行platform | 保存方式 | マウント先 | TCP認証・Health・秘密一致・AOF | 停止起動・再作成後のデータ |
| --- | --- | --- | --- | --- | --- |
| 8.2 | linux/amd64 | bind | `/data` | 成功 | AOFから復元・保持 |
| 8.2 | linux/amd64 | named | `/data` | 成功 | AOFから復元・保持 |

検証時に`redis:8.2`をpullして解決したRepoDigestは`redis@sha256:47670742d7924adbcdb404d1288b6327815b23141969c0939b8e5d61f78c2634`。生成Composeはタグではなく、このdigestを使い、停止起動・再作成で再pullしない。タグは更新され得るため、再検証ログにその回のdigestを出力する。

ローカルの作業コンテナはUbuntu 24.04.3 LTS x86_64、Rust 1.98.1で、Docker CLI/Engine/socketがない。Core 3 crateの319テスト（Docker依存等5件はignored）、整形、全targetsのclippy、フロントエンドのIPC契約・型検査・ビルドは成功した。ローカル結果を実Dockerの成功証拠としては扱わない。

CIの`Redis 8.2 bind and named acceptance`はUbuntu 24.04 x86_64でignoredテストを明示的に実行する。ホストOS、Docker CLI/Engine・Compose版、解決したRepoDigestと両保存方式の結果をログに残す。通常テストはDockerへ接続せず、仕様例との完全定義の一致、配布resourceからの除外、明示的なローカル追加の出所、既存の接続表示・秘密取得APIを検証する。

## 検証する内容

- 全パッケージを既存パーサー・意味検証へ通し、確定設定から既存の標準Composeを生成する。`compose config --quiet`で実行前に構成を確認する。フォームやRunnerへRedis専用処理を追加しない。
- ドル・引用符・バックスラッシュ・空白・日本語を含む秘密を確定Specへ保存する。接続表示のhost/port/input slot、通常表示での秘密の非表示、明示的な秘密取得APIを確認し、その取得値を生成ComposeとホストTCP接続に使う。
- 実コンテナの`command`の`--requirepass`と`REDISCLI_AUTH`が同じ秘密であることを確認する。argv・環境変数・inspect/config出力や失敗メッセージへ秘密を出力しない。
- ホストから`127.0.0.1`の公開TCPポートへホストのredis-cliで接続し、正しい秘密によるAUTH/PINGと、認証なし・誤った秘密の拒否を確認する。コンテナ内ソケットで代用せず、秘密はホストCLIの環境変数へ渡す。
- 実Healthがhealthyで、`redis-cli -e ping`が`REDISCLI_AUTH`で成功することと、空・誤った認証環境では同じHealth argvが失敗することを確認する。
- 実行imageのID・OS/architectureをdigest固定の設定と照合する。唯一の書込み可能マウントが`/data`で、bindのSourceまたはnamed volumeのNameに一致し、匿名・余分なマウントがないことを確認する。
- Redis 8.2系と`aof_enabled:1`、`/data/appendonlydir`内のAOF manifestを確認する。RDBのsaveを実行時に無効にし、`dump.rdb`が存在しない状態で日本語データを書き、停止起動と強制再作成の両方でAOFから復元されることを確認する。停止起動はコンテナID維持、再作成はID変更を確認し、各段階で認証・Health・Mounts・AOF・データを再確認する。
- テストだけの一意なproject・volume・一時ディレクトリを使い、終了時にテスト資源を除去する。bindはイメージのUIDによる初期化を検証し、全員書込み権限を使った起動回避はしない。

## 未検証の範囲

WindowsのDocker Desktop、macOSのDocker Desktop、Ubuntu **26.04**の実機、`linux/arm64`の実行は未検証。`platforms: [linux/amd64, linux/arm64]`はコンテナの対応宣言であり、ホストOSの検証済み表示ではない。配布resourceへ移す前に各対象OSで下記テストを実行し、OS/Desktop/Engine/Compose版、digest、bind/namedの結果を追記する。

別digest・別OS・別アーキテクチャの検証証拠としてこの記録を使わない。このCIのDocker版は設計上の候補基準29.8.1/5.5.1より古い。基準版Dockerでの配布受入、Redisの更新、Clone、アプリGUI全経路の受入を完了したことは意味しない。

## 再実行

ローカルDocker EngineとDocker Compose、PATH上のredis-cli、`redis:8.2`のpull権限が必要。redis-cliの場所は`COMPOSENEST_TEST_REDIS_CLI`でも指定できる。

```text
cargo test -p composenest-adapters --test redis_template --locked
cargo test -p composenest-adapters --test redis_template --locked docker_redis_preserves_authenticated_aof_data_in_both_storage_modes -- --ignored --exact --nocapture
```
