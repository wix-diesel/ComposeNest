import { ja } from "../messages";

/** Persistent notification; errors are announced and dismissal is always explicit. */
export function Toast({ message, kind, onDismiss }: {
  message: string; kind: "info" | "error"; onDismiss: () => void;
}) {
  return <div className={`toast ${kind}`} role={kind === "error" ? "alert" : "status"}>
    <span>{message}</span><button className="btn" onClick={onDismiss} aria-label={ja.dismiss}>×</button>
  </div>;
}
