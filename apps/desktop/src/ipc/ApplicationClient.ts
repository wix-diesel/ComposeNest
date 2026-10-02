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
