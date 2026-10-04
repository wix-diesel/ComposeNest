import { readFile, writeFile } from "node:fs/promises";

// This deliberately supports only the simple public DTO shapes listed below.
// Fail closed on an unfamiliar Rust field type or Serde attribute.
const sources = {
  instance_edit: ["EditInstancePortsRequest", "EditSettingView", "EditPortView", "InstanceEditView"],
  instance_actions: ["InstanceActionRequest", "RenameInstanceRequest", "ChangeInstanceRequest", "InstanceActionView"],
  template_form: ["FormInput", "FormOption", "FormSlot", "FormConnection", "TemplateForm"],
  create_session: ["PrepareCreateRequest", "CreatePlanRequest", "UpdateCreateRequest", "ConfirmCreateRequest", "CreateReceipt"],
  create_plan: ["InputView", "PlanConcern", "CreatePlanView", "PlanEdit"],
  clone_session: ["PrepareCloneRequest", "UpdateCloneRequest", "ConfirmCloneRequest"],
  clone_plan: ["InputDiff", "ClonePlanView", "CloneEdit"],
};
const primitives = { String: "string", "&'static str": "string", bool: "boolean", u16: "number", u64: "number", Value: "JsonValue" };
const camel = (name) => name.replace(/_([a-z])/g, (_, letter) => letter.toUpperCase());
function typeOf(rust) {
  if (rust === "crate::template_form::TemplateForm") return "TemplateForm";
  if (rust === "RequestContext") return "RequestContext";
  if (primitives[rust]) return primitives[rust];
  if (/^Option<(.+)>$/.test(rust)) return `${typeOf(rust.slice(7, -1))} | null`;
  if (/^Vec<(.+)>$/.test(rust)) return `Array<${typeOf(rust.slice(4, -1))}>`;
  const map = rust.match(/^BTreeMap<String, (.+)>$/);
  if (map) return `Record<string, ${typeOf(map[1])}>`;
  if ([...Object.values(sources).flat(), "StorageMethod", "ValueOrigin", "CloneAnswer"].includes(rust)) return rust;
  throw new Error(`Unsupported DTO type: ${rust}`);
}
let output = "/** Generated from Rust plan/form DTOs by scripts/generate-form-contract.mjs. */\n\n";
output += 'import type { RequestContext } from "./ipc";\n\n';
output += "export type JsonValue = null | boolean | number | string | Array<JsonValue> | { [key: string]: JsonValue };\n\n";
for (const [file, names] of Object.entries(sources)) {
  const source = await readFile(new URL(`../../../crates/application/src/${file}.rs`, import.meta.url), "utf8");
  for (const name of names) {
    const match = source.match(new RegExp(`#\\[serde\\(rename_all = "camelCase"(?:, deny_unknown_fields)?\\)\\]\\npub struct ${name} \\{([^}]+)\\}`, "s"));
    if (!match) throw new Error(`Missing camelCase DTO: ${name}`);
    const fields = match[1].split("\n").map((line) => line.trim()).filter(Boolean);
    output += `export interface ${name} {\n`;
    for (const field of fields) {
      if (field.startsWith("///") || field === "#[serde(default)]") continue;
      const parsed = field.match(/^pub (\w+): (.+),$/);
      if (!parsed) throw new Error(`Unsupported DTO field: ${field}`);
      output += `  ${camel(parsed[1])}: ${typeOf(parsed[2])};\n`;
    }
    output += "}\n\n";
  }
}
for (const [file, name, casing] of [["state_store", "StorageMethod", "snake_case"], ["clone_plan", "ValueOrigin", "snake_case"]]) {
  const source = await readFile(new URL(`../../../crates/application/src/${file}.rs`, import.meta.url), "utf8");
  const match = source.match(new RegExp(`#\\[serde\\(rename_all = "${casing}"\\)\\]\\npub enum ${name} \\{([^}]+)\\}`, "s"));
  if (!match) throw new Error(`Missing DTO enum: ${name}`);
  const variants = [...match[1].matchAll(/^    (\w+),$/gm)].map((item) => JSON.stringify(item[1].replace(/([a-z])([A-Z])/g, "$1_$2").toLowerCase()));
  if (match[1].split("\n").some((line) => line.trim() && !line.trim().startsWith("///") && !/^    \w+,$/.test(line))) throw new Error(`Unsupported enum: ${name}`);
  output += `export type ${name} = ${variants.join(" | ")};\n\n`;
}
const cloneSource = await readFile(new URL("../../../crates/application/src/clone_plan.rs", import.meta.url), "utf8");
if (!/#\[serde\(tag = "action", content = "value", rename_all = "snake_case"\)\]\npub enum CloneAnswer \{\s*(?:\/\/\/[^\n]+\n\s*)?Copy,\s*(?:\/\/\/[^\n]+\n\s*)?Generate,\s*(?:\/\/\/[^\n]+\n\s*)?Input\(Value\),\s*(?:\/\/\/[^\n]+\n\s*)?Clear,\s*\}/.test(cloneSource)) throw new Error("CloneAnswer contract changed");
output += 'export type CloneAnswer = { action: "copy" | "generate" | "clear" } | { action: "input"; value: JsonValue };\n';
const destination = new URL("../src/generated/template-form.ts", import.meta.url);
if (process.argv.includes("--check")) {
  if (await readFile(destination, "utf8") !== output) throw new Error("Template form contract is out of date; run pnpm run generate:forms");
} else await writeFile(destination, output);
