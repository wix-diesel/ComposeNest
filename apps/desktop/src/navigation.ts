/** Primary destinations shared by the sidebar and screen hierarchy. */
export type MainPage = "instances" | "templates" | "retained" | "diagnostics" | "settings";
/** Screen identity; business state and secrets are never stored in navigation. */
export type AppRoute =
  | { page: MainPage }
  | { page: "instance-create"; templateId: string; returnTo?: "instances" | "templates" }
  | { page: "instance-detail" | "instance-clone" | "instance-edit"; instanceId: string }
  | { page: "operation"; operationId: string; instanceId?: string };

/** The five primary destinations in mock navigation order. */
export const mainPages: readonly MainPage[] = ["instances", "templates", "retained", "diagnostics", "settings"];
const instancePages = ["instance-detail", "instance-clone", "instance-edit"] as const;
const validId = (value: string | null): value is string =>
  value !== null && value.length > 0 && value.length <= 200 && !/[\u0000-\u001f\u007f]/u.test(value);

/** Parses a link defensively, falling back to the list for missing or invalid targets. */
export function parseRoute(hash: string): AppRoute {
  const [path, query = ""] = hash.replace(/^#\/?/, "").split("?");
  const params = new URLSearchParams(query);
  const instanceId = params.get("instanceId");
  if (mainPages.includes(path as MainPage)) return { page: path as MainPage };
  if (instancePages.includes(path as typeof instancePages[number]) && validId(instanceId))
    return { page: path as typeof instancePages[number], instanceId };
  const templateId = params.get("templateId");
  const returnTo = params.get("returnTo");
  if (path === "instance-create" && validId(templateId)
    && (returnTo === null || returnTo === "instances" || returnTo === "templates"))
    return { page: path, templateId, ...(returnTo === null ? {} : { returnTo }) };
  const operationId = params.get("operationId");
  if (path === "operation" && validId(operationId) && (instanceId === null || validId(instanceId)))
    return { page: path, operationId, ...(instanceId === null ? {} : { instanceId }) };
  return { page: "instances" };
}

/** Encodes only the typed destination and target identifiers into a local link. */
export function routeHash(route: AppRoute): string {
  const params = new URLSearchParams();
  if ("templateId" in route) params.set("templateId", route.templateId);
  if ("returnTo" in route && route.returnTo !== undefined) params.set("returnTo", route.returnTo);
  if ("operationId" in route) params.set("operationId", route.operationId);
  if ("instanceId" in route && route.instanceId !== undefined) params.set("instanceId", route.instanceId);
  return `#/${route.page}${params.size ? `?${params}` : ""}`;
}

/** Returns the sidebar section containing the destination. */
export function selectedNavigation(route: AppRoute): MainPage {
  if (mainPages.includes(route.page as MainPage)) return route.page as MainPage;
  return route.page === "instance-create" ? route.returnTo ?? "instances" : "instances";
}

/** Returns a deterministic parent, independent of prior visits or browser history. */
export function parentRoute(route: AppRoute): AppRoute | null {
  if (mainPages.includes(route.page as MainPage)) return null;
  if (route.page === "instance-create") return { page: route.returnTo ?? "instances" };
  if ("instanceId" in route && route.instanceId && route.page !== "instance-detail")
    return { page: "instance-detail", instanceId: route.instanceId };
  return { page: "instances" };
}
