# TemplateのVersion別ファイル設計

作成・採用日: 2026-09-22\
状態: 採用済み。未実装の従来案を置き換え、分割形式をSchema 1の標準とする\
対象: Templateの記述・読込み・版管理・フォーム・Snapshot

構文と検証の正本は[Template Schema仕様](template-schema-spec.md)、保存と責務分担の正本は[アーキテクチャ](architecture-v1.md)とする。本書は方式の選択理由と実装タスクへの対応を記録する。仕様の採用はパーサーやアプリの実装完了を意味しない。

## 1. 採用する構成

1つのTemplateを、共通メタ情報・Version一覧を持つマニフェストと、Versionごとの完全な定義ファイルで構成する。

```text
postgresql/
  template.yaml
  versions/
    17.yaml
    18.yaml
redis/
  template.yaml
  versions/
    8.2.yaml
```

同じサービス系列をTemplate IDでまとめ、実際のImage参照は各Versionへ置く。稼働中コンテナの共有やImage更新を行う機能ではない。Versionが1つの場合も同じ構成にする。

- マニフェスト: Schema版、Template ID・定義版、名前・説明・作者等、既定Version、Version→相対ファイルパス、Version／保存方式のClone Policy。
- Versionファイル: Image、platforms、inputs、service、connections。そのVersionの最終設定を単独で読める。
- 継承・マージ・共通設定ファイルを導入しない。項目追加は対象Versionへの追記、削除は対象Versionからの除去で表す。
- 同じ安定入力キーの型は全Versionで一致させる。意味が変わる場合も別キーを使う。

完全な例は[PostgreSQL](template-examples/postgresql/template.yaml)と[Redis](template-examples/redis/template.yaml)を参照する。両例ともschemaVersionは1、templateVersionは1.0.0である。未配布・未実装段階の例の改訂なので、形式変更だけを理由に番号を上げない。

## 2. 選択理由

従来案でもinputs・connections全置換とservice直下キー単位の置換によって項目の増減は表現できた。ただしVersionが増えると1ファイルが長くなり、共通部分と差分を照合する負担、共通設定変更の波及、削除時の記述量が問題になる。

| 方式 | 評価 |
| --- | --- |
| 単一ファイル＋置換 | 配布は簡単だが、行数と共通変更の波及という課題が残る |
| 共通設定＋Version別差分ファイル | 重複は減るが、削除・継承・参照解決が必要で最終形が読みにくい |
| マニフェスト＋Version別完全定義 | 採用。重複を許容し、各Versionの設定と変更範囲を明確にする |
| Versionごとに別Template ID | 一覧とCloneのサービス系列が分散する |
| Versionごとに独立した定義版 | 組合せ固定と独立配布の仕組みが増える。今回の課題には不要 |

同じ変更を複数Versionへ反映するときは各ファイルを明示的に編集する。重複削減のためのinclude・extends・削除用null・条件式は追加しない。

## 3. 登録とSnapshotの単位

ファイルは分割するが、TemplateRevisionはパッケージ全体を1つの不変版として扱う。1 Versionの修正でも登録済みパッケージのtemplateVersionを上げる。ファイル単位のrevisionは持たない。

全Versionを検証し、1件でも不正・欠損なら全体の新規登録を拒否する。他Template・既存版・Instanceは維持する。固定した原文集合で検証・確認・登録を行い、読込み中の変更検知、パス・リンク・サイズ制約はSchema仕様11節に従う。

正規化方式はtemplate-normalization-v1を使う。意味のhashはメタ情報・全Versionの完全定義・表示順・展開済みPolicyを含み、コメント・空白・参照ファイル名は除く。原文集合は相対パス・バイト列・各SHA-256を別保存し、同内容の再登録で既存原文を上書きしない。

Snapshotは選択Versionだけでなく、その版の全Versionの完全定義と原文集合を保持する。カタログ更新で新Versionを自動追加せず、元ファイル削除後もSnapshot内のVersionからCloneできる。

## 4. フォームとClone

選択Versionの定義だけからフォームとComposeを生成する。Version切替で同じキー・型の候補を保持して新しい制約で検証し、削除項目は確定Spec・送信対象から除外する。追加項目は新規作成なら初期化規則、Cloneなら既存Policyに従う。

Cloneの元値のないcopyは入力待ちとし、Defaultで補完しない。秘密の不適合を無言で再生成せず、入力・明示再生成を求める。項目・slotの増減を差分表示し、計画版を進めて確認を無効にする。データ移行は行わず、Clone先の全保存領域は新規に割り当てる。

## 5. Schema番号と実装範囲

現行案が未実装であるため、本形式をSchema 1の標準へ直接反映する。Schema 2、旧単一ファイル形式の互換読込み、変換ツール、旧形式からのDB migrationは追加しない。旧.template.yamlを別の有効形式として残さず、入口はパッケージのtemplate.yamlだけとする。将来、配布済み形式を変更する場合のSchema版管理・Snapshot保護は維持する。

## 6. 実装タスクとの対応

| Issue | 反映する責務・受入条件 |
| --- | --- |
| [#11](https://github.com/wix-diesel/ComposeNest/issues/11) | revision／Snapshotの全Version、原文集合、各hashを保存し、カタログ削除から独立させる |
| [#13](https://github.com/wix-diesel/ComposeNest/issues/13) | マニフェストとVersion文書の型・未知キー・文書単体の構文上限 |
| [#14](https://github.com/wix-diesel/ComposeNest/issues/14) | 完全定義の全Version意味検証。型・参照・Policy・slotの整合、継承なし |
| [#15](https://github.com/wix-diesel/ComposeNest/issues/15) | 安全なパッケージ受付、原文集合固定、全体登録、出所・意味hash |
| [#23](https://github.com/wix-diesel/ComposeNest/issues/23) | Snapshot内の選択Versionから生成。外部ファイル再読込みなし |
| [#25](https://github.com/wix-diesel/ComposeNest/issues/25)・[#27](https://github.com/wix-diesel/ComposeNest/issues/27) | 新規／CloneのVersion切替、追加・削除項目、再検証・再確認 |
| [#38](https://github.com/wix-diesel/ComposeNest/issues/38)・[#40](https://github.com/wix-diesel/ComposeNest/issues/40)・[#61](https://github.com/wix-diesel/ComposeNest/issues/61) | Versionごとのフォーム、Clone差分、パッケージ単位の一覧・読込み結果 |
| [#46](https://github.com/wix-diesel/ComposeNest/issues/46)・[#47](https://github.com/wix-diesel/ComposeNest/issues/47) | 分割したPostgreSQL／Redisの全Versionと両保存方式の実機検証 |
| [#49](https://github.com/wix-diesel/ComposeNest/issues/49) | TS-T01〜36の追跡。パス・差替え・欠損・全体登録・Snapshotの適合性 |
| [#54](https://github.com/wix-diesel/ComposeNest/issues/54)・[#56](https://github.com/wix-diesel/ComposeNest/issues/56) | パッケージ作成・追加手順とタスク全体の対応 |

各Issueの担当範囲を維持し、独立した責務でPRが大きくなる場合は既存のタスク分割規則に従う。機械検証用Schema、パーサー、DB migration、フォーム、OS別のファイル実体検査、Docker実機検証は実装・検証タスクとして残る。
