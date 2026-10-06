import { useEffect, useRef, useState } from "react";
import type { TemplateCatalogView } from "../generated/template-form";
import type { ApplicationClient } from "../ipc/ApplicationClient";
import { Icon } from "../shell/Icon";
import "./template-list.css";

const sourceLabel = (source: string) => source === "bundled" ? "同梱" : source === "local" ? "ローカル" : "出所不明";

/** Package cards and actual reload outcomes, including retained registered revisions. */
export function TemplateList({ client, refresh, loadingChanged }: {
  client: ApplicationClient; refresh: number; loadingChanged: (loading: boolean) => void;
}) {
  const [view, setView] = useState<TemplateCatalogView | null>(null);
  const [loading, setLoading] = useState(true);
  const [failed, setFailed] = useState(false);
  const lastRefresh = useRef(refresh);
  useEffect(() => {
    let active = true;
    const reload = lastRefresh.current !== refresh;
    lastRefresh.current = refresh;
    setLoading(true); loadingChanged(true); setFailed(false); setView(null);
    void (reload ? client.reloadTemplates() : client.listTemplates()).then((result) => {
      if (active) setView(result);
    }).catch(() => { if (active) setFailed(true); }).finally(() => {
      if (active) { setLoading(false); loadingChanged(false); }
    });
    return () => { active = false; };
  }, [client, refresh, loadingChanged]);
  return <div className="template-list" aria-busy={loading}>
    <div className="template-hero"><Icon name="templates" /><div><h2>設定ファイルを書かずに、すぐ開発へ。</h2><p>接続ポートやデータ保存先は、ComposeNestが分かりやすく案内します。</p></div></div>
    {loading && <p className="panel" role="status">テンプレートを読込み中…</p>}
    {failed && <p className="notice warning" role="alert">テンプレート一覧の取得に失敗しました。管理ルートと定義へのアクセスを確認し、再読込みしてください。</p>}
    {view && <>
      {view.templates.length === 0 && <p className="panel" role="status">利用できるテンプレートはありません。ローカル定義を追加して再読込みしてください。</p>}
      <div className="template-cards">{view.templates.map((template) => <article className="template-card" key={template.revisionId}>
        <div className="template-card-top"><span className="template-service"><Icon name="templates" /></span><span className="badge">{sourceLabel(template.origin)}</span></div>
        <h2>{template.name}</h2><p>{template.description}</p>
        <div className="template-tags"><span className="badge">実機検証状態：未確認</span>{!template.loaded && <span className="badge">既存登録版（今回の読込みでは未登録）</span>}</div>
        <dl><div><dt>選択できるVersion</dt><dd>{template.versions.join(" / ")}</dd></div><div><dt>Template版</dt><dd>{template.templateVersion}</dd></div></dl>
        <div className="template-bottom"><small>{template.storageMethods.map((method) => method === "bind" ? "bind mount" : "named volume").join(" / ") || "永続化なし"}</small>
          <button className="btn primary" aria-label={`${template.name} ${template.templateVersion}のテンプレートを使う`} onClick={() => client.navigate({ page: "instance-create", templateId: template.revisionId, returnTo: "templates" })}>このテンプレートを使う</button></div>
      </article>)}</div>
      <div className="template-add"><Icon name="templates" /><div><strong>ローカルのテンプレートを追加できます</strong><p>追加先：<code>{view.localRoot}</code></p><p><code>&lt;package&gt;/template.yaml</code> と <code>&lt;package&gt;/versions/*.yaml</code> を配置し、再読込みしてください。読込みだけでイメージ取得や環境の起動は行いません。</p></div></div>
      <section className="panel template-results" aria-label="読込み結果"><h2>読込み結果</h2>
        <dl><div><dt>今回の読込み成功</dt><dd>{view.results.filter((result) => result.revisionId !== null).length} 件</dd></div><div><dt>読込みエラー</dt><dd>{view.results.filter((result) => result.error !== null).length} 件</dd></div></dl>
        <p>1つのVersionでも欠損・不正がある場合、パッケージ全体を登録できません。既存登録版は成功件数に含みません。</p>
        {view.results.map((result, index) => <div className="template-result" key={`${result.origin}:${result.package}:${index}`}><strong>{result.package}（{sourceLabel(result.origin)}）</strong>
          {result.error && <p className="template-error">{result.error}</p>}{result.warnings.map((warning) => <p key={warning}>{warning}</p>)}</div>)}
      </section>
    </>}
  </div>;
}
