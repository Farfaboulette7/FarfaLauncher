import { useEffect, useRef, useState } from "react";
import { createPortal } from "react-dom";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { GAME_STOPPED_EVENT, GameStopped } from "../api/backend";
import { Emblem } from "./Emblem";
import "./LaunchOverlay.css";

/** Le jeu tarde : on propose de masquer l'écran. */
const SLOW_MS = 10_000;
/** Le jeu ne montre jamais de fenêtre : on rend la main. */
const GIVE_UP_MS = 90_000;
const FADE_MS = 400;

interface Props {
  romId: number;
  name: string;
  /** Étape en cours (« Préparation du jeu… ») ; `null` : le jeu démarre. */
  step: string | null;
  /** Le jeu est lancé : sa fenêtre peut maintenant prendre la main. */
  started: boolean;
  onDone: () => void;
}

/**
 * Écran « Lancement en cours », comme celui de NieRDeck : de l'appui sur
 * Jouer jusqu'à l'apparition de la fenêtre du jeu (relevée par le Rust sous
 * Windows), ou jusqu'à ce que le jeu prenne le focus ailleurs.
 */
export function LaunchOverlay({ romId, name, step, started, onDone }: Props) {
  const [closing, setClosing] = useState(false);
  const [slow, setSlow] = useState(false);
  const done = useRef(onDone);
  done.current = onDone;
  const startedRef = useRef(started);
  startedRef.current = started;

  const close = useRef(() => {});
  close.current = () => {
    setClosing(true);
    window.setTimeout(() => done.current(), FADE_MS);
  };

  useEffect(() => {
    const slowTimer = window.setTimeout(() => setSlow(true), SLOW_MS);
    const giveUp = window.setTimeout(() => close.current(), GIVE_UP_MS);
    const subs = [
      listen<number>("game-window", ({ payload }) => payload === romId && close.current()),
      listen<GameStopped>(GAME_STOPPED_EVENT, ({ payload }) => payload.romId === romId && close.current()),
      // Sans fenêtre relevée (Linux) : le jeu qui prend le focus suffit.
      getCurrentWindow().onFocusChanged(({ payload: focused }) => !focused && startedRef.current && close.current()),
    ];
    return () => {
      window.clearTimeout(slowTimer);
      window.clearTimeout(giveUp);
      subs.forEach((s) => s.then((f) => f()));
    };
  }, [romId]);

  return createPortal(
    <div className="launch-overlay" data-closing={closing} onClick={() => slow && close.current()}>
      <div className="launch-panel">
        <Emblem className="launch-emblem" />
        <div className="launch-text">
          <div className="launch-heading">{step ?? "Lancement en cours"}</div>
          <div className="launch-title">{name}</div>
          <div className="launch-bar">
            <span />
          </div>
          {slow && <div className="launch-hint">Le jeu met du temps à s'afficher… Cliquez pour masquer ce message.</div>}
        </div>
      </div>
    </div>,
    document.body,
  );
}
