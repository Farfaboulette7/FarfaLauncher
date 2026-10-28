import { useEffect, useState } from "react";
import { backend } from "../api/backend";

/** Bascule plein écran / fenêtre (aussi F11). */
export function FullscreenButton() {
  const [on, setOn] = useState<boolean | null>(null);
  useEffect(() => {
    const read = () => backend.isFullscreen().then(setOn).catch(() => undefined);
    read();
    window.addEventListener("resize", read);
    return () => window.removeEventListener("resize", read);
  }, []);
  if (on === null) return null;
  return (
    <button className="n-btn" title="F11" onClick={() => backend.setFullscreen(!on).then(() => setOn(!on))}>
      {on ? "Quitter le plein écran" : "Plein écran"}
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
