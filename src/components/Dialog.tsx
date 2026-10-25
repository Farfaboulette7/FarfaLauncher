import { ReactNode, useEffect } from "react";
import "./Dialog.css";

interface Props {
  title: string;
  children: ReactNode;
  actions: ReactNode;
  onClose?: () => void;
}

/** Fenêtre modale au style YoRHa (Échap ferme si `onClose` est fourni). */
export function Dialog({ title, children, actions, onClose }: Props) {
  useEffect(() => {
    if (!onClose) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
        e.stopPropagation();
        onClose();
      }
    };
    window.addEventListener("keydown", onKey, true);
    return () => window.removeEventListener("keydown", onKey, true);
  }, [onClose]);

  return (
    <div
      className="dialog-backdrop"
      role="dialog"
      aria-modal="true"
      aria-label={title}
      // Clic à côté de la fenêtre : comme « Annuler ».
      onClick={(e) => e.target === e.currentTarget && onClose?.()}
    >
      <div className="dialog n-panel n-appear">
        <div className="n-panel-header">{title}</div>
        <div className="n-panel-body dialog-body">{children}</div>
        <div className="dialog-actions">{actions}</div>
      </div>
    </div>
  );
}
