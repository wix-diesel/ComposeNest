# Redis Templateの検証記録

Issue #47のSchema 1候補パッケージ、認証とAOFのbind/named受入、実測環境、digest、再実行手順、未検証OSは[Redis 8.2の実Docker受入](template-candidates/redis/VALIDATION.md)に記録する。

仕様例の`docs/template-examples/redis`は構文参照用。実Dockerテストは`docs/template-candidates/redis/template.yaml`と`versions/8.2.yaml`を一つのパッケージとして既存のパーサー・意味検証・Compose生成器へ通す。フォームとRunnerにRedis専用の分岐を追加しない。

対象OSすべてでの両保存方式の実機受入が残るため、Issue #47の配布resource登録は未完了。候補はTauri resource・同梱カタログに含めず、対象環境の実測結果を記録してから昇格する。CI成功や説明文での未検証表記だけでは昇格しない。
