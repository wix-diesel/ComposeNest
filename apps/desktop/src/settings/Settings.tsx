import { useEffect, useRef, useState } from "react";
import type { DisplayPreferences } from "../appearance";
import type { SettingsView, StorageMethod } from "../generated/template-form";
import type { ApplicationClient } from "../ipc/ApplicationClient";
import { ListViewChoices, ThemeChoices } from "../shell/DisplayChoices";
import { Icon } from "../shell/Icon";
import { Toast } from "../shell/Toast";
import "./settings.css";

const storageLabels = { bind: "ホストフォルダー", volume: "Docker管理" };

/** Settings surface; business defaults are read and written only through Core. */
export function Settings({ client, preferences, update }: {
  client: ApplicationClient;
  preferences: DisplayPreferences;
  update: <K extends keyof DisplayPreferences>(key: K, value: DisplayPreferences[K]) => void;
}) {
  const [view, setView] = useState<SettingsView | null>(null);
  const [draft, setDraft] = useState<StorageMethod | null>(null);
  const [loading, setLoading] = useState(true);
  const [saving, setSaving] = useState(false);
  const [failed, setFailed] = useState(false);
  const [reload, setReload] = useState(0);
  const [notice, setNotice] = useState<{ message: string; kind: "info" | "error" } | null>(null);
  const inFlight = useRef(false);

  useEffect(() => {
    let active = true;
    setLoading(true); setView(null); setDraft(null); setFailed(false); setNotice(null);
    void client.getSettings().then((result) => {
      if (active) { setView(result); setDraft(result.storageMethod); }
    }).catch(() => { if (active) setFailed(true); })
      .finally(() => { if (active) setLoading(false); });
    return () => { active = false; };
  }, [client, reload]);

  async function save() {
    if (!view || !draft || loading || inFlight.current) return;
    inFlight.current = true; setSaving(true); setNotice(null);
    try {
      const result = await client.saveSettings(draft);
      setView(result); setDraft(result.storageMethod);
      setNotice({ message: "設定を保存しました。以降の新規作成に適用します。", kind: "info" });
    } catch {
      setNotice({ message: "設定の保存を確認できませんでした。管理ルートへのアクセスを確認し、保存済み設定を再読込みしてから再試行してください。", kind: "error" });
    } finally { inFlight.current = false; setSaving(false); }
  }

  return <><form onSubmit={(event) => { event.preventDefault(); void save(); }}>
    <div className="settings-columns">
      <div>
        <section className="panel display-settings" aria-label="デザイン">
          <h2>デザイン</h2><p>表示設定は選択するとすぐに適用・保存されます。</p>
          <div><h3>カラーテーマ</h3><ThemeChoices value={preferences.theme} onChange={(value) => update("theme", value)} /></div>
          <div><h3>一覧表示</h3><ListViewChoices value={preferences.listView} onChange={(value) => update("listView", value)} /></div>
        </section>
        <section className="panel">
          <h2>新しい環境の既定値</h2>
          <fieldset className="settings-storage" disabled={loading || saving || !view}>
            <legend>データの保存方式</legend>
            <div className="settings-options">{(["bind", "volume"] as const).map((method) => <label key={method}>
              <input type="radio" name="storage" value={method} checked={draft === method} onChange={() => { setDraft(method); setNotice(null); }} />
              <span>{storageLabels[method]}<small>{method === "bind" ? "bind mount · 管理ルート内に保存" : "named volume · 専用領域に保存"}</small></span>
            </label>)}</div>
          </fieldset>
          <p className="settings-footnote">以降の新規作成に適用します。既存の環境のデータは移動しません。複製時は元環境の方式を初期選択し、複製先では変更できます。</p>
          {failed && <p className="notice warning" role="alert">設定を取得できませんでした。管理ルートへのアクセスを確認し、再読込みしてください。</p>}
          <button type="button" className="btn small" disabled={loading || saving} onClick={() => setReload((value) => value + 1)}>保存済み設定を再読込み</button>
        </section>
        <section className="panel">
          <h2>管理ルート</h2><p>設定・生成Compose・ホスト側のデータを保存する場所です。</p>
          <div className="settings-path"><code>{view?.managementRoot ?? (loading ? "確認中…" : "未取得")}</code></div>
          <p className="settings-footnote">OSが解決した保存先です。管理ルートの変更はv1では提供しません。</p>
        </section>
        <section className="panel">
          <h2>アプリについて</h2>
          {[["表示言語", "v1は日本語に対応しています。", "日本語"], ["アプリの更新", "新しい配布パッケージを使って手動で更新します。", "手動更新"], ["終了時の動作", "アプリを閉じても、起動済みの環境は停止しません。", null]].map(([title, description, tag]) => <div className="settings-row" key={title}>
            <div><h3>{title}</h3><p>{description}</p></div>{tag && <span className="settings-tag">{tag}</span>}
          </div>)}
        </section>
        <div className="settings-footer">
          <small>{view ? `最後に確認した保存済み設定：${storageLabels[view.storageMethod]}。再起動後も保持されます。` : loading ? "設定を読み込み中…" : "保存済み設定は未確認です。"}</small>
          <button type="submit" className="btn primary" disabled={loading || saving || !view}>{saving ? "保存中…" : "設定を保存"}</button>
        </div>
      </div>
      <aside>
        <div className="notice warning settings-security"><Icon name="info" /><div><strong>認証情報は暗号化せず保存</strong><p>設定と生成されたComposeファイルを読める利用者は、パスワードも取得できます。</p></div></div>
        <section className="panel"><h2>ComposeNest</h2><dl className="settings-summary">
          {[["対象", "ローカル開発"], ["画面仕様", "v1"], ["ライセンス", "Apache-2.0"]].map(([term, value]) => <div key={term}><dt>{term}</dt><dd>{value}</dd></div>)}
        </dl></section>
      </aside>
    </div>
  </form>{notice && <Toast {...notice} onDismiss={() => setNotice(null)} />}</>;
}
