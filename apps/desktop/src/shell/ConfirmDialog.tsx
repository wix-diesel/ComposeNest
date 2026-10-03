import { useEffect, useId, useRef, type ReactNode } from "react";
import { ja } from "../messages";

/** Native modal with safe initial focus, Escape, focus trapping and focus restoration. */
export function ConfirmDialog({ title, children, onClose, onConfirm, confirmLabel = ja.confirm, confirmDisabled = false }: {
  title: string; children: ReactNode; onClose: () => void;
  onConfirm?: () => void; confirmLabel?: string; confirmDisabled?: boolean;
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
    onKeyDown={(event) => {
      if (event.key !== "Tab") return;
      const targets = Array.from(event.currentTarget.querySelectorAll<HTMLElement>(
        'button:not(:disabled), a[href], input:not(:disabled), select:not(:disabled), textarea:not(:disabled), [tabindex]',
      )).filter((element) => element.tabIndex >= 0 && element.getClientRects().length > 0);
      const first = targets[0];
      const last = targets.at(-1);
      if ((event.shiftKey && document.activeElement === first)
        || (!event.shiftKey && document.activeElement === last)) {
        event.preventDefault();
        (event.shiftKey ? last : first)?.focus();
      }
    }}
    onCancel={(event) => { event.preventDefault(); onClose(); }}>
    <h2 id={titleId}>{title}</h2><div id={bodyId}>{children}</div>
    <div className="actions">
      <button className="btn" onClick={onClose}>{onConfirm ? ja.cancel : ja.close}</button>
      {onConfirm && <button className="btn primary" disabled={confirmDisabled} onClick={() => { onClose(); onConfirm(); }}>{confirmLabel}</button>}
    </div>
  </dialog>;
}
