# UIモックに基づくv1実装タスク

更新日: 2026-09-22

ユーザー指定により、[UIモック](ui-mockups/README.md)の見た目・画面構成・操作導線を本体実装の基準とする。React／Tauri実装は未着手。HTMLをそのまま本体へ組み込むのではなく、共通UIとApplicationClientを通じて実データ・実操作へ接続する。

業務ルール・資源の所有・秘密・復旧条件は[要件](requirements-v1.md)、[ドメインモデル](domain-model.md)、[アーキテクチャ](architecture-v1.md)に従う。モックの固定値、疑似成功、未描画の例外状態を理由に安全条件を省略しない。

## タスク構成

[全体管理 #56](https://github.com/wix-diesel/ComposeNest/issues/56)に集約する。従来の55タスクへ12タスクを追加し、計67タスク。既存18タスクと全体管理を更新した。番号順ではなく、各Issueの依存関係に従って着手する。

| 対象 | 主な実装・受入タスク | 境界 |
| --- | --- | --- |
| モックの登録と本対応表 | [#57](https://github.com/wix-diesel/ComposeNest/issues/57) | 設計資料の登録 #1 と別PR |
| sidebar・topbar・パンくず・通知・dialog枠 | [#37](https://github.com/wix-diesel/ComposeNest/issues/37) | 実画面遷移と対象IDを扱い、各画面の中身は分離 |
| ライト／ダークと表示設定 | [#58](https://github.com/wix-diesel/ComposeNest/issues/58) | 両テーマの共通スタイルと非機密の表示設定 |
| [instances.html](ui-mockups/instances.html) | [#59 カード・検索](https://github.com/wix-diesel/ComposeNest/issues/59)、[#60 DataGrid](https://github.com/wix-diesel/ComposeNest/issues/60) | 同じ照会データから集計・フィルター・ソートする |
| [templates.html](ui-mockups/templates.html) | [#61](https://github.com/wix-diesel/ComposeNest/issues/61) | 出所、検証済みVersion、定義別エラー、再読込み、作成導線 |
| [instance-create.html](ui-mockups/instance-create.html) | [#38 共通フォーム](https://github.com/wix-diesel/ComposeNest/issues/38)、[#39 確認・確定](https://github.com/wix-diesel/ComposeNest/issues/39) | Templateから汎用生成。受付後は独立した進捗画面へ |
| [instance-detail.html](ui-mockups/instance-detail.html) | [#62 詳細・タブ](https://github.com/wix-diesel/ComposeNest/issues/62)、[#41 操作](https://github.com/wix-diesel/ComposeNest/issues/41)、[#44 秘密・Compose](https://github.com/wix-diesel/ComposeNest/issues/44)、[#45 ログ](https://github.com/wix-diesel/ComposeNest/issues/45)、[#67 削除](https://github.com/wix-diesel/ComposeNest/issues/67) | 各操作を別PRで接続。未実装操作を成功表示しない |
| [instance-clone.html](ui-mockups/instance-clone.html) | [#40](https://github.com/wix-diesel/ComposeNest/issues/40) | 差分、由来、秘密引継ぎ、元変更後の再確認 |
| [instance-edit.html](ui-mockups/instance-edit.html) | [#41 名前変更](https://github.com/wix-diesel/ComposeNest/issues/41)、[#42 ポート編集](https://github.com/wix-diesel/ComposeNest/issues/42) | 名前とポートの別要求・結果を区別。ポート適用後も停止 |
| [operation.html](ui-mockups/operation.html) | [#66 進捗](https://github.com/wix-diesel/ComposeNest/issues/66)、[#43 復旧選択](https://github.com/wix-diesel/ComposeNest/issues/43) | 実Operationの段階を表示し、復旧判断はCoreから取得 |
| [diagnostics.html](ui-mockups/diagnostics.html) | [#63](https://github.com/wix-diesel/ComposeNest/issues/63) | 実CLI・Compose・Engine・platform・管理ルート検査 |
| [retained.html](ui-mockups/retained.html) | [#65](https://github.com/wix-diesel/ComposeNest/issues/65) | 実保存領域と元設定の所在、存在・所有状態の再確認 |
| [settings.html](ui-mockups/settings.html) | [#64](https://github.com/wix-diesel/ComposeNest/issues/64) | 保存方式の永続化、実管理ルート、平文保存・手動更新の案内 |
| 全10画面と確認dialog | [#68](https://github.com/wix-diesel/ComposeNest/issues/68) | 両テーマ・通常幅／狭幅・キーボード・例外状態の検証 |

## 共通の実装条件

- assets/styles.css・appearance.cssの配色、余白、文字階層、カード、フォーム、表、レスポンシブを再現する。DataGridという名称だけを理由に特定の大型ライブラリを導入しない。
- assets/mock.js等は操作意図の参考とする。定数をDOMへ挿入するモック実装を流用して、外部Template文言やログをHTMLとして描画しない。
- モックのナビゲーション通知を、対象ID・operationIdを伴う実画面遷移に置き換える。作成／Cloneは確定後に進捗へ、完了後に対象の詳細へ進む。
- 表示設定として保存できるのはテーマとカード／Gridの選択。保存不能時にも画面内で変更できる。秘密、Plan、業務設定の正本をlocalStorageへ移さない。
- カードとGridで検索・状態フィルターを共有し、件数・並び替え・切替状態を一致させる。状態を色だけで表さず、タブ・表・dialogのキーボード操作とフォーカスを扱う。
- 読込み中、全件なし、検索結果なし、取得失敗、Unknown、Missing、Unverified、Stale、確認待ち等を補う。クリックやタイマーだけで利用可能・保存成功と表示しない。

## モックの表示例と本番仕様の差異

| モックの表示・動作 | 本番で守る条件 |
| --- | --- |
| 短い環境IDや固定のcn_4d82_data | Coreの128bit ID、Project名、slot別割当て実名を表示する |
| instances/cn-7f2a/data等のサンプルパス | bindは管理ルートのdata/<instance-id>/<slot>、生成物はinstances/<instance-id>/artifacts/<artifact-id>から実値を解決する |
| ローカルTemplate追加先の概略表示 | 実管理ルートのtemplates/localを案内する |
| postgres:18や短縮Compose | 実Artifactは記録済みdigest、long syntax、所有ラベル等の生成契約を守る。抜粋を生成器へ転用しない |
| 開始／停止の即時badge変更、成功・失敗切替ボタン | 実体とHealthの観測で表示し、デモ切替ボタンは本番へ持ち込まない |
| 設定が再読込みで戻る | 保存方式はSQLiteへ永続化し、保存失敗を表示する。既存環境へ適用しない |
| 名前・ポートをまとめた変更確認 | RenameとEditPortの前提・要求・結果を区別する。部分的な成功を一括成功と表示せず、後続要求は新しい版で再検証する |
| Clone画面の固定候補 | 現Plan版から差分を生成し、Version変更・ask・秘密引継ぎ・元更新に応じて必要な操作と再確認を補う |
| 保持データの「保持済み」 | Missing・Unverified・NotMaterializedも実結果に応じて表示する。復元・完全削除は追加しない |
| 接続時刻・OS・Docker版・Version一覧 | 実診断とTemplateカタログの値を表示し、サンプルを検証済み環境と扱わない |

## Core・受入への反映

- #4: ApplicationClientによる遷移対象と購読の契約。UI表示設定と業務状態を分離する。
- #25: 作成／Cloneの実フォーム・確認事項と、保存方式設定の永続化結果。
- #32: 一覧・詳細に必要な状態軸、サービス、Version、保存方式、最終観測の照会。
- #35: 保持領域の元環境・削除日・所在・存在／所有状態を照会する。
- #49: モックのDOM操作、サンプル秘密、疑似成功が本番の信頼境界へ持ち込まれないことを検証する。
- #50: #68完了後、全10画面と新しい一覧・テーマ操作を含めて受入する。
- #54・#55: 完成した実画面で文書と配布物を確認し、表示設定の復元も検証する。

## PRの進め方

設計資料 #1 とモック登録 #57 は別PRにする。後者は前者のブランチをbaseとし、差分へ設計資料の再追加を含めない。設計資料PRのmerge後はモックPRのbaseをmainへ切り替える。

実装は原則1 Issue＝1 PR。手書きコード＋テスト200〜400行を目安に、600行を超える見込み、または独立した責務が増える場合は実装前にさらに分割する。既存モックの登録は参照資料として別集計し、機能の実装完了とは扱わない。
