# 全10画面のモック比較・アクセシビリティ検証（#68）

基準は `ui-mockups/` の全10HTML、`assets/styles.css`、`assets/appearance.css`。検証対象はReactの実画面。モックのDOMを本体へ挿入せず、ApplicationClientのIPC境界だけを検証用応答に置き換える。作成・複製フォームは既存のRust exampleで生成する実CoreのDTOを使用する。

## 再現と証跡

リポジトリルートで実Coreのフォームを生成し、フロントエンドディレクトリで実行する。

```sh
mkdir -p apps/desktop/test-results
cargo run -q -p composenest-adapters --example template_form_fixture > apps/desktop/test-results/template-form-plans.json
cd apps/desktop
pnpm run build
pnpm exec playwright install chromium
pnpm run test:ui
```

全10画面×ライト／ダーク×1280／390pxの40組に、DataGridの4組を加える。確認ダイアログ5種類（作成・複製・削除・名前変更・ポート変更）、詳細タブ、一覧の空／読込中／失敗も同じ4条件で記録する。

- `test-results/ui-validation/index.html`: HTMLモック／Reactの対比一覧と追加状態へのリンク。
- `*-mock.png` / `*-react.png`: 各画面・確認ダイアログの全体画像。
- `metrics.json`: sidebar・workspace・見出し・主要surfaceの配色、文字、余白、境界、領域の位置・寸法と見出し一覧。
- `*-aria.yml`: Reactの主要領域のARIA snapshot。
- `metadata.json`: 実行日時・ブラウザ版・比較件数。
- CIの `common-shell-screenshots` artifactに既存の画面別テストの証跡と一緒に保存する。途中失敗でも取得済みの比較を残す。

自動判定は共通枠の配色・余白・文字階層・surface形状、ページ全体の横はみ出し、操作の意味を組み合わせる。主要領域の配置・読み順・カード／フォーム／表の可読性は対比画像と領域情報でレビューする。サンプル値・件数・時刻の一致や固定ピクセル差分を単独の合否条件にしない。長い実ID・パス、Template依存の入力数、必要な安全説明による高さの差を許容する。

2026-10-07の検証では44組すべてと確認dialog20組が成功した。通常幅は一覧カード3列、Templateカード2列、フォーム／処理／診断／設定の主領域＋補助領域を確認し、狭幅はカードと補助領域の縦並び、表内スクロール、ページ横はみ出しなしを確認した。モックの狭幅Clone表自体のはみ出しを実装の許可条件にはしない。Reactのフォーム・処理・診断の列幅比には差があるが、主要領域の並びと読込順は維持される。全画面共通の「閉じても環境は停止しない」案内、topbarのテーマ操作、フォーカス対象の見出し枠は実装側の補足であり、配置の下方移動を生む。

検出したTemplateカードの角丸を基準の10pxへ修正した。作成／複製では非同期レビューが起点ボタンを無効にし、dialogを閉じた後にフォーカスが戻らない不具合を修正した。新しい検証はレビュー応答を遅延させ、起点復帰、フォーカス表示、Tab制限とキャンセル時の変更要求なしを両テーマ／両幅で確認する。
狭幅のClone差分表と保持データ表には、名前付きregion・Tabでのフォーカス・フォーカス枠を追加した。両テーマでArrowRightによる横スクロールを検証し、ボタンがない列にもキーボードで到達できるようにする。

## 画面対応と安全条件による補足

