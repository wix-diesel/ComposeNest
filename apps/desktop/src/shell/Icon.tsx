/** Mock shell icons rendered as React SVG without interpreting external HTML. */
export function Icon({ name }: { name: string }) {
  const paths: Record<string, string> = {
    nest: "m3 8 9-5 9 5-9 5zM3 12l9 5 9-5M3 16l9 5 9-5",
    templates: "m3 7 9-4 9 4-9 4zM3 12l9 4 9-4M3 17l9 4 9-4",
    retained: "M3 3h18v5H3zM5 8v12h14V8M10 12h4",
    diagnostics: "M2 12h5l3-8 4 16 3-8h5",
    settings: "m9 3-1 3-3 1-2 3 2 2-1 3 2 3 3-1 2 3h3l1-3 3-1 2-3-2-2 1-3-2-3-3 1-2-3z",
    home: "m3 10 9-7 9 7M5 9v12h14V9M9 21v-8h6v8",
  };
  return <svg aria-hidden="true" viewBox="0 0 24 24">
    {name === "instances" ? <><rect x="3" y="3" width="7" height="7" rx="1.5" /><rect x="14" y="3" width="7" height="7" rx="1.5" /><rect x="3" y="14" width="7" height="7" rx="1.5" /><rect x="14" y="14" width="7" height="7" rx="1.5" /></> : <path d={paths[name]} />}
    {name === "settings" && <circle cx="11.5" cy="11.5" r="3" />}
  </svg>;
}
