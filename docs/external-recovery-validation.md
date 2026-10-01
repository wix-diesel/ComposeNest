# 外部編集復帰の検証（Issue #36）

検証日: 2026-10-01。#150／#151／#152／#155に分割し、PR #153 → #154 → #156 → #157の順で統合する。

| 環境 | 検証対象 | 結果 |
| --- | --- | --- |
| GitHub Actions Ubuntu 24.04、Rust 1.98.1 | workspaceテスト、fmt、Clippy、IPC契約、TypeScript型検査、Vite build | 成功（[CI実行227](https://github.com/wix-diesel/ComposeNest/actions/runs/36826249363)） |
| GitHub Actions Windows Server 2025／macOS 15 | Adapterテストと管理ルート権限。Windowsの退避時ACL、Unixの0700／0600、リンク拒否 | 成功（[CI実行227](https://github.com/wix-diesel/ComposeNest/actions/runs/36826249363)） |
| GitHub Actions Ubuntu 24.04、Docker Engine 28.0.4（linux/amd64）、Compose v2.38.2、alpine:latest | 所有する構成不一致コンテナの停止・停止状態再作成、保存秘密・bindデータ・退避原文の維持 | 成功（[CI実行227](https://github.com/wix-diesel/ComposeNest/actions/runs/36826249363)） |

hashだけの差分、ファイル追加・削除、確認後の再編集、退避後の中断、CLI失敗後の実体照合、同じOperationでの再試行、完了済みリクエストの再送を検証する。一致する稼働中コンテナは維持し、構成が異なる場合だけStop→停止状態再作成を行う。

所有不明、Engine不一致、前CLI未終了、保存領域欠落では保留する。外部Composeとrecoveryファイルは実行引数にせず、保存Spec・固定Image・保存領域から新しいArtifactを生成する。Specの秘密とポート予約は変更しない。

呼出し側は`preview_external_recovery`の差分と確認hashを提示し、明示確認後に`ExternalRecoveryRequest`を送る。`RecoveryProbe`で記録済みCLIの終了を確認してから再試行する。管理UI／IPCへの接続は後続UIタスクの範囲とする。

Windows／macOSのDocker Desktop、named volume、PostgreSQL／Redisを組み合わせた実機受入は未実施。CIのOS権限テストとLinux Engineの検証範囲を区別する。
