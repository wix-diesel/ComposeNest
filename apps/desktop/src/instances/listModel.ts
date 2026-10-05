import type { InstanceListView } from "../generated/template-form";

/** Mutually exclusive list buckets; uncertain state always requires attention. */
export type InstanceFilter = "all" | "ready" | "stopped" | "attention";
/** Display projection shared by cards and the subsequent grid implementation. */
export interface InstanceListItem {
  source: InstanceListView;
  service: string;
  state: Exclude<InstanceFilter, "all">;
  endpoints: string;
  storage: string;
  configuration: string;
  observed: string;
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