| モック | 主な比較対象 | Reactで補う状態・導線 |
| --- | --- | --- |
| instances.html | 集計・検索・状態絞り込み・カード／Grid・確認時刻 | Unknownを確認必要に分類、空／検索結果なし／読込中／失敗。実状態の同一snapshotから集計し、更新は保存値の再取得と明示 |
| templates.html | Templateカード・出所・Version・再読込み・追加先 | 定義別エラー／警告、既存登録版、実機検証未確認を区別。実登録版IDを作成へ渡す |
| instance-create.html | 手順・設定フォーム・保存方式・右側の概要・確認dialog | Template別入力、Plan版／ポート再確認、平文保存の明示同意、受付不明時の照会。確認dialogに同意checkboxを追加 |
| instance-detail.html | 状態・概要／ログ／Composeタブ・接続情報・保存先・環境情報・削除 | 実行／処理／構成の3軸、未確認／過去観測、秘密マスク。#44／#45の未接続箇所は下表に記録 |
| instance-clone.html | 設定差分・用途名・保存方式・複製元・確認dialog | 入力の由来・追加／削除・秘密引継ぎ・元の更新を表示。データ非複製と平文保存の同意を追加 |
| instance-edit.html | 変更設定・作成時設定・現在状態・予約・確認dialog | 名前とポートを別要求・別dialogへ分離。ポートは新しい停止観測が必要、適用後も停止、旧新予約を保持 |
| operation.html | 進捗・段階・結果・対象環境・閉じる導線 | 保存されたOperation、通知欠落時の再照会、失敗／結果不明／判断待ちを区別。復旧選択の未接続箇所は下表に記録 |
| diagnostics.html | 利用可否の案内・診断行・接続情報・再確認 | CLI／Compose／Engine／platform／rootと対象不一致を区別。診断失敗を停止や利用可能と扱わない |
| retained.html | 保持領域の表・元設定／Composeの所在・Docker管理の説明 | Present／Missing／Unverified／NotMaterialized、所有未確認、部分再確認失敗。未確認領域を保持済みと断定しない |
| settings.html | デザイン・既定保存方式・実管理ルート・認証情報の方針 | 保存完了／失敗、平文保存、手動更新の案内。保存方式は既存環境へ適用しない |

## アクセシビリティ・例外状態

| 確認内容 | 検証 |
| --- | --- |
| キーボード・フォーカス表示 | `test:shell`のskip link・画面遷移、`test:list`の表内スクロール、`test:create`／`test:clone`の入力。`test:ui`のEnter／Space操作 |
| dialogの安全な初期フォーカス・Tab制限・Escape・起点復帰 | `test:ui`で5種類×4条件。キャンセルで変更要求を送らないことも確認 |
| タブの選択・読み順 | `test:ui`のArrowRight／ArrowLeft／Home／End、aria-selected・フォーカス・表示tabpanel。画面ごとの詳細は`test:instances` |
| ソートと色以外の状態表示 | `test:ui`／`test:list`のaria-sort、pressed状態。badgeの状態名、進捗の段階・記号・結果、保持データの存在／所有状態 |
| 空／読込中／取得失敗／Unknown | `test:ui`で4条件。`test:templates`／`test:retained`／`test:diagnostics`／`test:settings`でも取得失敗・再試行・未確認状態を確認 |
| 秘密マスクと受付不明 | `test:ui`で詳細／編集／保持／処理の秘密値非表示。`test:appearance`で保存対象制限、`test:create`／`test:clone`／`test:delete`／`test:instances`で受付不明・再確認 |

## 機能の接続後に再検証する差異

| 担当Issue | 現在の状態と残る検証 |
| --- | --- |
| #43 | 失敗／結果不明の状態表示は存在する。Coreの復旧判断による再試行・安全終了・旧port復帰・外部編集退避の選択と確認dialogは未接続 |
| #44 | 通常表示はマスクし、秘密表示・コピーとCompose原文表示は利用不可と明示。接続後に明示表示／再マスク／コピー失敗、Artifact原文の比較が必要 |
| #45 | ログタブは未接続と明示し、疑似ログを描画しない。接続後に購読解除、欠落表示、上限、chunk境界マスクと両テーマのログ可読性を検証する |

この検証はブラウザ上のUI・IPC契約の検証であり、Docker実機とWindows／macOS／LinuxのWebView受入を代替しない。上の未接続機能を含む完全なモック再現・#68全条件の最終受入は、各Issue完了後に再実行する。
