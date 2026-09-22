# ComposeNest Template Schema仕様 v1

作成日: 2026-09-21\
更新日: 2026-09-22\
状態: 採用済み。Schema 1はマニフェスト＋Version別完全定義。実装・実機検証は未完了\
参照: [要件定義](requirements-v1.md)、[ドメインモデル](domain-model.md)、[Clone Policy仕様](clone-policy-spec.md)

## 1. 目的と設計方針

Templateは「何を入力してもらい、その値でどのサービスを起動するか」を宣言するパッケージである。作者がPostgreSQL、Redis等の設定知識を一度定義すれば、利用者は生成された日本語フォームから環境を作れる。

ユーザー作成Templateのインポートを将来提供することを前提に、同梱Templateだけに通じる暗黙の挙動を作らない。作者のためには読みやすいYAML、アプリのためには厳密な検証と版管理を採用する。

優先する原則は次の5つ。

1. **小さく書き始められる。** 名前、対応Version、入力、サービス設定で基本形を構成する。
2. **役割が見える。** マニフェストは共通メタ情報とVersion一覧、各Versionファイルの`inputs`はフォーム、`service`はコンテナを表す。
3. **安全処理を書かせない。** 名前・ポート・保存領域の分離はCoreが担当し、Templateはコンテナ内の待受ポートと保存先を指定する。
4. **文字列をプログラムにしない。** 値の参照は`{ input: password }`のように明示する。シェル式や任意Composeの差込みは使わない。
5. **登録と起動を分ける。** Templateの読込み・インポートではコンテナを起動せず、内容と出所を確認できるようにする。

本仕様は宣言可能な範囲の基礎であり、任意のDockerコンテナを無条件に扱えるとは約束しない。v1は単一主要サービス、複数ポート、複数保存領域、環境変数、起動引数、Health Checkを対象とする。ユーザー作成Templateにも同じ制限を適用する。

本形式を未実装段階のSchema 1の標準とする。従来案の単一ファイル・Version差分形式は受け付けず、互換読込み・移行機能は設けない。判断理由は[Version別ファイル設計](template-version-files-design.md)を参照する。

## 2. まず読む最小例

次の例は、入力項目を持たないRedis環境を作る基本形である。認証なしの例は構文説明用であり、同梱候補は後述の認証付き例を使う。

`redis-basic/template.yaml`:

```yaml
schemaVersion: 1
id: example.redis-basic
templateVersion: "1.0.0"
name: Redis 入門用
description: データを保存する、単一のRedis環境です。

defaultVersion: "8.2"
versions:
  "8.2": versions/8.2.yaml
```

`redis-basic/versions/8.2.yaml`:

```yaml
image: redis:8.2
platforms: [linux/amd64, linux/arm64]

service:
  command: [redis-server, --appendonly, "yes"]
  ports:
    redis:
      label: 接続ポート
      container: 6379
      defaultHost: 6379
  storage:
    data:
      label: Redisデータ
      container: /data
  healthcheck:
    command: [redis-cli, -e, ping]
```

これだけで、アプリは「環境名」「Version」「接続ポート」「保存方式」「保存領域」を表示する。初期保存方式はbind mountで、設定によりnamed volumeを選択できる。Clone時のポート探索と新しい保存領域の割当ては自動適用する。

**作者が書くのはコンテナ内の設定であり、`C:\ProgramData\ComposeNest`やvolume実名は書かない。** ホスト側の保存先は実行環境とInstance IDからCoreが決定する。

完全な定義例:

- [PostgreSQL 17／18](template-examples/postgresql/template.yaml): 環境変数、秘密入力、Version別マウント先。
- [Redis 8.2](template-examples/redis/template.yaml): 引数への入力値参照、認証付きHealth Check、永続化。

例は構文・仕様の参照用である。使用イメージの起動・接続・永続化・両保存方式・各OSでの実機検証が済んだ配布物という意味ではない。

## 3. 全体構造と識別子

1ディレクトリを1 Templateパッケージとする。入口は`template.yaml`、Versionファイルは`versions/<name>.yaml`。各ファイルはUTF-8の単一YAML文書とする。Versionが1つでも同じ構成とし、ファイル名からVersionを推測しない。以下はマニフェストのキーである。

| キー | 必須 | 型 | 意味・省略時 |
| --- | --- | --- | --- |
| `schemaVersion` | 必須 | 整数 | 本仕様は`1`のみ |
| `id` | 必須 | 文字列 | 安定したTemplate ID。例: `example.postgresql` |
| `templateVersion` | 必須 | 文字列 | 定義の版。例: `"1.0.0"` |
| `name` | 必須 | 文字列 | 日本語の表示名 |
| `description` | 必須 | 文字列 | サービスの目的や利用上の説明 |
| `author` | 任意 | 文字列 | 作者の表示名。本人確認済みという意味ではない |
| `homepage` | 任意 | 文字列 | 作者等のHTTPSページ。読込み時に取得しない |
| `license` | 任意 | 文字列 | Template文書のライセンス表記。コンテナイメージのライセンスとは別 |
| `defaultVersion` | 必須 | 文字列 | 初回作成で選ぶ`versions`のキー |
| `versions` | 必須 | マップ | Versionキー→相対ファイルパス。記述順が表示順 |
| `versionClone` | 任意 | 文字列 | `copy`または`ask`。省略時`copy` |
| `storageClone` | 任意 | 文字列 | 保存方式の`copy`または`ask`。省略時`copy` |

`id`は小文字英数字・ハイフンからなる区間をドットで区切り、2区間以上、全体3〜128文字とする。各区間は英小文字で始める。推奨は`作者またはプロジェクト.サービス`であり、`composenest.*`を名乗っても同梱扱いにはならない。

