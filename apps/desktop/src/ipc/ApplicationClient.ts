import type { ChangeInstanceRequest, InstanceActionView, RenameInstanceRequest } from "../generated/template-form";
import type { EditInstancePortsRequest, InstanceEditView } from "../generated/template-form";
import type { CloneEdit, ClonePlanView, ConfirmCloneRequest, ConfirmCreateRequest, CreatePlanView, CreateReceipt, PlanEdit } from "../generated/template-form";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import type { InstanceListView } from "../generated/template-form";
import { parseRoute, routeHash, type AppRoute } from "../navigation";
import {
  API_VERSION,
  type BootstrapRequest,
  type BootstrapResponse,
  type ResponseEnvelope,
} from "../generated/ipc";

/** Typed boundary between React features and the Tauri transport. */
export class ApplicationClient {
  /** Reads durable state; notifications and elapsed time never establish success. */
  async getOperation(operationId: string): Promise<import("../generated/template-form").OperationProgressView> {
    const view = await this.createCall<import("../generated/template-form").OperationProgressView>("get_operation", { context: this.context(), operationId } as import("../generated/template-form").OperationRequest);
    if (!view?.operation || !view.instance || view.operation.id !== operationId || !view.instance.id
      || !Array.isArray(view.instance.ports) || !Array.isArray(view.instance.connections)
      || !Number.isSafeInteger(view.sequence) || view.sequence < 0) throw new Error("invalid_operation_response");
    return view;
  }

  /** Registers non-sensitive invalidation hints; the caller releases the listener on close. */
  subscribeOperation(listener: (event: import("../generated/ipc").OperationEvent) => void): Promise<() => void> {
    return listen<import("../generated/ipc").OperationEvent>("operation-progress", ({ payload }) => listener(payload));
  }

  /** Subscribes to local navigation, including browser back and forward. */
  subscribeNavigation = (listener: () => void): (() => void) => {
    window.addEventListener("hashchange", listener);
    return () => window.removeEventListener("hashchange", listener);
  };

  /** Returns a stable snapshot for React's external store subscription. */
  getNavigationSnapshot = (): string => window.location.hash;

  /** Opens a typed screen without executing a business operation or reloading the page. */
  navigate(route: AppRoute): void {
    const hash = routeHash(route);
    if (routeHash(parseRoute(hash)) !== hash) {
      throw new Error("invalid_navigation_target");
    }
    window.location.hash = hash;
  }
  /** Reads package cards and the latest actual reload results without runtime operations. */
  listTemplates(): Promise<import("../generated/template-form").TemplateCatalogView> { return this.catalogCall("list_templates"); }

  /** Reloads fixed trusted directories without pulling images or starting environments. */
  reloadTemplates(): Promise<import("../generated/template-form").TemplateCatalogView> { return this.catalogCall("reload_templates"); }

  private async catalogCall(command: string): Promise<import("../generated/template-form").TemplateCatalogView> {
    const request = this.context();
    const response = await invoke<ResponseEnvelope<import("../generated/template-form").TemplateCatalogView>>(command, { request });
    if (response.apiVersion !== API_VERSION || response.requestId !== request.requestId
      || response.error !== null || response.result === null || typeof response.result.localRoot !== "string"
      || !Array.isArray(response.result.templates) || !Array.isArray(response.result.results)) throw new Error("template_catalog_failed");
    return response.result;
  }

  private context() { return { apiVersion: API_VERSION, requestId: crypto.randomUUID() }; }

  /** Reads saved retained observations without inspecting resources. */
  async listRetainedStorage(): Promise<import("../generated/template-form").RetainedInstance[]> {
    const request = this.context();
    const response = await invoke<ResponseEnvelope<import("../generated/template-form").RetainedInstance[]>>("list_retained_storage", { request });
    if (response.apiVersion !== API_VERSION || response.requestId !== request.requestId || response.error !== null
      || !Array.isArray(response.result)) throw new Error("retained_list_failed");
    return response.result;
  }

  /** Rechecks the original allocation using only its scoped instance identity. */
  async refreshRetainedStorage(instanceId: string): Promise<import("../generated/template-form").RetainedInstance> {
    const request = { context: this.context(), instanceId };
    const response = await invoke<ResponseEnvelope<import("../generated/template-form").RetainedInstance>>("refresh_retained_storage", { request });
    if (response.apiVersion !== API_VERSION || response.requestId !== request.context.requestId || response.error !== null
      || !response.result || response.result.instance.id !== instanceId) throw new Error("retained_refresh_failed");
    return response.result;
  }

