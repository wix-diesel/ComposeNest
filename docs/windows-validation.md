# Windows 管理ルート・権限検証

Issue #5 の初回セットアップと検証手順。通常 GUI は昇格せずに起動する。初期化は昇格した Windows PowerShell 5.1 で行い、管理する個人利用者の SID を明示して渡す。昇格したプロセスの SID を所有者として推測しない。

## 初回セットアップ

通常利用者のセッションで `whoami /user` を実行し、対象 SID を控える。別の昇格した Windows PowerShell 5.1 から次を実行する。

```powershell
powershell.exe -NoProfile -ExecutionPolicy Bypass -File scripts\windows\Initialize-ManagementRoot.ps1 -OwnerSid 'S-1-5-21-...'
icacls "$([Environment]::GetFolderPath([Environment+SpecialFolder]::CommonApplicationData))\ComposeNest"
```

スクリプトは ProgramData 既知フォルダーを解決し、`ComposeNest` と管理用の直下ディレクトリを作る。所有者を指定 SID にし、継承を切って、その利用者、SYSTEM、Administrators のみにフルアクセスを与える。既存ディレクトリに異なる所有者・ACL・再解析ポイントがある場合は停止し、再帰的な権限変更はしない。通常 GUI 用の SID はローカル／ドメインの個人利用者を指定する。Docker Desktop の設定やグループは変更しない。

通常利用者の Windows PowerShell から次を確認する。`state` の検証ファイルは確認後に削除する。

```powershell
$root = Join-Path ([Environment]::GetFolderPath([Environment+SpecialFolder]::CommonApplicationData)) 'ComposeNest'
$probe = Join-Path $root 'state\permission-probe.txt'
[IO.File]::WriteAllText($probe, 'ok')
[IO.File]::ReadAllText($probe)
Remove-Item -LiteralPath $probe
```

自動検証は `powershell.exe -NoProfile -ExecutionPolicy Bypass -File scripts\windows\Test-ManagementRoot.ps1`。日本語・空白を含む一時パスで、再実行、別の実在利用者 SID に対する所有者不一致の拒否、一般利用者による状態ファイル作成、Users／Everyone の ACE が付かないことを確認する。`data` 子領域だけ読取り拒否にしても、親 `data` と `state` にアクセスできることも検証する。このテストは ProgramData の実ディレクトリを変更しない。

## 実機検証記録（2026-09-23）

| 項目 | 環境・結果 |
| --- | --- |
| OS と権限 | Windows 11 Home x64、build 26200。通常ユーザー SID `S-1-5-21-4133512733-519479502-335887275-1001`、非昇格を確認 |
| Host Adapter | Rust テストで ProgramData 既知フォルダー配下の絶対パスを解決。成功 |
| ACL 初期化 | 一時的な日本語・空白パスで `Test-ManagementRoot.ps1` 成功。承認後、実際の ProgramData に昇格セットアップを実施。全管理ディレクトリの所有者が指定利用者で、継承が無効、明示 ACE が指定利用者・SYSTEM・Administrators のみと確認。非昇格の通常利用者から `state` に書込み・読取り成功。GUI 自体の管理ファイル操作は未実装 |
| Docker Desktop bind | CLI／Engine 29.5.3、Compose v5.1.4。`alpine:latest` から日本語・空白を含む一時パスと ProgramData 管理ルート内の `data` 子領域へ書込み、ホストで `bind-ok` を確認。検証子領域は削除し、`state` の所有者は維持 |
| CLI 子プロセス終了 | `docker run` の CLI プロセスを終了できた。コンテナはその後も稼働していたため、検証コンテナを `docker rm -f` で削除。今後の Docker CLI Adapter では終了後の Engine 照合が必要 |
| 最低版 | 設計上の Windows 11 24H2 では未検証。今回の実機は build 26200 のみ |

通常 GUI からの管理ファイル操作と Windows 11 24H2 は、アプリ機能実装と対象環境での追加受入が必要。Docker CLI Adapter は現時点で未実装のため、アプリからの CLI 停止・再照合の試験はその実装時に行う。
