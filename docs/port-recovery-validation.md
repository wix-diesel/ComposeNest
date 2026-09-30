# ポート復旧の検証（Issue #34）

検証日: 2026-09-30。実装は #133／#134、障害注入テストは #136 に分割した。

| 環境 | 実施内容 | 結果 |
| --- | --- | --- |
| Windows x86_64、Rust 1.98.1、Node 24.19.0 | workspace テスト、fmt、Clippy、IPC 契約、TypeScript 型検査、Vite build | 成功 |
| GitHub CI Ubuntu 24.04／macOS 15 | 固定 Engine・構成・所有・保存領域を模擬したポート復旧テスト、実 OS の TCP 競合 | 成功（PR #138 の CI） |
| GitHub CI Ubuntu 24.04 | 既存 Docker Compose／コンテナ契約テスト | 成功。ポート復旧の実機受入とは区別する |

前 CLI 未終了時の保留、反映直後の中断、停止状態の再試行・旧 Artifact 復帰、復帰後の再編集、所有／Artifact／領域／Engine 不明時の予約保持を検証した。Create／Clone の競合提案、確認前の無変更、確認時と反映後の再競合、同じ秘密・領域で Ready を確認して予約を切り替える動作も検証した。

Docker 実機でのポート復旧、PostgreSQL／Redis × bind／volume の実機組合せ、対象最低 OS での受入は未実施。模擬 CLI テストの成功を実機検証済みとは扱わない。

呼出し側は `RecoveryProbe` で記録済み試行の CLI 終了を確認し、`PortRecoveryAction::Propose` の候補を提示してから `Confirm` を送る。管理 UI／IPC の接続は後続 UI タスクの範囲であり、無確認のポート変更やデータのロールバックは追加していない。
