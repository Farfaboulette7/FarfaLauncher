import { useSyncExternalStore } from "react";
import { musicState, nextTrack, subscribeMusic } from "../music";
import "./NowPlaying.css";

/** Morceau en cours et bouton pour en changer, en haut à droite de chaque écran. */
export function NowPlaying() {
  const { title, enabled, switching } = useSyncExternalStore(subscribeMusic, musicState);
  if (!enabled || !title) return null;
  return (
    <div className="now-playing">
      <span className="now-playing-title" title={title}>
        ♪ {title}
      </span>
      <button className="n-btn now-playing-next" onClick={() => void nextTrack()} disabled={switching}>
        {switching ? "…" : "Changer de musique"}
      </button>
    </div>
  );
}
