# PostgreSQL Templateの検証記録

Issue #46の受入対象、実測環境、PostgreSQL 17/18 × bind/namedの結果、digest、再実行手順、未検証OSは、パッケージと一緒に配布resourceへ保持する。

[PostgreSQL 17/18の実Docker受入](../resources/templates/postgresql/VALIDATION.md)を参照。

仕様例の`docs/template-examples/postgresql`は引き続き候補・構文参照用。同梱パッケージの入口は`resources/templates/postgresql/template.yaml`で、既存のTauri resource設定とTemplateカタログがマニフェストと全Versionを読み込む。
