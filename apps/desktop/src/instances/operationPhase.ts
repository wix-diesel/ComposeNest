const phase: Record<string, string> = { accepted: "受付済み", prepare: "準備中", candidate: "変更候補の確認", image: "イメージ確認", target: "接続先確認", config: "設定確認", compose: "構成の準備", observe: "実行状態を確認", expected: "構成の照合", inspect: "状態確認", storage: "データ確認", artifact: "設定確認", recreate: "コンテナ再作成", start: "起動中", stop: "停止中", restart: "再起動中", ready: "利用可能か確認", running: "実行状態を確認", stopped: "停止を確認", absent: "不在を確認", reconcile: "結果の確認が必要", start_required: "起動を選択", done: "完了" };

/** Labels saved operation phases without exposing internal tokens. */
export function operationPhaseLabel(value: string | null | undefined): string {
  return phase[value ?? ""] ?? "確認中";
}
