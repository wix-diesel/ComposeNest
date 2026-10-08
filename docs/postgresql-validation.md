# PostgreSQL Templateの検証記録

Issue #46の受入対象、実測環境、PostgreSQL 17/18 × bind/namedの結果、digest、再実行手順、未検証OSは、配布前の候補パッケージと一緒に保持する。

[PostgreSQL 17/18の実Docker受入](template-candidates/postgresql/VALIDATION.md)を参照。

仕様例の`docs/template-examples/postgresql`は構文参照用。実Dockerテストの入口は`docs/template-candidates/postgresql/template.yaml`で、マニフェストと全Versionを一つのパッケージとして読む。この候補はTauri resourceにも同梱カタログにも含めない。

Windows・macOS・Ubuntu 26.04での両保存方式の実機受入が残るため、Issue #46の配布resource登録は未完了。対象環境の結果を記録した後に配布へ昇格する。Ubuntu 24.04 CIの成功や説明文での未検証表記だけでは昇格しない。
