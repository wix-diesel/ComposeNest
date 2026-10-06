# ComposeNest
Dockerを管理するためのアプリ

- [画面ごとのHTML UIモック](docs/ui-mockups/README.md)（本体実装の参照資料）
- [画面別の実装タスク対応表](docs/ui-implementation-plan.md)

## プロダクト文書

- [企画書](docs/product-proposal.md)
- [v1 要件定義書](docs/requirements-v1.md)
- [v1 ドメインモデル](docs/domain-model.md)
- [v1 Clone Policy仕様](docs/clone-policy-spec.md)
- [v1 Template Schema仕様](docs/template-schema-spec.md)
- [v1 アーキテクチャ設計](docs/architecture-v1.md)
- [TemplateのVersion別ファイル設計（採用済み）](docs/template-version-files-design.md)
- Schema 1パッケージ例: [PostgreSQL](docs/template-examples/postgresql/template.yaml)、[Redis](docs/template-examples/redis/template.yaml)

## 実装タスク

[v1 実装タスク一覧](https://github.com/wix-diesel/ComposeNest/issues/56)で、各タスクの依存関係と受入条件を管理します。原則として1 Issueを1 PRで扱い、レビュー可能な規模へ分割して進めます。

## 開発

[開発基盤の手順と採用版](docs/development-setup.md)を参照してください。

環境詳細はURLの`instanceId`を使い、保存済みの接続設定・保存先・Template版・複製元を表示します。実行状態、直前の処理、構成の適用版、元の観測時刻を分けて表示し、適用版の一致だけで外部構成が一致しているとは扱いません。概要・ログ・Composeタブは矢印キーとHome/Endで切り替えられます。ログ購読・Compose閲覧・秘密情報操作・削除は個別タスクとの接続待ちとして無効化しています。

Docker診断はCLI・Compose・Engineの実際の版、Linuxコンテナ・アーキテクチャ・ローカル接続先、管理ルートの読書き権限を個別に確認します。現在のContext名は参考情報であり、対象一致は保存済み接続先で観測したEngine IDから判定します。診断では対象を登録・変更せず、コンテナを作成・停止しません。接続不能時も「保存済みの環境を見る」から一覧へ戻れます。Docker導入・設定変更後は再確認に加えてアプリを再起動してください。

初回起動時から、アプリを閉じても起動済みコンテナが停止しないことを案内します。診断画面の回帰テストは`pnpm --dir apps/desktop run test:diagnostics`で実行できます。
