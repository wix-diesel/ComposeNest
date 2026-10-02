/** Stable Japanese keys for the common shell; service names and IDs remain unchanged. */
export const ja = {
  pages: {
    instances: "環境一覧", templates: "テンプレート", retained: "保持データ",
    diagnostics: "Docker診断", settings: "設定", "instance-create": "環境を作成",
    "instance-detail": "環境の詳細", "instance-clone": "設定を複製",
    "instance-edit": "環境を編集", operation: "処理状況",
  },
  subtitles: {
    instances: "必要な環境を、必要なときに。開発環境をまとめて管理します。",
    templates: "サービスを選ぶだけで、開発環境の準備を始められます。",
    retained: "削除した環境のデータと、元の設定の所在を確認できます。",
    diagnostics: "環境の作成・起動に必要な準備状況を確認します。",
    settings: "新しい環境の既定値と、アプリの情報を確認します。",
    "instance-create": "用途に合わせて設定し、新しい環境を作成します。",
    "instance-detail": "対象の環境の設定と実行状態を確認します。",
    "instance-clone": "元の設定から、独立した新しい環境を作成します。",
    "instance-edit": "対象の環境の表示名と接続ポートを変更します。",
    operation: "対象の処理の状況を確認します。",
  },
  workspace: "ワークスペース", application: "アプリケーション", menu: "メインメニュー",
  breadcrumb: "現在位置", home: "ホーム", back: "戻る", about: "この画面について",
  unimplemented: "この画面の機能は準備中です。Docker操作や設定変更は実行されません。",
  engineUnknown: "Docker 未確認", local: "ローカルワークスペース", computer: "このコンピューター",
  loading: "アプリに接続中", ready: "アプリに接続済み", bootstrapFailed: "アプリの初期情報を取得できませんでした。再確認してください。",
  retry: "再確認", cancel: "キャンセル", close: "閉じる", confirm: "確認", dismiss: "通知を閉じる",
  target: "対象ID", operationId: "処理ID", templateId: "テンプレートID",
  footer: "ローカル開発を、もっと身近に。", version: "v1", skip: "画面の内容へ移動",
} as const;
