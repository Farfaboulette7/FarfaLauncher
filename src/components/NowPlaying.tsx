import { useSyncExternalStore } from "react";
import { musicState, nextTrack, setMusicPaused, subscribeMusic } from "../music";
import { Marquee } from "./Marquee";
import "./NowPlaying.css";

/** Morceau en cours, bouton pour en changer et pause, en haut à droite de chaque écran. */
export function NowPlaying() {
  const { title, enabled, switching, paused } = useSyncExternalStore(subscribeMusic, musicState);
  if (!enabled || !title) return null;
  return (
    <div className={`now-playing n-marquee-host${paused ? " now-playing--paused" : ""}`}>
      <Marquee className="now-playing-title" text={`♪ ${title}`} onHover />
      <button className="n-btn now-playing-next" onClick={() => void nextTrack()} disabled={switching}>
        {switching ? "…" : "Changer de musique"}
      </button>
      <button
        className="now-playing-pause"
        onClick={() => setMusicPaused(!paused)}
        title={paused ? "Reprendre la musique" : "Mettre la musique en pause"}
        aria-label={paused ? "Reprendre la musique" : "Mettre la musique en pause"}
        aria-pressed={paused}
      >
        <svg viewBox="0 0 12 12" aria-hidden="true">
          {paused ? <path d="M3 1.5v9l7.5-4.5z" /> : <path d="M2.5 1.5h2.5v9H2.5zM7 1.5h2.5v9H7z" />}
        </svg>
      </button>
    </div>
  );
}
