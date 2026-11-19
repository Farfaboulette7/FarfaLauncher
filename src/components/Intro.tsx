import { useEffect, useRef, useState } from "react";
import { Emblem } from "./Emblem";
import { capturePad } from "../gamepad";
import "./Intro.css";

/** Filet : l'ouverture se termine au plus tard après ce délai (l'animation dure 3 s, Intro.css). */
const INTRO_MAX_MS = 6000;

/**
 * Ouverture de l'application (reprise de NieRDeck) : l'emblème YoRHa apparaît
 * en fondu sur le papier, puis tout s'efface pendant que l'écran se défloute.
 *
 * L'animation ne part qu'une fois l'écran prêt à être dessiné (deux images), et se termine à
 * sa vraie fin (`animationend`), pas après un délai fixe : sous Linux (WebKitGTK), le démarrage
 * occupe le navigateur, et un délai fixe la coupait avant la fin (logo figé, puis final).
 */
export function Intro({ title = "FaRFa", onDone }: { title?: string; onDone: () => void }) {
  const [running, setRunning] = useState(false);
  const done = useRef(onDone);
  done.current = onDone;
  useEffect(() => {
    let second = 0;
    const first = requestAnimationFrame(() => {
      second = requestAnimationFrame(() => setRunning(true));
    });
    const timer = setTimeout(() => done.current(), INTRO_MAX_MS);
    // Rien ne répond pendant l'intro : ni la manette, ni le clavier, ni la souris (l'écran
    // en dessous est déjà là, on pouvait ouvrir un jeu sans le voir).
    const releasePad = capturePad();
    const block = (e: Event) => {
      e.preventDefault();
      e.stopImmediatePropagation();
    };
    const kinds = ["keydown", "keyup", "keypress", "mousedown", "mouseup", "click", "dblclick", "contextmenu", "wheel", "pointerdown", "pointerup", "touchstart"] as const;
    kinds.forEach((k) => window.addEventListener(k, block, { capture: true, passive: false }));
    return () => {
      cancelAnimationFrame(first);
      cancelAnimationFrame(second);
      clearTimeout(timer);
      releasePad();
      kinds.forEach((k) => window.removeEventListener(k, block, { capture: true }));
    };
  }, []);
  return (
    <div
      className={`intro${running ? " is-running" : ""}`}
      aria-hidden="true"
      onAnimationEnd={(e) => {
        if (e.target === e.currentTarget) done.current();
      }}
    >
      <div className="intro-mark">
        <Emblem className="intro-emblem" />
        <div className="intro-title">{title}</div>
        <div className="intro-rule" />
      </div>
    </div>
  );
}
