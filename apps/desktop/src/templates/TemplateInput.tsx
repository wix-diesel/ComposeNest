import { useState } from "react";
import type { FormInput, JsonValue } from "../generated/template-form";
import { integerCandidate } from "./formModel";

/** Renders one of the five Schema 1 input types without inventing an initial value. */
export function TemplateInput({ input, value, hasSecret, needsAnswer, error, onChange, onGenerate }: {
  input: FormInput; value: JsonValue; hasSecret: boolean; needsAnswer: boolean; error?: string;
  onChange: (value: JsonValue) => void; onGenerate: () => void;
}) {
  const [revealed, setRevealed] = useState(false);
  const id = `input-${input.key}`;
  const props = { id, "aria-required": input.required, "aria-invalid": !!error,
    "aria-describedby": `${id}-help${error ? ` ${id}-error` : ""}` };
  const secret = input.inputType === "secret";
  const validation = input.validation !== null && typeof input.validation === "object" && !Array.isArray(input.validation) ? input.validation : {};
  return <div className="field" data-field-path={`inputs.${input.key}`}>
    <label htmlFor={id}>{input.label}{input.required && <span className="required">必須</span>}</label>
    <div className="input-action">
      {input.inputType === "boolean" ? <select {...props} value={value === null ? "" : String(value)}
        onChange={(event) => onChange(event.target.value === "" ? null : event.target.value === "true")}>
        <option value="">未設定</option><option value="true">はい（true）</option><option value="false">いいえ（false）</option>
      </select> : input.inputType === "select" ? <select {...props} value={typeof value === "string" ? value : ""}
        onChange={(event) => onChange(event.target.value === "" ? null : event.target.value)}>
        <option value="">未設定</option>{input.options.map((option) => <option key={option.value} value={option.value}>{option.label}</option>)}
        {typeof value === "string" && value !== "" && !input.options.some((option) => option.value === value) && <option value={value}>現在値: {value}（選択肢外）</option>}
      </select> : <input {...props} type={secret && !revealed ? "password" : "text"}
        inputMode={input.inputType === "integer" ? "numeric" : undefined} autoComplete="off"
        value={value === null ? "" : String(value)} placeholder={secret && hasSecret ? "設定済み（値は非表示）" : "未設定"}
        onChange={(event) => onChange(input.inputType === "integer" ? integerCandidate(event.target.value) : event.target.value)} />}
      {secret && <button type="button" className="btn small" disabled={typeof value !== "string"}
        aria-label={`${input.label}を${revealed ? "隠す" : "表示"}`} aria-pressed={revealed} onClick={() => setRevealed(!revealed)}>{revealed ? "隠す" : "表示"}</button>}
      {secret && input.canGenerate && <button type="button" className="btn small" aria-label={`${input.label}を再生成`} onClick={() => { setRevealed(false); onGenerate(); }}>再生成</button>}
      {!input.required && <button type="button" className="btn small" aria-label={`${input.label}を未設定にする`} onClick={() => { setRevealed(false); onChange(null); }}>未設定にする</button>}
    </div>
    <small id={`${id}-help`}>{input.description}{needsAnswer && " 入力待ち。"}
      {value === null ? secret && hasSecret ? " 設定済み（値は非表示）。" : " 未設定。" : value === "" ? " 空文字を設定。" : ""}
      {Object.entries(validation).map(([key, item]) => <span key={key} className="constraint">{key}: {String(item)}</span>)}
    </small>
    {error && <p id={`${id}-error`} className="error-text">{error}</p>}
  </div>;
}
