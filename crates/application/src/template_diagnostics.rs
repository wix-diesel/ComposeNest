//! Japanese author guidance for restricted Template validation diagnostics.

use composenest_domain::template::TemplateError;

/// Preserves source coordinates and paths while showing a Japanese reason and correction.
pub fn format_template_error(error: &TemplateError) -> String {
    let position = error.position.map_or_else(String::new, |position| {
        format!(" ({}行・{}列)", position.line, position.column)
    });
    format!(
        "Version {}: {}{position}: {}: {}",
        error.version.as_deref().unwrap_or("—"),
        error.file,
        error.path,
        japanese_reason(&error.message)
    )
}

pub(crate) fn japanese_reason(message: &str) -> String {
    let reason = match message {
        "input is not used by service configuration" => {
            "この値はコンテナ設定に反映されません。serviceから参照するか、不要な入力を削除してください。"
        }
        "duplicate key; remove the repeated entry" => {
            "項目名が重複しています。同じ項目の定義を1つにまとめてください。"
        }
        "listed version definition is missing" => {
            "列挙されたVersionの定義がありません。指定したファイルを追加してください。"
        }
        "duplicate version definition" => {
            "Versionの定義が重複しています。各Versionを1つだけ定義してください。"
        }
        "unlisted version definition" => {
            "一覧にないVersionが定義されています。versionsへの登録または不要な定義の削除を行ってください。"
        }
        "input type differs between versions; use a new key" => {
            "同じ入力名の型がVersion間で異なります。型をそろえるか、別の入力名を使用してください。"
        }
        "minimum exceeds maximum" => {
            "最小値が最大値を超えています。最小値が最大値以下になるように修正してください。"
        }
        "duplicate select option" => {
            "選択肢の値が重複しています。各選択肢に異なる値を指定してください。"
        }
        "default violates input validation" => {
            "既定値が入力の制約を満たしていません。制約内の既定値に修正してください。"
        }
        "secret-v1 requires a 32-character range without a pattern; use ask policies" => {
            "秘密値の自動生成条件を満たしていません。32文字を許可してpatternを外すか、initialとcloneをaskにしてください。"
        }
        "input is not defined in this version" => {
            "参照した入力がこのVersionにありません。inputsへ定義するか、参照名を修正してください。"
        }
        "onMissing is only allowed for optional inputs" => {
            "必須入力にはonMissingを指定できません。onMissingを削除してください。"
        }
        "optional input requires environment onMissing: omit and cannot be used in command" => {
            "省略可能な入力の参照方法が不正です。environmentでonMissing: omitを指定し、commandでは参照しないでください。"
        }
        "duplicate container port" => {
            "コンテナ内ポートが重複しています。ポートごとに異なる番号を指定してください。"
        }
        "use an absolute Linux path without root, parent segments or control characters" => {
            "保存先のパスが不正です。ルート単独・親階層・制御文字を含まないLinuxの絶対パスを指定してください。"
        }
        "storage targets overlap" => {
            "保存先が重複または包含しています。互いに重ならないパスを指定してください。"
        }
        "port slot is not defined in this version" => {
            "参照したポートがこのVersionにありません。service.portsへ定義するか、参照名を修正してください。"
        }
        "only integer schemaVersion: 1 is supported" => {
            "Schema版が未対応です。schemaVersionを整数の1にしてください。"
        }
        "use 2 or more lowercase identifier segments separated by dots" => {
            "IDの形式が不正です。example.redisのように、小文字の識別子を2つ以上ドットで区切ってください。"
        }
        "use a three-part numeric version such as \"1.0.0\"" => {
            "Template版の形式が不正です。\"1.0.0\"のような3つの数値をドットで区切った文字列を指定してください。"
        }
        "homepage must use HTTPS" => "homepageのURLが不正です。https://で始まるURLにしてください。",
        "version key must contain 1–64 ASCII letters, digits, dots, hyphens or underscores" => {
            "Version名の形式が不正です。英数字・ドット・ハイフン・下線を使い、1～64文字にしてください。"
        }
        "use a safe versions/<name>.yaml relative path" => {
            "Versionファイルのパスが不正です。versions/<name>.yaml形式の安全な相対パスを指定してください。"
        }
        "two versions cannot reference the same file" => {
            "複数のVersionが同じファイルを参照しています。Versionごとに別のファイルを指定してください。"
        }
        "select a key listed in versions" => {
            "既定Versionが一覧にありません。defaultVersionにversions内のVersion名を指定してください。"
        }
        "image cannot be empty" => {
            "イメージ名が空です。imageに使用するイメージの参照を指定してください。"
        }
        "duplicate platform" => {
            "プラットフォームが重複しています。同じ値を1つだけ指定してください。"
        }
        "file exceeds the 256 KiB limit" => {
            "ファイルのサイズが上限を超えています。256 KiB以下にしてください。"
        }
        "file must be UTF-8" => "文字コードが未対応です。ファイルをUTF-8で保存してください。",
        "expected one YAML document" | "only one YAML document is allowed" => {
            "YAML文書の数が不正です。1ファイルに1文書だけ記述してください。"
        }
        "nesting exceeds the depth limit of 16" => {
            "入れ子が深すぎます。階層を16以下にしてください。"
        }
        "scalar exceeds the 16 KiB limit" => {
            "値のサイズが上限を超えています。1つの値を16 KiB以下にしてください。"
        }
        "null and non-integer numbers are unsupported; quote numeric text to use a string" => {
            "nullや小数は使用できません。整数を使うか、文字列として引用符で囲んでください。"
        }
        "mapping keys must be strings without tags or anchors"
        | "mapping keys must be strings; quote numeric keys and do not use merge keys" => {
            "項目名の形式が不正です。数字は引用符で囲み、マージキー・タグ・アンカーを使用しないでください。"
        }
        "YAML aliases are not allowed" | "YAML anchors and tags are not allowed" => {
            "YAMLのエイリアス・アンカー・タグは使用できません。値を直接記述してください。"
        }
        "expected a YAML value" => "YAMLの値がありません。項目に値を記述してください。",
        "default must match input type; secret defaults are forbidden" => {
            "既定値の型が不正、または秘密値に既定値が指定されています。入力型をそろえ、秘密値のdefaultを削除してください。"
        }
        "options is only allowed for select inputs" => {
            "この入力型にはoptionsを指定できません。optionsを削除するか、typeをselectにしてください。"
        }
        "select inputs require options" => {
            "選択肢がありません。select型の入力にoptionsを指定してください。"
        }
        "pattern exceeds 256 characters" => {
            "正規表現が長すぎます。patternを256文字以下にしてください。"
        }
        "initial is only allowed for secret inputs" => {
            "この入力型にはinitialを指定できません。secret型以外のinitialを削除してください。"
        }
        "onMissing is only allowed in environment" => {
            "environment以外にはonMissingを指定できません。onMissingを削除してください。"
        }
        "expected a scalar or { input: name } reference" => {
            "値の形式が不正です。単一の値、または{ input: name }形式の参照を指定してください。"
        }
        "duplicate connection input" => {
            "接続情報の入力が重複しています。同じ入力名を1つだけ指定してください。"
        }
        "invalid key; use the key format specified for this map" => {
            "項目名の形式が不正です。このマップのSchema仕様に沿った識別子に修正してください。"
        }
        "expected a mapping" => "マップ形式ではありません。項目名: 値の形式で記述してください。",
        "expected a sequence" => {
            "配列形式ではありません。-で始まる行、または[値, 値]の形式で記述してください。"
        }
        "expected a string; quote numeric values" => {
            "文字列ではありません。数字を文字列として使う場合は引用符で囲んでください。"
        }
        "expected an integer" => "整数ではありません。引用符なしの整数を指定してください。",
        "expected true or false" => {
            "真偽値ではありません。引用符なしのtrueまたはfalseを指定してください。"
        }
        _ if message.starts_with("unknown key ") => {
            "未知の項目です。表示された項目パスの定義を削除するか、項目名の綴りを修正してください。"
        }
        _ if message.starts_with("required key ") => {
            "必須項目がありません。表示された項目パスに値を追加してください。"
        }
        _ if message.starts_with("invalid regular expression:") => {
            "正規表現の構文が不正です。patternの括弧・エスケープ・対応する構文を確認してください。"
        }
        _ => return japanese_constraint(message),
    };
    reason.into()
}

