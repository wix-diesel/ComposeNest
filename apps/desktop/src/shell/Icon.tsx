/** Mock shell icons rendered as React SVG without interpreting external HTML. */
export function Icon({ name }: { name: string }) {
  const paths: Record<string, string> = {
    nest: "m3 8 9-5 9 5-9 5zM3 12l9 5 9-5M3 16l9 5 9-5",
    templates: "m3 7 9-4 9 4-9 4zM3 12l9 4 9-4M3 17l9 4 9-4",
    retained: "M3 3h18v5H3zM5 8v12h14V8M10 12h4",
    diagnostics: "M2 12h5l3-8 4 16 3-8h5",
    settings: "m9 3-1 3-3 1-2 3 2 2-1 3 2 3 3-1 2 3h3l1-3 3-1 2-3-2-2 1-3-2-3-3 1-2-3z",
    home: "m3 10 9-7 9 7M5 9v12h14V9M9 21v-8h6v8",
    refresh: "M20 7v5h-5M4 17v-5h5M6 6a8 8 0 0 1 13 2M5 16a8 8 0 0 0 13 2",
    plus: "M12 5v14M5 12h14",
    play: "m8 5 11 7-11 7z",
    stop: "M6 6h12v12H6z",
    info: "M12 11v6M12 7h.01M12 3a9 9 0 1 0 0 18 9 9 0 0 0 0-18",
    search: "M21 21l-5-5M10 3a7 7 0 1 0 0 14 7 7 0 0 0 0-14",
    arrow: "M4 12h16m-6-6 6 6-6 6",
  };
  return <svg aria-hidden="true" viewBox="0 0 24 24">
    {name === "instances" ? <><rect x="3" y="3" width="7" height="7" rx="1.5" /><rect x="14" y="3" width="7" height="7" rx="1.5" /><rect x="3" y="14" width="7" height="7" rx="1.5" /><rect x="14" y="14" width="7" height="7" rx="1.5" /></> : <path d={paths[name]} />}
    {name === "settings" && <circle cx="11.5" cy="11.5" r="3" />}
  </svg>;
}
