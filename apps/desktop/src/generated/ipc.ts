/** Generated from the Rust IPC DTOs. Do not edit by hand. */

export const API_VERSION = 1 as const;

export interface RequestContext {
  apiVersion: number;
  requestId: string;
}

export type BootstrapRequest = RequestContext;

export interface BootstrapResponse {
  applicationTitle: string;
  startedAtUnixSeconds: number;
}

export type Retryability = "retryable" | "notRetryable" | "unknown";

export interface ErrorDto {
  code: string;
  fieldPath: string | null;
  reason: string;
  retryability: Retryability;
  operationId: string | null;
  safeDetails: string | null;
}

export interface ResponseEnvelope<T> {
  apiVersion: number;
  requestId: string;
  result: T | null;
  error: ErrorDto | null;
}

export interface OperationEvent {
  operationId: string;
  instanceId: string | null;
  revision: number;
  sequence: number;
  kind: "accepted" | "progress" | "completed" | "failed";
}

export interface EventEnvelope<T> {
  apiVersion: number;
  event: T;
}