  /** Reads the backend default and OS-resolved root without runtime operations. */
  getSettings(): Promise<import("../generated/template-form").SettingsView> {
    return this.settingsCall("get_settings", this.context());
  }

  /** Persists only the scoped default, accepting success after validated readback. */
  async saveSettings(storageMethod: import("../generated/template-form").StorageMethod): Promise<import("../generated/template-form").SettingsView> {
    const result = await this.settingsCall("save_settings", { context: this.context(), storageMethod });
    if (result.storageMethod !== storageMethod) throw new Error("settings_save_unconfirmed");
    return result;
  }

  private async settingsCall(command: string, request: import("../generated/ipc").RequestContext | import("../generated/template-form").SaveSettingsRequest): Promise<import("../generated/template-form").SettingsView> {
    const context = "context" in request ? request.context : request;
    const response = await invoke<ResponseEnvelope<import("../generated/template-form").SettingsView>>(command, { request });
    if (response.apiVersion !== API_VERSION || response.requestId !== context.requestId || response.error !== null
      || response.result === null || !["bind", "volume"].includes(response.result.storageMethod)
      || typeof response.result.managementRoot !== "string" || !response.result.managementRoot.trim()) throw new Error("settings_unavailable");
    return response.result;
  }

  /** Observes fixed runtime prerequisites without registering or changing a target. */
  async diagnoseRuntime(): Promise<import("../generated/template-form").RuntimeDiagnosis> {
    const request = this.context();
    const response = await invoke<ResponseEnvelope<import("../generated/template-form").RuntimeDiagnosis>>("diagnose_runtime", { request });
    if (response.apiVersion !== API_VERSION || response.requestId !== request.requestId || response.error !== null
      || response.result === null || !Array.isArray(response.result.checks)
      || !Number.isSafeInteger(response.result.observedAt)) throw new Error("runtime_diagnosis_failed");
    return response.result;
  }

  /** Reloads scoped saved list data, preserving its original observation timestamps. */
  async listInstances(): Promise<InstanceListView[]> {
    const request = this.context();
    const response = await invoke<ResponseEnvelope<InstanceListView[]>>("list_instances", { request });
    if (response.apiVersion !== API_VERSION || response.requestId !== request.requestId
      || response.error !== null || !Array.isArray(response.result)) throw new Error("instance_list_failed");
    return response.result;
  }

  private async createCall<T>(command: string, request: { context: { apiVersion: number; requestId: string } }): Promise<T> {
    const response = await invoke<ResponseEnvelope<T>>(command, { request });
    if (response.apiVersion !== API_VERSION || response.requestId !== request.context.requestId)
      throw new Error("invalid_create_response");
    if (response.error !== null) throw response.error;
    return response.result as T;
  }

  /** Prepares the selected registered revision without creating external resources. */
  prepareCreate(templateRevisionId: string): Promise<CreatePlanView> {
    return this.createCall("prepare_create", { context: this.context(), templateRevisionId } as import("../generated/template-form").PrepareCreateRequest);
  }

  /** Applies edits against the exact current plan revision. */
  updateCreate(planId: string, edit: PlanEdit): Promise<CreatePlanView> {
    return this.createCall("update_create_plan", { context: this.context(), planId, edit } as import("../generated/template-form").UpdateCreateRequest);
  }

  /** Refreshes candidates on the same plan without issuing a new plan. */
  viewCreate(planId: string): Promise<CreatePlanView> {
    return this.createCall("view_create_plan", { context: this.context(), planId } as import("../generated/template-form").CreatePlanRequest);
  }

  /** Submits explicit confirmation using a stable idempotency request. */
  confirmCreate(request: ConfirmCreateRequest): Promise<CreateReceipt> {
    return this.createCall("confirm_create", request);
  }

  /** Reconciles an uncertain acceptance through durable receipt lookup. */
  getCreateReceipt(planId: string): Promise<CreateReceipt | null> {
    return this.createCall("get_create_receipt", { context: this.context(), planId } as import("../generated/template-form").CreatePlanRequest);
  }

  /** Discards an unconfirmed in-memory plan only. */
  discardCreate(planId: string): Promise<null> {
    return this.createCall("discard_create_plan", { context: this.context(), planId } as import("../generated/template-form").CreatePlanRequest);
  }

