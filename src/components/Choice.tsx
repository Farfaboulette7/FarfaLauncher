// Liste déroulante au style YoRHa : la liste native de Windows (menu noir) ne suit pas le thème.
// Pilotable à la souris, au clavier et à la manette (chaque option est un bouton).
import { useEffect, useRef, useState } from "react";

export interface ChoiceOption {
  value: string;
  label: string;
}

export function Choice({
  value,
  options,
  placeholder,
  onChange,
}: {
  value: string;
  options: ChoiceOption[];
  placeholder: string;
  onChange: (value: string) => void;
}) {
  const [open, setOpen] = useState(false);
  const root = useRef<HTMLDivElement>(null);
  const list = useRef<HTMLUListElement>(null);
  const current = options.find((o) => o.value === value);

  // Clic ailleurs : la liste se ferme.
  useEffect(() => {
    if (!open) return;
    const away = (e: MouseEvent) => {
      if (!root.current?.contains(e.target as Node)) setOpen(false);
    };
    window.addEventListener("mousedown", away);
    // Focus sur l'option choisie (ou la première) : la manette et le clavier partent de là.
    const selected = list.current?.querySelector<HTMLButtonElement>("[aria-selected='true']") ?? list.current?.querySelector("button");
    selected?.focus({ preventScroll: true });
    selected?.scrollIntoView({ block: "nearest" });
    return () => window.removeEventListener("mousedown", away);
  }, [open]);

  function pick(v: string) {
    onChange(v);
    setOpen(false);
    root.current?.querySelector<HTMLButtonElement>(".choice-btn")?.focus();
  }

  return (
    <div
      className="choice"
      ref={root}
      onKeyDown={(e) => {
        // Échap (ou B à la manette) ferme la liste, pas la fenêtre autour.
        if (e.key === "Escape" && open) {
          e.stopPropagation();
          setOpen(false);
          root.current?.querySelector<HTMLButtonElement>(".choice-btn")?.focus();
        }
      }}
    >
      <button type="button" className="n-input choice-btn" aria-haspopup="listbox" aria-expanded={open} onClick={() => setOpen((o) => !o)}>
        <span className={current ? undefined : "choice-placeholder"}>{current?.label ?? placeholder}</span>
        <span className="choice-arrow" aria-hidden="true">
          {open ? "▴" : "▾"}
        </span>
      </button>
      {open && (
        <ul className="choice-list" role="listbox" ref={list}>
          {options.map((o) => (
            <li key={o.value}>
              <button type="button" className="choice-opt" role="option" aria-selected={o.value === value} onClick={() => pick(o.value)}>
                {o.label}
              </button>
            </li>
          ))}
        </ul>
      )}
    </div>
  );
}
