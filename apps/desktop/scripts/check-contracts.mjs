import { readFile } from "node:fs/promises";

const generated = await readFile(new URL("../src/generated/ipc.ts", import.meta.url), "utf8");
const rustDtos = await readFile(
  new URL("../../../crates/application/src/lib.rs", import.meta.url),
  "utf8",
);

const requiredContractFragments = [
  "export const API_VERSION = 1",
  "apiVersion: number",
  "requestId: string",
  "applicationTitle: string",
  "startedAtUnixSeconds: number",
  "fieldPath: string | null",
  "retryability: Retryability",
  "operationId: string | null",
  "safeDetails: string | null",
  "sequence: number",
];

const missing = requiredContractFragments.filter((fragment) => !generated.includes(fragment));
const rustContractFragments = [
  "pub const API_VERSION: u16 = 1",
  "pub api_version: u16",
  "pub request_id: String",
  "pub application_title: &'static str",
  "pub started_at_unix_seconds: u64",
  "pub field_path: Option<String>",
  "pub retryability: Retryability",
  "pub operation_id: Option<String>",
  "pub safe_details: Option<String>",
  "pub sequence: u64",
];

const missingRust = rustContractFragments.filter((fragment) => !rustDtos.includes(fragment));
if (missing.length > 0) {
  console.error(`IPC contract is out of date. Missing: ${missing.join(", ")}`);
  process.exitCode = 1;
}
if (missingRust.length > 0) {
  console.error(`Rust IPC DTOs changed unexpectedly. Missing: ${missingRust.join(", ")}`);
  process.exitCode = 1;
}
