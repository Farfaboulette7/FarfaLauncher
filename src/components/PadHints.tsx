// Indications des boutons, en bas à droite, seulement pendant la navigation à la
// manette (classe `pad-nav` sur <html>). Noms des boutons de la dernière manette utilisée.
import { useEffect, useState } from "react";
import { PadLayout } from "../api/backend";
import { onPad, padLayoutOf } from "../gamepad";
import { confirmButtons, faceColor, padLabel } from "../padLayouts";

type Hint = { buttons: string[]; label: string };

/** Une fenêtre (dialogue) est ouverte : B la ferme, les onglets ne servent pas. */
function modalOpen(): boolean {
  return !!document.querySelector("[aria-modal='true'], .dialog-backdrop");
}

export function hintsFor(layout: PadLayout, modal: boolean): Hint[] {
  const { confirm, back } = confirmButtons(layout);
  if (modal) {
    return [
      { buttons: [confirm], label: "Valider" },
      { buttons: [back], label: "Fermer" },
    ];
  }
  return [
    { buttons: [confirm], label: "Valider" },
    { buttons: [back], label: "Retour" },
    { buttons: ["lb", "rb"], label: "Onglets" },
    { buttons: ["lt", "rt"], label: "Page" },
    { buttons: ["select"], label: "Plein écran" },
  ];
}

export function PadHints() {
  const [active, setActive] = useState(() => document.documentElement.classList.contains("pad-nav"));
  const [layout, setLayout] = useState<PadLayout>("generic");
  const [modal, setModal] = useState(modalOpen);
  useEffect(() => {
    const root = document.documentElement;
    const watch = new MutationObserver(() => setActive(root.classList.contains("pad-nav")));
    watch.observe(root, { attributes: true, attributeFilter: ["class"] });
    // Une fenêtre qui s'ouvre ou se ferme déplace le focus.
    const refresh = () => setModal(modalOpen());
    document.addEventListener("focusin", refresh);
    document.addEventListener("focusout", refresh);
    const stop = onPad((e) => {
      if (e.kind === "button" || e.kind === "axis") setLayout(padLayoutOf(e.pad));
      window.setTimeout(refresh, 60);
    });
    return () => {
      watch.disconnect();
      document.removeEventListener("focusin", refresh);
      document.removeEventListener("focusout", refresh);
      stop();
    };
  }, []);
  if (!active) return null;
  return (
    <div className="pad-hints" aria-hidden="true">
      {hintsFor(layout, modal).map((h) => (
        <span key={h.label} className="pad-hint">
          {h.buttons.map((b) => {
            const color = faceColor(layout, b);
            return (
              <span key={b} className="pad-glyph" style={color ? { borderColor: color, color } : undefined}>
                {padLabel(layout, b)}
              </span>
            );
          })}
          {h.label}
        </span>
      ))}
    </div>
  );
}
