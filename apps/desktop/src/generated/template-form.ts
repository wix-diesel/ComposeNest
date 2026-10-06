/** Generated from Rust plan/form DTOs by scripts/generate-form-contract.mjs. */

import type { RequestContext } from "./ipc";

export type JsonValue = null | boolean | number | string | Array<JsonValue> | { [key: string]: JsonValue };

export interface RetainedLocation {
  storage: StorageView;
  location: string;
  ownership: string;
  ownershipVerified: boolean;
  observedAt: string | null;
}

export interface RetainedArtifact {
  id: string;
  specRevision: number;
  placement: string;
  directory: string | null;
}

export interface RetainedInstance {
  instance: InstanceView;
  serviceName: string;
  deletedAt: string;
  locations: Array<RetainedLocation>;
  settingsDatabase: string;
  snapshotId: string;
  artifacts: Array<RetainedArtifact>;
}

export interface SettingsView {
  storageMethod: StorageMethod;
  managementRoot: string;
}

export interface SaveSettingsRequest {
  context: RequestContext;
  storageMethod: StorageMethod;
}

export interface DiagnosticCheck {
  name: string;
  status: string;
  version: string | null;
}

export interface RuntimeDiagnosis {
  observedAt: number;
  checks: Array<DiagnosticCheck>;
  endpoint: string | null;
  contextName: string | null;
  platform: string | null;
  engineId: string | null;
  registeredEngineId: string | null;
  targetStatus: string;
  managementRoot: string;
}

export interface TemplateCard {
  revisionId: string;
  templateId: string;
  name: string;
  description: string;
  templateVersion: string;
  versions: Array<string>;
  storageMethods: Array<StorageMethod>;
  origin: string;
  loaded: boolean;
}

export interface TemplateLoadResult {
  package: string;
  origin: string;
  revisionId: string | null;
  error: string | null;
  warnings: Array<string>;
}

export interface TemplateCatalogView {
  localRoot: string;
  templates: Array<TemplateCard>;
  results: Array<TemplateLoadResult>;
}

export interface PortView {
  slot: string;
  hostIp: string;
  hostPort: number;
  containerPort: number;
}

export interface StorageView {
  slot: string;
  method: string;
  presence: string;
  initialization: string;
}

export interface SavedInputView {
  slot: string;
  label: string;
  secret: boolean;
  value: JsonValue | null;
}

export interface ConnectionView {
  slot: string;
  label: string;
  port: PortView;
  inputSlots: Array<string>;
}

export interface ObservationView {
  runtimeState: string;
  health: string | null;
  observedAt: string;
  freshness: string;
}

export interface OperationView {
  id: string;
  kind: string;
  status: string;
  phase: string;
  startedAt: string;
}

export interface InstanceView {
  id: string;
  name: string;
  revision: number;
  lifecycle: string;
  projectName: string;
  templateId: string;
  templateVersion: string;
  selectedVersion: string;
  image: string | null;
  storageMethod: string;
  specRevision: number;
  appliedSpecRevision: number | null;
  ports: Array<PortView>;
  storage: Array<StorageView>;
  inputs: Array<SavedInputView>;
  connections: Array<ConnectionView>;
  observation: ObservationView | null;
  runtimeStatus: string;
  lastOperation: OperationView | null;
  needsAttention: boolean;
}

export interface InstanceListView {
  id: string;
  name: string;
  templateId: string;
  selectedVersion: string;
  ports: Array<PortView>;
  storageMethod: string;
  specRevision: number;
  appliedSpecRevision: number | null;
  runtimeStatus: string;
  needsAttention: boolean;
  observation: ObservationView | null;
}

export interface StorageLocationView {
  slot: string;
  location: string;
}

export interface InstanceDetailView {
  instance: InstanceView;
  state: InstanceActionView;
  locations: Array<StorageLocationView>;
  creationStartedAt: string | null;
  cloneSourceId: string | null;
}

export interface EditInstancePortsRequest {
  context: RequestContext;
  instanceId: string;
  expectedRevision: number;
  expectedSpecRevision: number;
  ports: Record<string, number>;
}

export interface EditSettingView {
  slot: string;
  secret: boolean;
  value: JsonValue | null;
}

export interface EditPortView {
  slot: string;
  hostIp: string;
  containerPort: number;
  oldPort: number;
  committedPort: number;
  candidatePort: number | null;
  oldReservation: string;
  candidateReservation: string | null;
}

export interface InstanceEditView {
  state: InstanceActionView;
  templateId: string;
  selectedVersion: string;
  storageMethod: string;
  specRevision: number;
  appliedSpecRevision: number | null;
  inputs: Array<EditSettingView>;
  ports: Array<EditPortView>;
  canEditPorts: boolean;
}

export interface InstanceActionRequest {
  context: RequestContext;
  instanceId: string;
}

export interface RenameInstanceRequest {
  context: RequestContext;
  instanceId: string;
  expectedRevision: number;
  name: string;
}