`templateVersion`は非負整数3つをドットで区切った版とし、不要な先頭ゼロを認めない。v1ではプレリリースやビルド識別子を使わない。`schemaVersion`、`templateVersion`、サービスVersion、アプリVersionを混同しない。

入力キー・ポートslot・保存slot・接続情報キーは、小文字英字で始まり、小文字英数字・ハイフンで構成する1〜32文字とする。同じマップ内で重複不可。異なるマップ間で同名は許可し、参照構文で区別する。

表示名は1〜100文字、説明は1〜2,000文字を上限とする。文字数はUnicodeスカラー値で数える。入力値そのものと異なり、表示メタ情報の前後空白は正規化できる。表示文言はプレーンテキストとして扱い、HTML・スクリプトとして描画しない。

## 4. Version別完全定義

### 4.1 マニフェストとVersionファイル

```yaml
defaultVersion: "18"
versions:
  "18": versions/18.yaml
  "17": versions/17.yaml
```

各Versionファイルは次のキーだけを持つ。マニフェストにこれらのキーは置けず、Versionファイルにid・templateVersion・schemaVersion・Versionキー・Clone選択Policyを重複記述しない。単独のVersionファイルは登録できない。

| キー | 必須 | 意味・省略時 |
| --- | --- | --- |
| `image` | 必須 | タグまたはdigest付きの固定Image参照。入力値で補間しない |
| `platforms` | 必須 | `linux/amd64`、`linux/arm64`から選ぶ重複のない配列 |
| `inputs` | 任意 | そのVersionの全入力項目。省略時は空 |
| `service` | 必須 | そのVersionの完全な実行設定。healthcheck必須 |
| `connections` | 任意 | そのVersionの接続表示。省略時は標準表示、空マップは追加カードなし |

Versionキーは1〜64文字の英数字・ドット・ハイフン・アンダースコア。必ず引用して文字列として書き、数値を暗黙変換しない。versionsとplatformsは1件以上とし、defaultVersionは列挙済みキーに一致させる。全Versionを検証し、同じ入力キーの型をVersion間で変えることは禁止する。意味が変わる場合も別キーを使う。

platformはホストOSではなくLinuxコンテナのアーキテクチャを表す。作者の宣言だけで実機検証済みと表示せず、実行対象とImageの対応は実行前に確認する。

### 4.2 継承・マージをしない

各Versionファイルだけで入力と実行設定が完結する。共通inputs／service、Version間参照、include、extends、YAMLアンカー、削除用null、差分マージは認めない。共通する内容も各ファイルへ明示的に記述する。

項目の追加は対象Versionへの追記、削除は対象Versionからの除去で表す。service・connectionsに削除したinputへの参照が残ればパッケージ全体の検証を失敗させる。他Versionの設定を引き継いで不足を補わない。空マップと省略の意味は各キーの規定に従う。

PostgreSQL例は17と18にそれぞれ完全定義を置き、Imageとdataのマウント先を独立して記述する。1つのVersionを修正しても、他Versionの内容は変化しない。ファイル単位の独立revisionは設けず、登録済み定義を変更するときはパッケージ全体のtemplateVersionを上げる。

### 4.3 Version選択と項目の増減

新規作成は選択VersionのDefault・secret初期化規則を使う。切替時は同じ安定キー・型の現在候補を保持して新制約で再検証し、追加項目には新規作成の初期化規則を適用する。不適合な値を無言で変更せず入力待ちにする。削除項目は確定Spec・送信対象から除外し、非表示の旧値を復活させない。

