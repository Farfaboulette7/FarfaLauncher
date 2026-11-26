import { CSSProperties, useEffect, useRef, useState } from "react";
import { Emblem } from "./Emblem";
import { capturePad } from "../gamepad";
import "./Intro.css";

/** Filet : l'ouverture se termine au plus tard après ce délai (l'animation dure 3 s, Intro.css). */
const INTRO_MAX_MS = 6000;
/** Écart d'espacement entre deux lettres du titre au départ (1,1 em → 0,5 em, Intro.css). */
const TITLE_SPREAD_EM = 0.6;

/**
 * Ouverture de l'application (reprise de NieRDeck) : l'emblème YoRHa apparaît
 * en fondu sur le papier, puis tout s'efface pendant que l'écran se défloute.
 *
 * La même animation partout (Windows, Linux, Mac, Deck) : elle ne bouge que la position,
 * la taille et l'opacité de calques déjà dessinés, ce que le moteur web anime sans
 * redessiner la page, même quand le démarrage occupe le navigateur. Le flou est une copie
 * floutée une fois pour toutes, en fondu enchaîné ; l'espacement du titre, des lettres qui
 * glissent. Sous Linux (WebKitGTK), un flou ou un espacement animés étaient redessinés à
 * chaque image par le processeur, et restaient figés.
 *
 * L'animation ne part qu'une fois l'écran prêt à être dessiné (deux images), et se termine à
 * sa vraie fin (`animationend`), pas après un délai fixe.
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
  const letters = Array.from(title);
  const middle = (letters.length - 1) / 2;
  return (
    <div
      className={`intro${running ? " is-running" : ""}`}
      aria-hidden="true"
      onAnimationEnd={(e) => {
        // Le papier est le dernier à finir (3 s) : c'est la fin de l'ouverture.
        if (e.animationName === "intro-paper") done.current();
      }}
    >
      <div className="intro-blur" />
      <div className="intro-paper" />
      <div className="intro-mark">
        <div className="intro-emblem-box">
          <Emblem className="intro-emblem intro-emblem--soft" />
          <Emblem className="intro-emblem intro-emblem--sharp" />
        </div>
        <div className="intro-title">
          {letters.map((c, i) => (
            <span key={i} style={{ "--d": `${(i - middle) * TITLE_SPREAD_EM}em` } as CSSProperties}>
              {c}
            </span>
          ))}
        </div>
        <div className="intro-rule" />
      </div>
    </div>
  );
}