  /** Prepares a configuration-only clone from its private Snapshot. */
  prepareClone(sourceId: string): Promise<ClonePlanView> {
    return this.createCall("prepare_clone", { context: this.context(), sourceId } as import("../generated/template-form").PrepareCloneRequest);
  }
  /** Applies source- and revision-guarded clone edits. */
  updateClone(planId: string, edit: CloneEdit): Promise<ClonePlanView> {
    return this.createCall("update_clone_plan", { context: this.context(), planId, edit } as import("../generated/template-form").UpdateCloneRequest);
  }
  /** Revalidates the same clone plan and committed source. */
  viewClone(planId: string): Promise<ClonePlanView> {
    return this.createCall("view_clone_plan", { context: this.context(), planId } as import("../generated/template-form").CreatePlanRequest);
  }
  /** Accepts explicit configuration-only and plaintext confirmations. */
  confirmClone(request: ConfirmCloneRequest): Promise<CreateReceipt> {
    return this.createCall("confirm_clone", request);
  }
  /** Looks up durable Clone acceptance without creating a new plan. */
  getCloneReceipt(planId: string): Promise<CreateReceipt | null> {
    return this.createCall("get_clone_receipt", { context: this.context(), planId } as import("../generated/template-form").CreatePlanRequest);
  }
  /** Discards an unconfirmed clone and its secret candidates. */
  discardClone(planId: string): Promise<null> {
    return this.createCall("discard_clone_plan", { context: this.context(), planId } as import("../generated/template-form").CreatePlanRequest);
  }

  private instanceChanges = new Map<string, { command: string; request: ChangeInstanceRequest | RenameInstanceRequest | EditInstancePortsRequest; readBack?: boolean; promise?: Promise<InstanceActionView> }>();

  /** Reads immutable masked settings and durable old/candidate reservations. */
  getInstanceEdit(instanceId: string): Promise<InstanceEditView> {
    return this.createCall("get_instance_edit", { context: this.context(), instanceId } as import("../generated/template-form").InstanceActionRequest);
  }
  /** Restores an uncertain port draft across navigation without creating another request. */
  getPendingInstancePorts(instanceId: string): Record<string, number> | null {
    const request = this.instanceChanges.get(instanceId)?.request;
    return request && "ports" in request ? { ...request.ports } : null;
  }
  /** Changes ports separately from the display name with a stable request identity. */
  editInstancePorts(instanceId: string, expectedRevision: number, expectedSpecRevision: number, ports: Record<string, number>): Promise<InstanceActionView> {
    return this.beginInstanceChange("edit_instance_ports", { context: this.context(), instanceId, expectedRevision, expectedSpecRevision, ports: { ...ports } });
  }

  /** Reads committed state and available actions without optimistic status changes. */
  getInstanceActions(instanceId: string): Promise<InstanceActionView> {
    return this.createCall("get_instance_actions", { context: this.context(), instanceId } as import("../generated/template-form").InstanceActionRequest);
  }
  /** Reads masked detail data and available actions from one scoped saved snapshot. */
  async getInstanceDetail(instanceId: string): Promise<import("../generated/template-form").InstanceDetailView> {
    const detail = await this.createCall<import("../generated/template-form").InstanceDetailView>("get_instance_detail", { context: this.context(), instanceId } as import("../generated/template-form").InstanceActionRequest);
    if (!detail || detail.instance.id !== instanceId || detail.state.id !== instanceId) throw new Error("invalid_instance_detail");
    return detail;
  }
  /** Reports an in-flight or uncertain change across screen navigation. */
  hasInstanceChange(instanceId: string): boolean { return this.instanceChanges.has(instanceId); }

  /** Restores the user's original name draft while an uncertain rename is pending. */
  getPendingInstanceName(instanceId: string): string | null {
    const request = this.instanceChanges.get(instanceId)?.request;
    return request && "name" in request ? request.name : null;
  }