export interface ChangeInstanceRequest {
  context: RequestContext;
  instanceId: string;
  expectedRevision: number;
  action: string;
}

export interface InstanceActionView {
  id: string;
  name: string;
  revision: number;
  runtimeStatus: string;
  observedAt: string | null;
  operationId: string | null;
  operationStatus: string | null;
  operationKind: string | null;
  operationPhase: string | null;
  actions: Array<string>;
}

export interface FormInput {
  key: string;
  label: string;
  inputType: string;
  required: boolean;
  description: string | null;
  validation: JsonValue;
  options: Array<FormOption>;
  canGenerate: boolean;
}

export interface FormOption {
  value: string;
  label: string;
}

export interface FormSlot {
  key: string;
  label: string;
  container: JsonValue;
}

export interface FormConnection {
  key: string;
  label: string;
  port: string;
  inputs: Array<string>;
}

export interface TemplateForm {
  name: string;
  templateVersion: string;
  description: string;
  inputs: Array<FormInput>;
  ports: Array<FormSlot>;
  storage: Array<FormSlot>;
  connections: Array<FormConnection>;
}

export interface PrepareCreateRequest {
  context: RequestContext;
  templateRevisionId: string;
}

export interface CreatePlanRequest {
  context: RequestContext;
  planId: string;
}

export interface UpdateCreateRequest {
  context: RequestContext;
  planId: string;
  edit: PlanEdit;
}

export interface ConfirmCreateRequest {
  context: RequestContext;
  planId: string;
  revision: number;
  confirmedPorts: Record<string, number>;
  acceptPlaintext: boolean;
}

export interface CreateReceipt {
  planId: string;
  confirmedRevision: number;
  operationId: string;
  instanceId: string;
}

export interface InputView {
  key: string;
  definition: JsonValue;
  value: JsonValue | null;
  hasSecret: boolean;
}

export interface PlanConcern {
  code: string;
  fieldPath: string;
}

export interface CreatePlanView {
  planId: string;
  planRevision: number;
  displayName: string;
  templateRevisionId: string;
  templateOrigin: string;
  version: string;
  versions: Array<string>;
  form: JsonValue;
  templateForm: TemplateForm;
  storageMethod: StorageMethod;
  inputs: Array<InputView>;
  ports: Record<string, number>;
  storageSlots: Array<string>;
  concerns: Array<PlanConcern>;
}

export interface PlanEdit {
  expectedRevision: number;
  displayName: string | null;
  version: string | null;
  storageMethod: StorageMethod | null;
  inputs: Record<string, JsonValue>;
  regenerateSecrets: Array<string>;
  ports: Record<string, string | null>;
}

export interface PrepareCloneRequest {
  context: RequestContext;
  sourceId: string;
}

export interface UpdateCloneRequest {
  context: RequestContext;
  planId: string;
  edit: CloneEdit;
}

export interface ConfirmCloneRequest {
  context: RequestContext;
  planId: string;
  revision: number;
  confirmedPorts: Record<string, number>;
  acceptPlaintext: boolean;
  acceptConfigurationOnly: boolean;
}

export interface InputDiff {
  key: string;
  definition: JsonValue | null;
  source: JsonValue | null;
  sourceHasSecret: boolean;
  sourceLabel: string;
  candidate: JsonValue | null;
  policy: string;
  canCopy: boolean;
  origin: ValueOrigin;
  changed: boolean;
  added: boolean;
  removed: boolean;
  needsAnswer: boolean;
  needsSecretConfirmation: boolean;
  hasSecret: boolean;
}

export interface ClonePlanView {
  planId: string;
  planRevision: number;
  sourceId: string;
  sourceName: string;
  sourceRevision: number;
  instanceId: string;
  projectName: string;
  displayName: string;
  version: string;
  sourceVersion: string;
  versions: Array<string>;
  storageMethod: StorageMethod;
  sourceStorageMethod: StorageMethod;
  templateForm: TemplateForm;
  inputs: Array<InputDiff>;
  ports: Record<string, number>;
  sourcePorts: Record<string, number>;
  explicitPorts: Array<string>;
  addedPorts: Array<string>;
  removedPorts: Array<string>;
  storageSlots: Array<string>;
  addedStorage: Array<string>;
  removedStorage: Array<string>;
  concerns: Array<PlanConcern>;
}

export interface CloneEdit {
  expectedRevision: number;
  displayName: string | null;
  version: string | null;
  storageMethod: StorageMethod | null;
  inputs: Record<string, CloneAnswer>;
  confirmSecrets: Array<string>;
  ports: Record<string, string | null>;
}

export type StorageMethod = "bind" | "volume";

export type ValueOrigin = "inherited" | "generated" | "user_input" | "unset";

export type CloneAnswer = { action: "copy" | "generate" | "clear" } | { action: "input"; value: JsonValue };
