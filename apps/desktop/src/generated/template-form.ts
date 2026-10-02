/** Generated from Rust plan/form DTOs by scripts/generate-form-contract.mjs. */

import type { RequestContext } from "./ipc";

export type JsonValue = null | boolean | number | string | Array<JsonValue> | { [key: string]: JsonValue };

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

export interface InputDiff {
  key: string;
  definition: JsonValue | null;
  source: JsonValue | null;
  candidate: JsonValue | null;
  policy: string;
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