CloneはSnapshot内の全Versionから選ぶ。元値のないcopyは入力待ちとし、Defaultで補わない。秘密候補の不適合は入力・明示再生成を求める。項目・slotの増減を差分表示し、全条件を再検証して計画版を進め、作成確認を無効にする。詳細は[Clone Policyの6節](clone-policy-spec.md#6-項目別の具体的な方針)に従う。

稼働中InstanceのVersion変更やデータ移行は行わない。Clone先の全保存slotは新規領域となる。

## 5. 入力項目とフォーム

### 5.1 基本形

```yaml
inputs:
  database:
    label: データベース名
    type: string
    default: app
    description: この環境で最初に作成するデータベース名です。
    validation:
      minLength: 1
      maxLength: 63
  password:
    label: パスワード
    type: secret
    description: 自動生成できます。v1では端末内に平文保存されます。
```

必要最低限は`label`と`type`である。`required`は省略時true。通常項目のCloneはcopy、secretの初回作成・Cloneはsecret-v1による新規生成が既定となる。

| 入力内キー | 型・値 | 意味 |
| --- | --- | --- |
| `label` | 必須文字列 | 日本語ラベル |
| `type` | 必須列挙値 | `string`、`integer`、`boolean`、`select`、`secret` |
| `description` | 任意文字列 | 項目の補足説明 |
| `required` | 真偽値、既定true | 未設定を許可しない。false、0は設定済みの値 |
| `default` | 型に合う値 | 通常の新規作成用。secretには禁止 |
| `options` | 選択肢配列 | selectのみ必須。各要素は`value`と`label` |
| `validation` | マップ | 型ごとの制約。後述 |
| `clone` | Policy名 | 対応表に従う。未指定時の動作は型で決まる |
| `initial` | secretのみ | `generate`または`ask`。省略時generate。初回作成の動作 |
| `advanced` | 真偽値、既定false | 初期画面では詳細設定へまとめられる |

全入力はフォームで編集可能とする。固定値は入力項目にせず`service`に書く。v1では入力専用のreadOnly・自由HTML・ファイルアップロード・ホストパス入力を設けない。

任意のsecretを未設定で作りたい場合は`required: false`と`initial: ask`を指定する。secretのDefaultへ固定パスワードを書いて配布することは禁止する。

### 5.2 型ごとの表示・値

| type | フォーム | default・入力値 |
| --- | --- | --- |
| string | テキスト欄 | 文字列。空文字の可否はValidationで決める |
| integer | 整数欄 | JSONで安全に表せる整数範囲内。浮動小数は不可 |
| boolean | チェックボックス等 | true／false。未設定を許可する場合は未選択と区別する |
| select | 選択欄 | `options[].value`の文字列 |
| secret | 伏字欄＋表示・生成・コピー | 文字列。初期値・Clone時の扱いを明示する |

選択肢の例:

```yaml
mode:
  label: 動作モード
  type: select
  default: development
  options:
    - value: development
      label: 開発用
    - value: test
      label: テスト用
```

optionsは1件以上で、valueは1〜256文字、labelは1〜100文字の文字列とする。valueの重複は拒否し、defaultがあればoptions内のvalueと一致する必要がある。select以外のoptions、secret以外のinitial、型に不適合なValidationキーは未知の設定として拒否する。

選択Versionの入力マップの記述順をフォーム順として保持する。Policy実行順やポート探索順には使わない。正規化Snapshotには表示順を明示的な配列として保存する。

### 5.3 Validation

| 型 | 許可するValidation |
| --- | --- |
| string／secret | `minLength`、`maxLength`、`pattern` |
| integer | `min`、`max` |
| boolean | 追加のValidationキーなし |
| select | options内のvalueに一致。追加のValidationキーなし |

文字列は省略時0〜4,096文字。secretは省略時1〜4,096文字。minLengthは0以上、maxLengthは1〜4,096、min≤maxとする。必須とは値の存在を意味し、空文字禁止はminLengthで明示する。

整数は−9,007,199,254,740,991〜9,007,199,254,740,991以内で、指定のmin≤maxとする。文字列から整数へSchema側が暗黙変換しない。フォーム入力の変換は別の入力処理である。

`pattern`は全体一致の正規表現として扱う。v1はRustのregexが扱える構文の範囲を採用し、後方参照・先読み・後読みは不可。パターン長は256文字以内、改行を含む入力や制御文字はv1の入力で拒否する。実装の採用版と互換性テストはアーキテクチャ設計で固定する。[regex公式資料](https://docs.rs/regex/latest/regex/)

secret-v1で自動生成する項目にpatternは認めず、長さ条件は32文字を許容しなければならない。独自形式の秘密には`initial: ask`、`clone: ask`等で手入力を用いる。通常の秘密値にtrimやUnicode正規化を適用しない。

Default・選択肢は読込み時にも検証する。入力間の任意条件式や表示条件はv1対象外。参照の整合、ポート・保存領域の重複、型、Version差等の項目間検証はCoreが行う。

## 6. Clone Policyとの対応

書き方は`clone: copy`のようにPolicy名を1つ指定する。構造化した別構文や任意生成器名を同時に用意しない。

| 宣言場所 | 指定可能な値 | 省略時 |
| --- | --- | --- |
| 通常のinputs項目 | copy、clear、ask | copy |
| secret項目 | copy、regenerate、clear、ask | regenerate |
| `versionClone` | copy、ask | copy |
| `storageClone` | copy、ask | copy |
| ポートslot | Policy宣言を置かない | Host PortはnextAvailable固定 |
| 保存slot | Policy宣言を置かない | 保存領域はstorage-v1で新規割当て固定 |
| ID・Project・表示名 | Templateに定義しない | Coreの規則に固定 |

secretのregenerateはsecret-v1、内部IDはidentity-v1、Projectはproject-v1を使う。schemaVersion 1はClone Policy仕様版1とこれらの生成器版に対応する。作者に同じ版番号を何箇所も書かせず、読込み後のSnapshotへ明示的に展開する。

秘密のcopyには明示確認が必要である。clearの後にDefaultを適用しない。必須clearは入力待ちになる。詳細な優先順位・計画・確定・再試行は[Clone Policy仕様](clone-policy-spec.md)を正とする。

## 7. 入力値をサービスへ渡す

### 7.1 直接値と入力参照

`environment`の値または`command`の各要素には、文字列・整数・真偽値の直接値、あるいは入力参照を置ける。

```yaml
environment:
  POSTGRES_DB: { input: database }
  POSTGRES_USER: { input: username }
  POSTGRES_PASSWORD: { input: password }
  TZ: Asia/Tokyo
```

`{ input: database }`は、選択Versionで有効なinputs.databaseの確定値を使う意味。参照オブジェクトに許可するキーは`input`と、環境変数でのみ利用できる`onMissing`だけである。

| 記法 | 意味 |
| --- | --- |
| `hello`、`"123"` | 固定文字列 |
| `123`、`true` | 直接値。出力時は10進整数／小文字true・falseへ変換 |
| `{ input: password }` | 入力値全体を1つの値として参照 |
| `{ input: note, onMissing: omit }` | noteが未設定なら、その環境変数自体を出力しない |

任意項目をenvironmentで参照する場合は`onMissing: omit`を必須とする。必須項目では省略し、未設定なら作成を止める。空文字は未設定ではないためomitしない。

commandやHealth Checkの引数では任意項目を参照できない。未設定の引数だけを落として前後のフラグがずれる挙動を防ぐためである。任意フラグ・動的引数の組立ては将来の拡張対象にする。

### 7.2 文字列補間をしない

`${password}`、`{{ inputs.password }}`、`$(...)`などはComposeNestの参照構文ではない。入力参照の結果も再解釈しない。固定文字列中の`$`は文字であり、生成Composeへ書くときにComposeの変数展開で変化しないように出力する。

文字列の連結、任意パス参照、環境変数の読込み、ファイル内容の読込み、計算式、入力間の参照はv1で提供しない。例えば`--password=値`が必要なサービスは`--password`と値を分離できるかを確認する。分離できない方式は、将来の明示的な連結機能や構成ファイル生成が必要であり、文字列補間の抜け道を作らない。

secretへの参照は機密指定を引き継ぐ。生成されたComposeやコンテナ内の設定に平文が存在し得るが、フォーム・差分・ログの通常表示には実値を出さない。

## 8. serviceの定義

serviceは1つの主要コンテナの実行設定である。Composeの`services`マップを直接貼り付ける場所ではない。

| キー | 型 | 省略時・制約 |
| --- | --- | --- |
| `environment` | 環境変数名→値のマップ | 空。名前は英字または`_`で始まり、続きは英数字・`_` |
| `command` | 引数配列 | Imageの既定commandを使う。指定時は1〜64要素、先頭は空でない固定文字列 |
| `ports` | slotマップ | 空。最大16slot |
| `storage` | slotマップ | 空。最大16slot。永続化なしサービスも表現可能 |
| `healthcheck` | オブジェクト | Version解決後に必須 |
| `restart` | 文字列 | `"no"`、`on-failure`、`always`、`unless-stopped`。省略時`"no"` |
| `memoryLimitMiB` | 整数 | 16〜1,048,576。省略時アプリは上限を生成しない |

memoryLimitMiBは1単位=1,048,576 bytesとしてComposeへ変換する。単位をキーに含め、文字列の単位解析やMBとの混同を避ける。

commandはコンテナ内で実行されるargv配列へ変換する。ホストのシェルへ渡さず、空白で分割し直さない。文字列1本のcommand、CMD-SHELL、entrypoint変更、ホストコマンド実行、作成前後のスクリプトフックはv1では提供しない。

配列形式でも、作者がshellやインタープリターを指定すればコンテナ内でコードを実行できる。またImage自身にも起動処理がある。したがってargv形式はホストへの文字列注入を減らすための契約であり、悪意のあるコンテナの無害化を保証するものではない。出所の確認と実行前の説明を併用する。

### 8.1 ports

```yaml
ports:
  api:
    label: API接続ポート
    container: 9000
    defaultHost: 9000
  console:
    label: 管理画面ポート
    container: 9001
    defaultHost: 9001
```

各slotは`label`、`container`を必須とし、`defaultHost`は任意とする。containerは1〜65535、defaultHostは1024〜65535。Host IPは127.0.0.1、protocolはTCPに固定し、Template側に設定キーを設けない。

defaultHostは新規作成時の提案値であり、Clone時には元の次から探索する。defaultHost未指定なら新規フォームで入力を求める。フォームの値・候補はCoreがPortBindingへ保存する。inputsへ同じHost Portを重複定義しない。

1サービスで同じcontainerポートを複数slotへ登録する定義はv1では拒否する。公開しないコンテナ内ポートは宣言不要。任意ネットワーク、外部サービスへの接続自動化はこのslotに含めない。

### 8.2 storage

```yaml
storage:
  data:
    label: データ保存領域
    container: /data
```

各slotは`label`と`container`が必須。containerはLinuxコンテナ内の絶対パスとする。`/`、`..`要素、NUL、制御文字を禁止し、正規化後の同一パス・親子関係を複数slotに指定しない。ホストパス、volume名、共有、external、保存方式固定は指定不可。

全slotにInstanceが選択したbind mountまたはnamed volumeを適用する。作者は両方式に対応する同じコンテナ内マウント先を書く。永続データの実際の出力先がここに含まれるかは、Image・command・environmentと合わせてレビュー・実機検証する。

Imageが持つVOLUME宣言によって未管理の匿名volumeが生じる可能性を考慮し、取得後の実体検証で未管理の書込みマウントを検出する。TemplateだけでImageの全挙動を静的に保証できるとはしない。想定外のマウントを確認したら利用可能・安全なCloneと判定せず、原因を示す。

保存slotゼロは許可するが、コンテナ再作成で失われるデータがあるサービスには「永続化なし」を表示する。PostgreSQL・Redisの同梱候補ではデータslotを必須にする。

### 8.3 healthcheck

```yaml
healthcheck:
  command: [pg_isready, -U, { input: username }, -d, { input: database }]
  intervalSeconds: 5
  timeoutSeconds: 3
  startPeriodSeconds: 10
  retries: 12
```

| キー | 必須 | 範囲・既定 |
| --- | --- | --- |
| `command` | 必須 | コンテナ内argv、1〜64要素。先頭は固定文字列 |
| `intervalSeconds` | 任意 | 1〜300、既定5 |
| `timeoutSeconds` | 任意 | 1〜300、既定3 |
| `startPeriodSeconds` | 任意 | 0〜600、既定10 |
| `retries` | 任意 | 1〜120、既定12 |

終了コード0を成功、それ以外を失敗としてComposeのexec形式Health Checkへ対応させる。指定したプログラムがImageに存在する必要がある。コンテナの実行中とサービスのReadyを区別する。[Composeサービス仕様](https://docs.docker.com/reference/compose-file/services/)

アプリ側の準備完了待ち期限はHealth Checkの各値とは別である。取得タイムアウトをHealth Check成功へ変換しない。Health Check自身がデータを書き換える可能性があるため、読取り目的の検査を作者ガイドの原則とする。

## 9. 接続情報の表示

```yaml
connections:
  database:
    label: PostgreSQLへの接続
    port: database
    inputs: [database, username, password]
```

各要素は`label`と参照先のport slotを必須とし、`inputs`は表示する入力キーの配列である。Version解決後に有効なport・inputを参照していることを検証する。

ホストと割当て済みHost PortはCoreが表示する。入力のラベルと機密指定を再利用し、Passwordだけをここで通常文字列に変更することはできない。未設定の任意値は未設定と示す。

connections省略時は全公開ポートと入力の標準表示を使う。URLや接続文字列の自由な補間、ブラウザ自動起動、SQL実行ボタンはv1対象外。v2ではlocalhostがブラウザ端末を指す問題を、Coreの接続先解決で扱う。

connectionsが空マップの場合は追加の接続カードを作らない。公開ポートや入力値をInstance詳細で確認する共通機能まで隠すことはできない。inputs配列に重複キーを置いた場合は拒否する。serviceから一度も参照されない入力には「この値はコンテナ設定に反映されません」と作者向けの警告を出す。

## 10. アプリが自動的に決めること

| 項目 | Coreの規則 |
| --- | --- |
| Instance名 | 共通フォームで入力。Templateに重複定義しない |
| Instance ID | identity-v1で生成 |
| Compose Project名 | project-v1で生成 |
| Compose Service Name | v1は`main`で統一。Template種類によらずProject単位で分離 |
| Container Name | 明示指定せずComposeへ委ねる |
| Network | Project専用の既定ネットワーク |
| Host IP／protocol | 127.0.0.1／TCP |
| Host Port | PortBindingと予約台帳で割当て・検証 |
| ホスト保存先・volume名 | storage-v1で全slotを新規割当て |
| 保存方式 | 新規作成の既定はアプリ設定、Cloneは元の方式。ユーザーは作成前に変更可 |
| 管理印・識別情報 | アプリが管理。Template側から上書き不可 |
| 入力値の出力 | 型に応じた文字列化・Compose上のエスケープ。値そのものは変更しない |

raw Compose、`privileged`、Docker socket、`network_mode`、devices、capabilities、任意ホストマウント、任意labels、build、外部env_file、include、extends、Compose secrets定義等の未知キーは拒否する。必要性が出た場合は明示的なSchema拡張として検討し、`extra`等の自由な抜け道を設けない。

## 11. 読込み・検証の段階

Schemaに適合したこと、作者の説明どおり動くこと、信頼できることを分けて判定する。

| 段階 | 確認する内容 | 行ってはいけないこと |
| --- | --- | --- |
| 1. パッケージ受付 | サイズ、UTF-8、マニフェストと列挙Versionファイルの安全な受付 | URL取得、パッケージ外参照、未列挙ファイルの取込み |
| 2. YAML構文 | 重複キー、許可するデータ型、深さ | 不明タグの実行・任意オブジェクト生成 |
| 3. 構造検証 | 必須キー、未知キー、型、値域 | 誤記を無視して起動設定から落とす |
| 4. 意味検証 | 全Versionの解決、参照、Policy、Default、slot重複 | 選択DefaultのVersionだけ確認して他を未検証にする |
| 5. 登録判定 | 同一ID／版、内容、出所と利用者の確認 | 既存定義やInstanceを上書き |
| 6. 作成時検証 | RuntimeTarget、Image、platform、資源、生成Compose | インポート承認だけを起動許可とみなす |
| 7. 起動後検証 | Ready、マウント、接続・永続化の期待 | 未検証の自作Templateを公式検証済みと表示 |

任意のJSON Schema検証だけでは、参照の存在、Versionファイルの対応、同一型、secret-v1互換性、実際のDocker環境まで判定できない。機械検証用Schemaは本書の構造契約を実装する補助であり、意味検証が別途必要となる。本段階ではアプリ用パーサーや検証器の実装には進めない。

### 11.1 YAMLの制限

- YAML 1.2のJSON互換データ型を基準とし、マップキーは文字列のみ。
- 未知キー、重複キー、複数文書、anchors／aliases／merge key、独自タグを拒否する。
- 日時、バイナリ、NaN、Infinity、nullは使わない。日付が必要な説明は文字列にする。
- `true`／`false`以外のyes・no等を真偽値へ変換しない。Versionや文字列の`"no"`は引用を推奨する。
- 文字列入力値・秘密の全角半角変換やtrimをパーサーで実施しない。
- マップ順序はinputs・versions・connections・slotの表示順として別途保持する。意味上の参照は常にキーを使う。

### 11.2 大きさの上限

v1は1ファイル256 KiB、パッケージ合計8 MiB、文書総数33件、深さ16、Version32件。各Versionはinputs64件、環境変数128件、ports16件、storage16件、connections16件、選択肢128件、各argv64要素を上限とする。単一の固定文字列・説明以外の文字列値は最大16 KiB。型ごとの小さい上限がある場合はそちらを優先する。

過大なファイルをすべてメモリーに展開した後で拒否する設計にしない。正規表現もサイズ上限内でコンパイルできることを確認する。上限超過のエラーは「この版で扱える上限」を明示する。

### 11.3 パス・集合の固定と登録単位

- 参照は`versions/<name>.yaml`のみ。nameは小文字英数字で始まる1〜64文字の小文字英数字・ドット・ハイフン・アンダースコア。末尾ドット、`..`、Windows予約名（最初のドット以前がcon、prn、aux、nul、com1〜9、lpt1〜9）を拒否する。
- 区切りは`/`固定。絶対パス、ドライブ、UNC、逆スラッシュ、URL、環境変数、親参照、追加階層は認めない。同一パスまたは同一実体ファイルの複数Versionへの割当てを拒否する。
- パッケージ・versionsディレクトリ・参照ファイルのsymlink／junction等を拒否する。実際に開いた対象がパッケージ内の通常ファイルかを検証し、文字列prefixだけで判定しない。
- 読込み中に単体・累積サイズ上限を検査する。全原文のバイト列を固定し、検証・確認・登録は同じ集合を使う。取得中の変更・差替えを検出した場合は破棄して再読込みを求める。確認後は外部ファイルを読み直さない。
- 未列挙ファイルは取込み・展開せず、未使用のVersion定義を作者向けに通知する。別のTemplateやファイルを探索して欠損を補わない。
- 全Versionが有効な場合だけ、パッケージ全体を1つの不変版として原子的に登録する。1件でも欠損・不正ならその版全体を拒否し、他Template・既存登録版・Instanceを維持する。再読込み失敗を旧版の表示で成功と見せない。

## 12. 分かりやすいエラー

エラーはパッケージ、Version、相対ファイル名、行・列（取得できる場合）、項目パス、理由、修正例を含める。全入力を丸ごと出力せず、秘密値は必ず伏せる。

| エラー例 | 日本語の案内例 |
| --- | --- |
| `service.ports.database.defautHost` | `defautHost`は使えません。`defaultHost`の誤記ではありませんか |
| `defaultVersion: 18` | Versionは文字列で指定してください。例: `defaultVersion: "18"` |
| 未定義`{ input: passwrod }` | `passwrod`という入力がありません。参照先のキーを確認してください |
| secretに`default` | パスワードの固定値はTemplateに保存できません。自動生成または初回入力を使ってください |
| storageに`host` | ホスト側の保存先はアプリが割り当てます。コンテナ内の保存先を`container`へ指定してください |
| Version内の参照先が消失 | Version「17」のversions/17.yamlに入力「database」がありません。参照先または入力定義を修正してください |
| 未知schemaVersion | このアプリではSchema版「2」を扱えません。対応アプリを確認してください |

誤記候補は提案するだけで自動修正しない。不正Templateは個別に無効化し、他Templateや既存Instanceを壊さない。

## 13. ユーザー作成Templateとインポート

### 13.1 版ごとの範囲

v1必須はローカルパッケージの追加・再読込みである。templates/local直下の各ディレクトリのtemplate.yamlだけを入口として検出する。単一.template.yamlやVersionファイルは入口として受け付けない。本仕様の検証、出所区分、不変版、Snapshotはこの段階から適用する。

ユーザー向けのディレクトリ選択によるインポート画面は将来機能として仕様の接続点を定める。v2のコンテナWeb版と同じリリースへ必ず含めるとは決めない。オンラインカタログ、URL直接取得、ZIPパッケージ、依存Template、署名配布の実装をv1へ増やさない。

### 13.2 インポートの基本フロー

1. ユーザーがローカルのパッケージディレクトリを選ぶ。
2. アプリがマニフェストと列挙Versionファイルのバイト列集合を固定し、構文・構造・意味を検証する。パッケージ外のファイルやURLを取得しない。
3. 名前、作者（自己申告）、Template版、サービスVersion、イメージの取得先、公開ポート、保存slot、command、Health Check、秘密入力の有無を要約する。
4. 「ユーザー作成・未検証」と出所を示し、登録操作を受け付ける。
5. 検証した同じ内容を不変なTemplateRevisionとして登録する。確認後に元ファイルを読み直して別内容を登録しない。
6. Templatesに表示する。環境を作るには改めてTemplateを選択し、作成フォームで操作する。

インポートではImage pull、コンテナ作成、実行、永続データ領域の作成を行わない。ローカル定義ディレクトリで追加を検知した場合も、検証・内容確認を経る。ディレクトリへ置かれただけで自動起動しない。

### 13.3 出所と名前の衝突

同梱／ローカル追加／インポートと登録元は、ファイルに書けないアプリ側メタ情報とする。`author`、`homepage`、`id`は信頼の証明ではない。公式イメージを使う自作Templateも、同梱Templateとは表示を分ける。

| 同じID・版を読み込んだ場合 | 動作 |
| --- | --- |
| 正規化した定義内容が同一 | 再登録を増やさず、既存定義を表示。自己申告で信頼区分を昇格させない |
| 正規化した定義内容が異なる | 上書きを拒否。templateVersionを上げるか、別idにするよう案内 |
| 同じIDで新しい版 | 別の不変版として追加。既存Instanceや開いたClone計画へ自動適用しない |
| 同梱IDを自作ファイルが使用 | 出所を偽装できない。異なる内容の同一版は拒否。同梱系列への別版追加も通常の自作扱いとし、既定選択を乗っ取らせない |

同じIDの複数版から利用する版は明示選択する。番号が大きい自作版を自動的に既定にしない。Template全体をカタログから削除しても、InstanceのSnapshotを削除しない。

### 13.4 信頼性の限界

構造検証に通っても、Image・command・Health Checkがネットワーク通信や破壊的な処理を行わないとは保証できない。特に入力した秘密をコンテナが外部送信する可能性はSchemaでは排除できない。

対策は、Templateにホスト操作の権限を与えないこと、任意ホストマウント等を許可しないこと、使用イメージと実行内容を確認できること、未検証を明示することを組み合わせる。ホストの管理ディレクトリをコンテナへ一括マウントしてはならない。

## 14. 正規化・Snapshot・更新

読込みに成功した定義から、Versionごとの有効inputs・service・connections、既定値、型別Policy、生成器版、Core固定項目、表示順を展開した正規形を作る。

Snapshotには少なくとも以下を保持する。

- マニフェストと全Versionの原文集合（相対パス・バイト列・各SHA-256）、schemaVersion、Template ID・版、出所のアプリ側情報。
- 選択Versionとそのパッケージ版の全Versionの完全定義、正規化方式template-normalization-v1。
- Clone Policy仕様版1、identity-v1／project-v1／storage-v1／secret-v1等の使用規則。
- 定義の同一性を判定する情報と表示順。

正規化はtemplate-normalization-v1の安定JSONとSHA-256を使う。コメント、インデント、参照ファイルの配置名は意味の同一性に含めない。原文hashと意味のhashは区別し、同内容の再登録で既存原文を上書きしない。表示順、ラベル、Default、Policy、実行設定が変われば内容変更として扱う。表示順は明示配列としてhashに含め、プロセス依存のハッシュ値を使わない。新しいカタログのVersionを既存Snapshotへ自動追加せず、外部ファイルの更新・削除後も保存済みの全Versionから再生成・Cloneする。

既存Snapshotへ新しい省略規則を適用し直さない。新アプリが旧Snapshotを扱える場合も、その定義時点の意味を維持する。未知版を「似ているから」と解釈せず、対応不可として案内する。

### Image参照と再現性

タグ付き参照と`sha256`digest付き参照を許可し、タグなし・`latest`タグは拒否する。Image参照はVersionごとの固定値であり、ホスト環境変数やフォームから変更しない。

v1の基本契約は確定したImage参照を維持すること。digest指定なら同じVersionのCloneでそのdigestを保持する。タグから取得時に解決したdigest・platformは証跡として記録するが、それだけで保存済みタグを黙って書き換えない。再試行のイメージ解決、同一内容の強制方針は実行設計で定め、タグだけで内容完全同一を保証しない。

## 15. サービス対応の広がりと限界

| サービスの設定形態 | 本仕様での表現 |
| --- | --- |
| 環境変数で初期DB・ユーザーを設定 | inputsとenvironment。PostgreSQL・MySQL系等の定義に適する |
| 引数で動作・永続化・認証を設定 | argv配列とinput参照。Redis例で示す |
| APIと管理画面など複数ポート | portsに独立slotを追加する |
| データ・ログ等の複数保存先 | storageに独立slotを追加する |
| Versionでマウント先や起動方法が変わる | Version別ファイルに独立した完全なserviceを書く |
| 入力不要の開発用エミュレーター | inputsなしで、command・ports・storageを定義する |
| 複数コンテナの依存関係が必須 | v1対象外。将来の複合Templateモデルが必要 |
| 任意の設定ファイル、証明書、初期化スクリプトが必須 | Version定義以外の実行用ファイルは取り込まない。将来のファイル生成・配布仕様が必要 |
| デバイス・特権・host networkが必須 | v1の安全範囲では非対応 |
| 単一引数への値連結・複雑な条件付き構成が必須 | v1では非対応。必要に応じて明示的な拡張を検討 |

MongoDB、MinIO、Azurite、RabbitMQ等も、実際に選ぶImageの設定方式が上記の宣言能力へ収まるかを個別に検証する。名前を追加しただけで対応済みとはしない。

将来拡張はSchema版・能力を明示し、旧アプリが未知キーを無視して危険な省略をしないようにする。インポートUI、多言語ラベル、構成ファイル生成、複数サービス、データCloneフックは別々の拡張として評価する。

日本語ラベルはv1では文字列のままとし、最初から作者へ多言語マップを要求しない。安定キーと表示文字列を分離しているため、将来の翻訳を別の表示資源や明示的な新版形式で追加できる。

## 16. 作者向けチェック手順

1. 対象Imageの公式説明を確認し、必要な環境変数・引数・保存先・Health Checkを洗い出す。
2. 近いパッケージ例をディレクトリごとコピーし、id・templateVersion・名前を変更する。利用者の実Passwordやホストパスを書き込まない。
3. マニフェストに1 Versionを登録し、そのVersionファイルへ完全なinputsとserviceを書く。ポートと保存領域のClone規則は追記しない。
4. 読込み検証で解決後のフォームと実行設定を確認する。
5. 実際に作成・接続・停止・再起動・再作成を試す。保存すべきデータが残るか確認する。
6. bind mountとnamed volumeで、元データありの設定Cloneを試す。先に元データがなく、双方が独立して動くことを確認する。
7. Version追加時は新ファイルとマニフェストを更新し、登録済みならtemplateVersionを上げる。全Versionの構造・意味と、変更対象の両保存方式の動作を検証する。
8. 定義を共有するときは、作者、取得先、検証した環境を別途示す。アプリ側の「検証済み」表示をファイル内で自己申告しない。

Redis例のHealth Checkでは、認証情報をREDISCLI_AUTHから受け取り、`redis-cli -e`でコマンド失敗を終了コードへ反映する。起動側のrequirepass引数に秘密が現れる点はv1平文方針の範囲内で説明し、アプリログには出さない。[Redis CLI公式資料](https://redis.io/docs/latest/develop/tools/cli/)

## 17. 仕様の受入シナリオ

| ID | 検証内容 | 期待結果 |
| --- | --- | --- |
| TS-T01 | 最小例を読み込む | 共通フォームとポート・保存選択が表示可能。inputsは不要 |
| TS-T02 | PostgreSQL例の17／18を解決 | dataのマウント先だけが各定義どおりになる |
| TS-T03 | Redis例の秘密参照を解決 | commandと環境変数に同じ先の秘密が渡り、表示はマスク |
| TS-T04 | string・integer・boolean・select・secret | 型に応じた入力とValidationが成立する |
| TS-T05 | Versionを数値で書く | 文字列を要求し、無断変換しない |
| TS-T06 | 必須キー欠落・未知キー・重複キー | 読込み拒否。項目と修正方法を示す |
| TS-T07 | YAML alias・独自タグ・複数文書・上限超過 | 実行・過大展開前に拒否 |
| TS-T08 | 未定義input・削除されたVersion項目参照 | 全Versionの意味検証で拒否 |
| TS-T09 | optionalをcommandで参照 | 引数欠落の可能性があるため拒否 |
| TS-T10 | optionalのenvironment参照にomit指定 | 未設定なら変数全体を省略。空文字なら保持 |
| TS-T11 | copyでfalse・0・空文字 | 未設定に変換せず型に従って保持 |
| TS-T12 | secretへdefault、secret-v1へ不適合制約 | 配布時の固定秘密・生成不適合を拒否 |
| TS-T13 | clone: clearとDefault | CloneでDefaultを再適用せず入力待ちになる |
| TS-T14 | storageのhost・volume実名・clone指定 | Coreの割当てに反する未知キーとして拒否 |
| TS-T15 | 同一・親子storage target、同一container port | 解決後の各Versionで拒否 |
| TS-T16 | Version別にenvironmentやinputsを追加・削除 | 各ファイルの完全定義だけを使用。他Versionから補完しない |
| TS-T17 | `${...}`や`$`を含む秘密・固定文字列 | 再解釈せずCompose上で値を維持 |
| TS-T18 | 同じID／版・同内容を再登録 | 重複を増やさず出所を昇格させない |
| TS-T19 | 同じID／版・異なる内容を登録 | 上書き拒否。新しい版かIDを要求 |
| TS-T20 | 新版・カタログ削除後に旧InstanceをClone | 旧Snapshotと固定されたPolicyを使用 |
| TS-T21 | 自作ファイルで同梱名・作者を名乗る | 未検証の出所を保持。既定Templateを乗っ取らせない |
| TS-T22 | 将来のインポート確認中に元ファイルを変更 | 検証したバイト列だけを登録。確認前後で内容をすり替えない |
| TS-T23 | インポート登録だけを実行 | Image取得・コンテナ・データ領域が作られない |
| TS-T24 | 両方式で元にデータを入れてClone | 全保存slotが独立し、元データが先へコピーされない |
| TS-T25 | Imageのplatform・マウントが宣言と不一致 | 実行時に不一致を示し、安全な利用可能状態と判定しない |
| TS-T26 | 自作の悪意あるImage | 構造検証だけで安全と宣言しない。権限制限と出所表示を確認 |
| TS-T27 | 未列挙ファイル・Versionファイルの単独受付 | 未列挙内容を取込みせず、単独登録は拒否 |
| TS-T28 | 非既定Versionの欠損・不正 | パッケージ全体を拒否。他Templateと既存版は維持 |
| TS-T29 | 絶対・親参照・URL・予約名・重複参照 | ファイル受付で拒否。3OSで結果が一致 |
| TS-T30 | symlink／junction・開いた実体の差替え | 範囲外・通常ファイル以外を拒否し、取得中の変更で集合を破棄 |
| TS-T31 | 単体・累積サイズ・文書数上限超過 | 上限超過の展開前に拒否 |
| TS-T32 | コメント・配置名のみ変更／表示順変更 | 前者の意味hashは同じ、後者は異なる。原文hashは別保持 |
| TS-T33 | 新Versionをカタログへ追加後に旧InstanceをClone | Snapshot内の全Versionだけを提示。新Versionを自動追加しない |
| TS-T34 | マニフェストの共通inputs・Version内の別ファイル参照 | 未知キーとして拒否し、継承・差分解釈をしない |
| TS-T35 | 同一入力キーをVersion間で型変更 | 全体検証で拒否。別キーの使用を案内 |
| TS-T36 | 新規作成／Cloneで入力項目・slotが増減 | 選択先だけを対象に再検証。新規DefaultとCloneのcopy入力待ちを区別 |

TS-T01〜23および27〜36は構造・意味・登録の検証、TS-T24〜26は実行基盤・信頼境界を含む検証へ引き継ぐ。これらは検証すべきシナリオであり、現段階の実行済みテスト結果ではない。

## 18. 上位仕様への具体化と次の成果物

今回、Schema 1のマニフェスト＋Version別完全定義、5つの入力型、明示的なinput参照、継承なしの設定管理、Core固定項目、secretの安全な初期値、インポート時の検証と出所管理を定めた。通常の作者はClone生成器やホスト側命名を記述する必要がない。

上位仕様で未決だったTemplateの宣言構文、slot定義、Version差、Policy省略時の固定化を具体化した。入力間の任意式、パッケージ外参照、実行用外部ファイル、自由なCompose断片は導入していない。ユーザー作成Templateのインポートは将来の明示的な利用フローとして位置づけ、v1のローカル追加にも同じ検証原則を適用する。

後続の[v1 アーキテクチャ設計](architecture-v1.md)で、Reactでのフォーム生成、Rustでの構文・意味検証、TemplateRevision／Snapshotの保存、SQLiteと生成物の整合、Docker CLI実行、管理ルートの権限、コンテナWeb版の境界を具体化した。機械検証用Schemaとパーサーの実装・適合性テストは、同書の責務分担に沿ってMVPタスクへ落とす。
