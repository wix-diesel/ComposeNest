import type { ConfirmCreateRequest, CreatePlanView, CreateReceipt, PlanEdit } from "../generated/template-form";
import { invoke } from "@tauri-apps/api/core";
import { parseRoute, routeHash, type AppRoute } from "../navigation";
import {
  API_VERSION,
  type BootstrapRequest,
  type BootstrapResponse,
  type ResponseEnvelope,
} from "../generated/ipc";

/** Typed boundary between React features and the Tauri transport. */
export class ApplicationClient {
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
  private context() { return { apiVersion: API_VERSION, requestId: crypto.randomUUID() }; }

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
