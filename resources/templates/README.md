# 配布Template

このディレクトリには、実Dockerで両保存方式の受入を通過した定義と、その検証環境・結果をセットで登録する。
PostgreSQL 17/18の記録は [postgresql/VALIDATION.md](postgresql/VALIDATION.md) を参照する。
確認済みの範囲はUbuntu 24.04 x86_64のCI上のDocker Engineに限る。製品の対象OS（Windows・macOS・Ubuntu 26.04）での配布受入は未完了であり、同梱という出所や`platforms`宣言を各OSの検証済み表示として扱わない。
`docs/template-examples` は引き続き仕様例であり、同梱扱いにしない。
ローカル定義は実管理ルートの `templates/local/<package>/template.yaml` と `versions/*.yaml` に配置する。
