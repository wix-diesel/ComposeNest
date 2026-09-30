# データを保持するDeleteと保持領域照会（Issue #35）

## 公開するCore / Adapter境界

- `DeleteOperation::run` / `delete_stages::run_delete` は、現在のInstance版とデータ保持確認を検証し、既存のOperation journalと排他を通してDeleteを受け付ける。未解決Operationがある場合は受け付けない。
- 受付時にRetiringを記録する。所有確認・ネットワーク接続確認・各外部効果のjournal・不在観測・保存領域照合を経て、Retired、Retained、成果物の保持、予約解放、最新の不在観測、完了日時を一括確定する。Engine不通・所有不明・結果不明ではRetiringと予約を維持する。
- `RetainedStore::list_retained_storage` はDockerを使わず、元環境名、サービス名、Version、削除完了日時、マスキング済み設定、slotごとの所在・存在/所有状態・確認日時、設定DBとSnapshotのキー、成果物の参照を返す。
- `delete_stages::refresh_retained_storage` は新しい読み取り観測で保存状態を更新する。bindのみならDocker不通でも確認できる。volumeでEngineに接続できない場合はUnverifiedとする。NotMaterialized、Missing、UnverifiedをPresentと扱わない。
- 同じ要求の再送は同じreceiptを返す。再試行は既存の復旧境界で前CLIの終了・実体を照合した後、同じOperationのattemptを進める。未照合のjournal段階がある場合は再実行しない。

コンテナは完全ID、Compose project/service、独立したscope/instanceラベルで所有を確認する。専用networkには生成Composeでscope/instanceラベルを付け、project/networkラベル・実名・完全IDも検証する。削除前に管理外の接続を拒否し、除去直前にも空であることを確認する。所有証拠のない既存networkは自動採用しない。

アプリのDelete経路は`container stop --time 30 <id>`、`container rm <id>`、`network rm <id>`のみを使う。Composeのdown、volume削除、`rm -v`、prune、強制除去を使わず、元設定・Compose・データを移動しない。保持一覧は所在を示す参照APIで、任意パス開放API・画面・Retired復活・データ完全削除を追加しない。

## Windows実機検証（2026-09-30）

Windows x86_64、Docker CLI/Engine 29.5.3、Compose 5.1.4、Rust 1.98.1、Node.js 24.19.0で実行した。Dockerの版は設計上の受入基準29.8.1/5.5.1より古く、対象版すべての配布受入を完了したことは意味しない。

`docker_delete_preserves_bind_and_volume_data_and_rejects_foreign_network_endpoints` は既存のローカル`alpine:latest`をpullせず使用し、一意なInstance/projectのテスト資源で次を確認した。

- bindとnamed volumeに書いた`sentinel`の内容がDelete後も残る。
- 管理外コンテナが専用networkに接続すると、管理コンテナを停止せずDeleteを拒否する。テスト側でその接続を解消し、同じOperationを再試行すると完了する。
- コンテナと専用networkだけを除去し、元Composeのバイト列と設定、Snapshot、成果物の所在を保持する。
- 保持一覧の再確認でPresentと所有確認を得る。削除要求を再送しても同じreceiptを返す。

実機テストで既存named-volume Adapterのinspect投影に余分な閉じ括弧があり、JSON解析が失敗することが判明したため修正した。テスト終了時のfixture清掃だけが、テストで作成したvolumeを除去する。アプリのDelete経路は除去しない。

通常テストでEngine不通時のRetiring/予約維持、所有不一致、外部効果の結果不明、未解決操作の拒否、要求再送、全slot照合、台帳のロールバック、秘密のマスキング、scope分離、Missing/Unverified/NotMaterializedとbindの非再作成を確認した。

```powershell
$env:COMPOSENEST_TEST_DOCKER = (Get-Command docker).Source
$env:COMPOSENEST_TEST_DOCKER_CONFIG = Join-Path $env:USERPROFILE '.docker'
cargo test -p composenest-adapters --test docker_delete --locked -- --ignored
```

`cargo test --workspace --locked`、`cargo fmt --all -- --check`、`cargo clippy --workspace --all-targets --locked -- -D warnings`、`pnpm --dir apps/desktop run check:contracts`、`pnpm --dir apps/desktop run build`が成功した。Docker結合テストは通常テストから明示的に分離している。macOS/Ubuntuの実機検証と基準版Dockerでの配布受入は未実施。
