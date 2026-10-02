import { useEffect, useRef, useState, useSyncExternalStore } from "react";
import { ApplicationClient } from "./ipc/ApplicationClient";
import { ja } from "./messages";
import { mainPages, parentRoute, parseRoute, selectedNavigation, type AppRoute } from "./navigation";
import { ConfirmDialog } from "./shell/ConfirmDialog";
import { Icon } from "./shell/Icon";
import { Toast } from "./shell/Toast";

const applicationClient = new ApplicationClient();

/** Shared Japanese shell; each destination reserves content for its own feature issue. */
export function App({ client = applicationClient }: { client?: ApplicationClient }) {
  const hash = useSyncExternalStore(client.subscribeNavigation, client.getNavigationSnapshot);
  const route = parseRoute(hash);
  const selected = selectedNavigation(route);
  const parent = parentRoute(route);
  const heading = useRef<HTMLHeadingElement>(null);
  const [title, setTitle] = useState("ComposeNest");
  const [connection, setConnection] = useState<"loading" | "ready" | "failed">("loading");
  const [retry, setRetry] = useState(0);
  const [notice, setNotice] = useState<string | null>(null);
  const [about, setAbout] = useState(false);

  useEffect(() => {
    let active = true;
    setConnection("loading");
    void client.getBootstrap(crypto.randomUUID()).then((bootstrap) => {
      if (active) { setTitle(bootstrap.applicationTitle); setConnection("ready"); setNotice(null); }
    }).catch(() => {
      if (active) { setConnection("failed"); setNotice(ja.bootstrapFailed); }
    });
    return () => { active = false; };
  }, [client, retry]);
  useEffect(() => {
    document.title = `${ja.pages[route.page]} | ${title}`;
    heading.current?.focus();
    setAbout(false);
  }, [hash, title, route.page]);

  function nav(destination: AppRoute) {
    return <button className={`nav ${selected === destination.page ? "active" : ""}`}
      aria-label={ja.pages[destination.page]} aria-current={selected === destination.page ? "page" : undefined}
      onClick={() => client.navigate(destination)}>
      <Icon name={destination.page} /><span className="nav-text">{ja.pages[destination.page]}</span>
    </button>;
  }
  const crumbs: AppRoute[] = [];
  for (let ancestor = parent; ancestor !== null; ancestor = parentRoute(ancestor)) crumbs.unshift(ancestor);

  return <>
    <a className="skip-link" href="#screen" onClick={(event) => { event.preventDefault(); heading.current?.focus(); }}>{ja.skip}</a>
    <div className="app">
      <aside className="sidebar">
        <div className="brand"><span className="brand-mark"><Icon name="nest" /></span><span className="brand-text">{title}</span></div>
        <div className="nav-label">{ja.workspace}</div>
        <nav aria-label={ja.menu}>{mainPages.slice(0, 4).map((page) => <div key={page}>{nav({ page })}</div>)}</nav>
        <div className="sidebar-bottom">
          {nav({ page: "settings" })}
          <div className="engine-card">{ja.engineUnknown}<small>{ja.computer}</small></div>
          <div className="user"><span className="avatar"><Icon name="home" /></span><div>{ja.local}<br /><small>{ja.computer}</small></div></div>
        </div>
      </aside>
      <main className="main">
        <header className="topbar">
          <nav className="crumb" aria-label={ja.breadcrumb}>
            <button className="crumb-link" aria-label={ja.home} onClick={() => client.navigate({ page: "instances" })}><Icon name="home" /></button>
            {crumbs.map((item) => <span key={item.page}> / <button className="crumb-link" onClick={() => client.navigate(item)}>{ja.pages[item.page]}</button></span>)}
            <span aria-hidden="true">/</span><strong aria-current="page">{ja.pages[route.page]}</strong>
          </nav>
          <div className="topbar-right"><span>{ja.engineUnknown}</span><span>{connection === "ready" ? ja.ready : connection === "loading" ? ja.loading : ja.bootstrapFailed}</span>
            {connection === "failed" && <button className="btn small" onClick={() => setRetry((value) => value + 1)}>{ja.retry}</button>}
          </div>
        </header>
        <div className="workspace">
          <div className="page-heading"><div>
            <div className="eyebrow">{route.page === "settings" ? ja.application : ja.workspace}</div>
            <h1 id="screen" ref={heading} tabIndex={-1}>{ja.pages[route.page]}</h1>
            <p className="subtitle">{ja.subtitles[route.page]}</p>
          </div><div className="actions">
            {parent && <button className="btn" onClick={() => client.navigate(parent)}>{ja.back}</button>}
            <button className="btn" onClick={() => setAbout(true)}>{ja.about}</button>
          </div></div>
          <section className="panel" aria-label={ja.pages[route.page]} key={hash}>
            <p>{ja.unimplemented}</p>
            {"templateId" in route && <p>{ja.templateId}: <code>{route.templateId}</code></p>}
            {"instanceId" in route && route.instanceId && <p>{ja.target}: <code>{route.instanceId}</code></p>}
            {"operationId" in route && <p>{ja.operationId}: <code>{route.operationId}</code></p>}
          </section>
          <footer className="page-footer"><span>{title} / {ja.footer}</span><span>{ja.version}</span></footer>
        </div>
      </main>
    </div>
    {notice && <Toast message={notice} kind="error" onDismiss={() => setNotice(null)} />}
    {about && <ConfirmDialog title={ja.pages[route.page]} onClose={() => setAbout(false)}><p>{ja.unimplemented}</p></ConfirmDialog>}
  </>;
}
