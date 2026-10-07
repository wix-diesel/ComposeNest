/** Installs a read-only UI transport; form shapes come from the real Core fixture. */
export async function installValidationFixture(page, plans) {
  await page.addInitScript((plans) => {
    window.uiMode = "normal";
    window.uiCalls = [];
    const port = { slot: "db", hostIp: "127.0.0.1", hostPort: 15432, containerPort: 5432 };
    const state = { id: "target", name: "検証用データベース", revision: 3, lifecycle: "managed", runtimeStatus: "stopped",
      observedAt: "2026-10-07 01:00:00", operationId: null, operationStatus: null, operationKind: null, operationPhase: null,
      actions: ["rename", "start", "restart", "delete"] };
    const instance = { ...state, projectName: "cn-target", templateId: "example.test", templateVersion: "1.0.0", selectedVersion: "1",
      storageMethod: "bind", specRevision: 1, appliedSpecRevision: 1, ports: [port],
      storage: [{ slot: "data", method: "bind", presence: "present", initialization: "unknown" }],
      inputs: [{ slot: "password", label: "Password", secret: true, value: "never-render-secret" }],
      connections: [{ slot: "db", label: "Database", port, inputSlots: ["password"] }],
      observation: { runtimeState: "stopped", health: null, observedAt: state.observedAt, freshness: "fresh" }, lastOperation: null };
    const location = "/managed/data/target/data/long-path-for-responsive-validation";
    const catalog = { localRoot: "/managed/templates/local", results: [], templates: [
      { revisionId: "registered", templateId: "example.test", name: "検証用サービス", description: "Coreの汎用フォームを使用する検証用定義",
        templateVersion: "1.0.0", versions: ["1", "2"], storageMethods: ["bind", "volume"], origin: "local", loaded: true }] };
    for (const kind of ["create", "clone"]) {
      plans[kind].displayName = "検証先の環境";
      plans[kind].concerns = [];
    }
    let callbackId = 0;
    window.__TAURI_EVENT_PLUGIN_INTERNALS__ = { unregisterListener: () => {} };
    window.__TAURI_INTERNALS__ = { transformCallback: () => ++callbackId, invoke: async (command, args) => {
      if (command.startsWith("plugin:event|")) return 1;
      const request = args.request;
      window.uiCalls.push(command);
      const context = request.context ?? request;
      const response = (result) => ({ apiVersion: 1, requestId: context.requestId, result: structuredClone(result), error: null });
      if (command === "get_bootstrap") return response({ applicationTitle: "ComposeNest", startedAtUnixSeconds: 1 });
      if (command === "discard_create_plan" || command === "discard_clone_plan") return response(null);
      if (window.uiMode === "loading") await new Promise((resolve) => { window.releaseUi = resolve; });
      if (window.uiMode === "failure") throw new Error("private-backend-error");
      if (command === "list_instances") return response(window.uiMode === "empty" ? [] : [instance, { ...instance, id: "unknown", name: "未確認の環境", runtimeStatus: "unknown", observation: null }]);
      if (command === "list_templates") return response(window.uiMode === "empty" ? { ...catalog, templates: [] } : catalog);
      if (command === "get_settings") return response({ storageMethod: "bind", managementRoot: "/managed" });
      if (command === "diagnose_runtime") return response({ observedAt: 1791334800, checks: ["cli", "compose", "engine", "linux", "platform", "endpoint", "root"].map((name) => ({ name, status: "ready", version: null })),
        endpoint: "unix:///docker.sock", contextName: "local", platform: "linux/amd64", engineId: "engine", registeredEngineId: "engine", targetStatus: "verified", managementRoot: "/managed" });
      if (command === "get_instance_detail") return response({ state, instance, locations: [{ slot: "data", location }], creationStartedAt: state.observedAt, cloneSourceId: null });
      if (command === "get_instance_actions") return response(state);
      if (command === "get_instance_edit") return response({ state, ...instance, canEditPorts: true,
        ports: [{ ...port, committedPort: port.hostPort, oldPort: port.hostPort, candidatePort: null, oldReservation: "committed", candidateReservation: null }] });
      if (command === "list_retained_storage") return response(window.uiMode === "empty" ? [] : [{ instance: { ...instance, lifecycle: "retired" }, serviceName: "検証用サービス", deletedAt: state.observedAt,
        locations: [{ storage: instance.storage[0], location, ownership: "retained", ownershipVerified: true, observedAt: state.observedAt }],
        settingsDatabase: "/managed/state/composenest.sqlite", snapshotId: "snapshot", artifacts: [{ id: "artifact", specRevision: 1, placement: "retained", directory: "/managed/instances/target/artifacts/artifact" }] }]);
      if (command === "get_operation") return response({ operation: { id: "op", kind: "create", status: "Executing", phase: "image", startedAt: state.observedAt },
        instance: { ...instance, runtimeStatus: "preparing", lastOperation: { id: "op" } }, sequence: 1, completedAt: null });
      const kind = command.includes("clone") ? "clone" : "create";
      // Exercise focus restoration after the initiating fieldset has been disabled.
      if (command === `view_${kind}_plan`) await new Promise((resolve) => setTimeout(resolve, 25));
      if ([`prepare_${kind}`, `view_${kind}_plan`].includes(command)) return response(plans[kind]);
      if (command === `update_${kind}_plan`) {
        if (request.edit.displayName !== null) plans[kind].displayName = request.edit.displayName;
        plans[kind].planRevision++;
        return response(plans[kind]);
      }
      throw new Error(`Unexpected UI validation command: ${command}`);
    } };
  }, plans);
}