fn japanese_constraint(message: &str) -> String {
    if let Some(range) = message.strip_prefix("expected an integer from ") {
        return format!(
            "整数が範囲外です。{}の整数を指定してください。",
            range.replace(" to ", "～")
        );
    }
    if let Some(choices) = message.strip_prefix("expected one of: ") {
        return format!("値が未対応です。次のいずれかを指定してください：{choices}。");
    }
    for (suffix, unit) in [(" characters", "文字"), (" items", "件")] {
        if let Some(range) = message
            .strip_prefix("expected ")
            .and_then(|value| value.strip_suffix(suffix))
        {
            return format!("{unit}数が範囲外です。{range}{unit}にしてください。");
        }
    }
    // Parser-specific text may include raw input; never echo it into the author-facing UI.
    "YAMLの定義を解析できません。表示位置の字下げ・引用符・区切りを確認し、Schema 1の形式に修正してください。".into()
}

#[cfg(test)]
mod tests {
    use super::*;
    use composenest_domain::template::Position;

    #[test]
    fn diagnostics_keep_coordinates_and_hide_unclassified_parser_input() {
        let mut error = TemplateError {
            package: "package".into(),
            version: Some("17".into()),
            file: "versions/17.yaml".into(),
            position: Some(Position {
                line: 4,
                column: 12,
            }),
            path: "$.service.unknown".into(),
            message: "unknown key unknown; remove it or correct the spelling".into(),
        };
        let message = format_template_error(&error);
        for expected in [
            "Version 17",
            "versions/17.yaml",
            "4行・12列",
            "$.service.unknown",
            "未知の項目",
            "綴りを修正",
        ] {
            assert!(message.contains(expected), "{message}");
        }
        error.position = None;
        error.message = "parser error with secret-source-value".into();
        let message = format_template_error(&error);
        assert!(!message.contains("行・"));
        assert!(!message.contains("secret-source-value"));
        assert!(message.contains("字下げ・引用符・区切り"));
    }

    #[test]
    fn constraints_preserve_ranges_and_choices_in_japanese_corrections() {
        for (reason, expected) in [
            ("expected an integer from 1 to 65535", "1～65535の整数"),
            ("expected 1–64 characters", "1–64文字"),
            ("expected 0–16 items", "0–16件"),
            ("expected one of: copy, ask", "copy, ask"),
            (
                "duplicate key; remove the repeated entry",
                "定義を1つにまとめて",
            ),
            ("required key image is missing", "必須項目がありません"),
            ("minimum exceeds maximum", "最小値が最大値"),
        ] {
            assert!(japanese_reason(reason).contains(expected));
        }
    }
}
