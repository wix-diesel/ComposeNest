# 共通画面枠とナビゲーション（Issue #37）

PR #70の元資料を復元するPR #158が依存元。各画面内容・テーマ・業務APIは別Issueであり、今回の画面は機能準備中と明示する。

- 5つの主要ナビ、sidebar・topbar・パンくず・footerはHTMLモックの共通枠に準拠。
- 通常幅224px／800px以下70pxのsidebar。アイコンのみでも日本語アクセシブル名を維持。
- `ApplicationClient.navigate(AppRoute)`で10画面へ移動。hashchange購読で戻る／進むにも追従。
- 詳細／複製／編集は`instanceId`、作成は`templateId`、処理画面は`operationId`と任意の`instanceId`を渡す。URLに秘密・Plan・業務設定を保存しない。
- 処理／複製／編集の戻り先は同じ対象の詳細。詳細は一覧、作成はテンプレートに戻る。直接リンクでも同じ階層になる。
- 読込み中・初期取得失敗／再確認、Docker未確認を表示。タイマーやクリックだけでDocker接続・業務操作成功を表示しない。
- 文言は`messages.ts`の日本語キー。通常の通知は共通Toast、確認はConfirmDialogを利用。
- native dialogの初期フォーカスは閉じる／キャンセル。Escape、Tabのフォーカス制限、閉じた後の起点復帰を扱う。

後続の画面は、取得した実IDを使って遷移する。例:

```ts
client.navigate({ page: "instance-detail", instanceId });
client.navigate({ page: "instance-create", templateId });
client.navigate({ page: "operation", operationId, instanceId });
```

検証: `pnpm --dir apps/desktop run test:navigation`（Node 24の型除去）、`run build`（TypeScript）、`run check:contracts`、`git diff --check`がローカルで成功。

`pnpm --dir apps/desktop exec playwright install chromium`後に`run test:shell`を実行する。ブラウザ試験は10画面、履歴、ID・戻り先、Escape・初期／復帰フォーカス、1280／800／390px、bootstrap失敗と再試行を検証。CIではChromiumをインストールし、画面PNGをartifactへ保存する。

ローカルはChromiumダウンロードが不完全なZIPとなりブラウザ試験を実行できなかったため、CIの実行結果をPRに記録する。今回Rust・Tauriの業務command／capabilityの変更はなく、Docker実機での業務操作・3OS WebView受入・各画面内容とテーマの比較は後続Issueの検証範囲。
