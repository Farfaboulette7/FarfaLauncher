import { useEffect } from "react";
import { Emblem } from "./Emblem";
import "./Intro.css";

/** Durée totale de l'animation d'ouverture (doit suivre Intro.css). */
const INTRO_MS = 3000;

/**
 * Ouverture de l'application (reprise de NieRDeck) : l'emblème YoRHa apparaît
 * en fondu sur le papier, puis tout s'efface pendant que l'écran se défloute.
 */
export function Intro({ title = "FaRFa", onDone }: { title?: string; onDone: () => void }) {
  useEffect(() => {
    const timer = setTimeout(onDone, INTRO_MS);
    return () => clearTimeout(timer);
  }, [onDone]);
  return (
    <div className="intro" aria-hidden="true">
      <div className="intro-mark">
        <Emblem className="intro-emblem" />
        <div className="intro-title">{title}</div>
        <div className="intro-rule" />
      </div>
    </div>
  );
}
