import type { InstanceListView } from "../generated/template-form";

/** Mutually exclusive list buckets; uncertain state always requires attention. */
export type InstanceFilter = "all" | "ready" | "stopped" | "attention";
/** Display projection shared by cards and the grid. */
export interface InstanceListItem {
  source: InstanceListView;
  service: string;
  state: Exclude<InstanceFilter, "all">;
  endpoints: string;
  storage: string;
  configuration: string;
  observed: string;
}

/** Display labels also used to sort the visible state column. */
export const instanceStateLabels = { ready: "利用可能", stopped: "停止中", attention: "要確認" };

/** Active grid column and its ordering direction. */
export interface InstanceSort {
  key: "name" | "state" | "port";
  direction: "ascending" | "descending";
}

/** Projects committed fields without inferring Docker observation or configuration equality. */
export function listItem(source: InstanceListView): InstanceListItem {
  const observed = source.observation;
  const known = observed?.freshness === "fresh";
  const configurationApplied = source.specRevision === source.appliedSpecRevision;
  const state = !known || source.needsAttention || !configurationApplied
    ? "attention" : source.runtimeStatus === "ready" ? "ready"
      : source.runtimeStatus === "stopped" ? "stopped" : "attention";
  return {
    source, state,
    service: ({ postgresql: "PostgreSQL", redis: "Redis" } as Record<string, string>)[source.templateId] ?? source.templateId,
    endpoints: source.ports.map((port) => `${port.hostIp.includes(":") ? `[${port.hostIp}]` : port.hostIp}:${port.hostPort}`).join(" / ") || "割当てなし",
    storage: ({ bind: "bind mount", volume: "named volume" } as Record<string, string>)[source.storageMethod] ?? source.storageMethod,
    configuration: configurationApplied ? "保存構成は適用済み" : "適用状況の確認が必要",
    observed: observed ? `${observed.observedAt} UTC${known ? "" : "（過去の観測）"}` : "未観測",
  };
}

/** Uses the same projected rows for search, filters and counts. */
export function selectInstances(items: InstanceListItem[], search: string, filter: InstanceFilter): InstanceListItem[] {
  const query = search.trim().normalize("NFKC").toLocaleLowerCase("ja");
  return items.filter((item) => (filter === "all" || item.state === filter)
    && `${item.source.name} ${item.service} ${item.source.templateId}`.normalize("NFKC").toLocaleLowerCase("ja").includes(query));
}

/** Sorts a copy; port bindings compare numerically, with unassigned rows always last. */
export function sortInstances(items: InstanceListItem[], sort: InstanceSort | null): InstanceListItem[] {
  if (!sort) return [...items];
  const direction = sort.direction === "ascending" ? 1 : -1;
  if (sort.key === "port") {
    const rows = items.map((item) => ({ item, ports: item.source.ports.map((port) => port.hostPort).sort((a, b) => a - b) }));
    rows.sort((a, b) => {
      if (!a.ports.length || !b.ports.length) return Number(!a.ports.length) - Number(!b.ports.length);
      for (let index = 0; index < Math.min(a.ports.length, b.ports.length); index++) {
        const order = a.ports[index] - b.ports[index];
        if (order) return direction * order;
      }
      return direction * (a.ports.length - b.ports.length);
    });
    return rows.map((row) => row.item);
  }
  return [...items].sort((a, b) => direction * (sort.key === "name"
    ? a.source.name.localeCompare(b.source.name, "ja")
    : instanceStateLabels[a.state].localeCompare(instanceStateLabels[b.state], "ja")));
}
