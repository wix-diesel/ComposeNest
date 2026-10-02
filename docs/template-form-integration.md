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

## Reactフォームを画面へ組み込む

`TemplateForm`へ`initialPlan: { kind: "create", view: CreatePlanView }`または`{ kind: "clone", view: ClonePlanView }`と`onUpdate(plan, edit)`を渡す。別Planを開く場合は`key={plan.view.planId}`で再マウントする。callbackはApplicationClient経由で、対応する`PlanEdit` / `CloneEdit`をCoreへ渡し、新しい版の同じ種類・IDのPlanを返す。未知の例外は汎用文言、構造化ErrorDtoはreasonとfieldPathを表示する。

編集中の入力はメモリーだけに保持する。「入力を反映」は差分だけを送り、Version切替はまず旧Versionへ差分を反映し、その応答版でVersion変更を送る。途中失敗時はCoreへ反映できた版を保持し、未反映の入力を消さない。二重送信を防ぎ、Coreが返した初期値・未回答・制約で再描画する。

秘密の表示ボタンはユーザーがこのフォームで入力した値のみを対象とする。Core生成済み秘密は存在だけを表示し、秘密取得APIは呼ばない。秘密はサマリー・URL・localStorageへ出さず、更新成功・Version変更後は表示状態をリセットする。Cloneの秘密引継ぎ確認は#40へ委ねる。

共通フォームは準備済みPlanを受ける部品であり、既存Appの準備中画面は#39/#40でPlan準備・確認・確定と同時に置き換える。テスト専用HTML/DTOは配布bundleへ組み込まない。テーマは`data-theme="dark"`に追従し、切替UIや保存は#58へ委ねる。

ローカルのモデル検証はfixture生成後に`node --experimental-strip-types --test scripts/formModel.test.mjs`、ブラウザ検証は`pnpm run test:forms`。必要なら`COMPOSENEST_TEST_BROWSER`で検証用Chromiumの絶対パスを指定できる。
