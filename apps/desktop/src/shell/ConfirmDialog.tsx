import { useEffect, useId, useRef, type ReactNode } from "react";
import { ja } from "../messages";

/** Native modal with safe initial focus, Escape, focus trapping and focus restoration. */
export function ConfirmDialog({ title, children, onClose, onConfirm, confirmLabel = ja.confirm }: {
  title: string; children: ReactNode; onClose: () => void;
  onConfirm?: () => void; confirmLabel?: string;
}) {
  const ref = useRef<HTMLDialogElement>(null);
  const titleId = useId();
  const bodyId = useId();
  useEffect(() => {
    const dialog = ref.current;
    const opener = document.activeElement;
    dialog?.showModal();
    dialog?.querySelector<HTMLButtonElement>("button")?.focus();
    return () => {
      dialog?.close();
      if (opener instanceof HTMLElement && opener.isConnected) opener.focus();
    };
  }, []);
  return <dialog ref={ref} className="dialog" aria-labelledby={titleId} aria-describedby={bodyId}
    onCancel={(event) => { event.preventDefault(); onClose(); }}>
    <h2 id={titleId}>{title}</h2><div id={bodyId}>{children}</div>
    <div className="actions">
      <button className="btn" onClick={onClose}>{onConfirm ? ja.cancel : ja.close}</button>
      {onConfirm && <button className="btn primary" onClick={() => { onClose(); onConfirm(); }}>{confirmLabel}</button>}
    </div>
  </dialog>;
}
