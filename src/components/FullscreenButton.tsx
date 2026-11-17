import { useEffect, useState } from "react";
import { backend } from "../api/backend";

/** Bascule plein écran / fenêtre (aussi F11 et View à la manette) : bouton-icône. */
export function FullscreenButton() {
  const [on, setOn] = useState<boolean | null>(null);
  useEffect(() => {
    const read = () => backend.isFullscreen().then(setOn).catch(() => undefined);
    read();
    window.addEventListener("resize", read);
    return () => window.removeEventListener("resize", read);
  }, []);
  if (on === null) return null;
  const label = on ? "Quitter le plein écran (F11)" : "Plein écran (F11)";
  return (
    <button className="n-btn n-icon-btn" title={label} aria-label={label} onClick={() => backend.setFullscreen(!on).then(() => setOn(!on))}>
      <svg viewBox="0 0 24 24" width="18" height="18" aria-hidden="true">
        <path
          d={on ? "M9 4v5H4M15 4v5h5M9 20v-5H4M15 20v-5h5" : "M4 9V4h5M20 9V4h-5M4 15v5h5M20 15v5h-5"}
          fill="none"
          stroke="currentColor"
          strokeWidth="1.8"
          strokeLinejoin="round"
        />
      </svg>
    </button>
  );
}

/** F11 : bascule plein écran / fenêtre. À appeler au démarrage. */
export function installFullscreenKey() {
  window.addEventListener("keydown", (e) => {
    if (e.key !== "F11") return;
    e.preventDefault();
    backend
      .isFullscreen()
      .then((on) => backend.setFullscreen(!on))
      .catch(() => undefined);
  });
}