  private async reconcileRename(request: RenameInstanceRequest): Promise<InstanceActionView> {
    const canonicalName = request.name.replace(/^\p{White_Space}+|\p{White_Space}+$/gu, "").normalize("NFC");
    const matches = (view: InstanceActionView) => view.id === request.instanceId
      && view.name === canonicalName && view.revision === request.expectedRevision + 1;
    const unconfirmed = () => ({ code: "INSTANCE_RENAME_UNCONFIRMED" });
    const current = await this.getInstanceActions(request.instanceId);
    if (matches(current)) return current;
    if (current.id !== request.instanceId || current.revision !== request.expectedRevision) throw unconfirmed();
    // An unchanged revision permits only the exact original optimistic-lock request.
    // If the first invocation won meanwhile, read back its result after the stale retry.
    try {
      const replayed = await this.createCall<InstanceActionView>("rename_instance", request);
      if (matches(replayed)) return replayed;
    } catch {
      const observed = await this.getInstanceActions(request.instanceId);
      if (matches(observed)) return observed;
    }
    throw unconfirmed();
  }

  private sendInstanceChange(instanceId: string): Promise<InstanceActionView> {
    const pending = this.instanceChanges.get(instanceId);
    if (!pending) return this.getInstanceActions(instanceId);
    if (pending.promise) return pending.promise;
    const call = pending.command === "edit_instance_ports"
      ? this.createCall<InstanceEditView>(pending.command, pending.request).then((view) => view.state)
      : pending.readBack && "name" in pending.request
        ? this.reconcileRename(pending.request) : this.createCall<InstanceActionView>(pending.command, pending.request);
    pending.promise = call.then((view) => {
      if ("action" in pending.request && pending.request.action === "delete"
        && (!view || view.id !== instanceId || view.operationKind !== "delete" || !view.operationId
          || !Number.isSafeInteger(view.revision) || view.revision <= pending.request.expectedRevision))
        throw new Error("invalid_delete_response");
      this.instanceChanges.delete(instanceId); return view;
    }).catch((error: unknown) => {
      const code = typeof error === "object" && error !== null && "code" in error ? error.code : null;
      if (["NAME_OR_REQUEST_CONFLICT", "INSTANCE_STALE", "INSTANCE_MISSING", "INSTANCE_INPUT_INVALID", "INSTANCE_ACTION_UNAVAILABLE", "PORT_EDIT_REJECTED", "PORT_EDIT_CONFLICT", "PORT_EDIT_NOT_ACCEPTED"].includes(String(code)))
        this.instanceChanges.delete(instanceId);
      else if (pending.command === "rename_instance") pending.readBack = true;
      throw error;
    }).finally(() => { pending.promise = undefined; });
    return pending.promise;
  }
  /** Reconciles the original request and releases a rename only after its name/version agree. */
  retryInstanceChange(instanceId: string): Promise<InstanceActionView> { return this.sendInstanceChange(instanceId); }

  private beginInstanceChange(command: string, request: ChangeInstanceRequest | RenameInstanceRequest | EditInstancePortsRequest): Promise<InstanceActionView> {
    if (this.hasInstanceChange(request.instanceId)) return Promise.reject(new Error("instance_change_pending"));
    this.instanceChanges.set(request.instanceId, { command, request });
    return this.sendInstanceChange(request.instanceId);
  }
  /** Accepts one fixed lifecycle action with a stable request ID until reconciliation. */
  changeInstance(instanceId: string, expectedRevision: number, action: string): Promise<InstanceActionView> {
    return this.beginInstanceChange("change_instance", { context: this.context(), instanceId, expectedRevision, action, retainDataConfirmed: false });
  }
  /** Deletes runtime resources while preserving data, using the explicitly confirmed revision. */
  deleteInstance(instanceId: string, expectedRevision: number): Promise<InstanceActionView> {
    return this.beginInstanceChange("change_instance", { context: this.context(), instanceId, expectedRevision, action: "delete", retainDataConfirmed: true });
  }
  /** Changes only the display name against the exact version shown by Core. */
  renameInstance(instanceId: string, expectedRevision: number, name: string): Promise<InstanceActionView> {
    return this.beginInstanceChange("rename_instance", { context: this.context(), instanceId, expectedRevision, name });
  }

  /** Loads the initial application state from the Rust application layer. */
  async getBootstrap(requestId: string): Promise<BootstrapResponse> {
    const request: BootstrapRequest = {
      apiVersion: API_VERSION,
      requestId,
    };
    const response = await invoke<ResponseEnvelope<BootstrapResponse>>(
      "get_bootstrap",
      { request },
    );

    const result = response.result;
    if (response.error !== null || result === null) {
      throw new Error(response.error?.reason ?? "bootstrap_failed");
    }
    return result;
  }
}
