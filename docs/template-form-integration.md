# Template共通フォーム

Issue #38の共通入力部分。画面単位のPlan準備・確認・確定は#39/#40、表示設定の切替・永続化は#58で扱う。

## Core契約

`CreatePlanView.templateForm` / `ClonePlanView.templateForm` は同じ形式で、Template名・版・説明、選択Versionのinputs、port/storage slot、connectionsを表示順付きで返す。新規作成は登録済みrevision、Cloneはprivate Snapshotが正本。フロントはYAML探索・Version継承・Default適用をしない。

入力候補は既存の`inputs`から取得する。新規作成は`value` / `hasSecret`、Cloneは`candidate` / `hasSecret` / `needsAnswer`を用い、removed項目は送信しない。未設定はnullで表し、0・false・空文字は候補として保持する。`concerns[].fieldPath`が各入力へ対応する。

`connections`省略時はport slotを標準表示し、明示的な空マップでは追加表示しない。秘密候補、元秘密、割当て済みhost pathは共通DTOに含めない。`canGenerate`はsecret-v1の長さ・pattern条件に適合する項目のみtrueとなる。

## 契約の更新と検証

```sh
cd apps/desktop
pnpm run generate:forms
pnpm run check:contracts
```

`generate-form-contract.mjs`は指定したRust DTOの単純なpublic fieldとSerde命名を読み、未対応の型・属性で失敗する限定的な生成器。新しい型・属性は生成器と検証を同時に更新する。既存bootstrap契約の検査は維持する。

`cargo run -p composenest-adapters --example template_form_fixture`は実Coreによる新規作成・CloneとVersion切替後のマスク済み結果を出力する。ブラウザテスト用で、配布アプリの初期値として使用しない。
