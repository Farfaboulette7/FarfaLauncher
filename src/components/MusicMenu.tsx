// Menu de la musique : un bouton-icône dans l'en-tête, qui ouvre un petit panneau avec le titre
// en cours, « changer de musique », les volumes de la musique et des bruitages, et de quoi couper l'un ou l'autre.
import { useEffect, useRef, useState, useSyncExternalStore } from "react";
import { musicSettings, musicState, nextTrack, setMusicSettings, subscribeMusic } from "../music";
import { play as playSound, setSoundSettings, soundSettings } from "../sounds";
import { clearAudioNotice } from "../audioGuard";
import { Marquee } from "./Marquee";
import "./MusicMenu.css";

export function MusicMenu() {
  const { title, enabled, switching } = useSyncExternalStore(subscribeMusic, musicState);
  const [open, setOpen] = useState(false);
  const [music, setMusic] = useState(musicSettings());
  const [sound, setSound] = useState(soundSettings());
  const root = useRef<HTMLDivElement>(null);

  // Clic ailleurs ou Échap (B à la manette) : le panneau se ferme.
  useEffect(() => {
    if (!open) return;
    setMusic(musicSettings());
    setSound(soundSettings());
    const away = (e: MouseEvent) => {
      if (!root.current?.contains(e.target as Node)) setOpen(false);
    };
    window.addEventListener("mousedown", away);
    root.current?.querySelector<HTMLElement>(".music-panel button, .music-panel input")?.focus({ preventScroll: true });
    return () => window.removeEventListener("mousedown", away);
  }, [open]);

  function updateMusic(next: Partial<ReturnType<typeof musicSettings>>) {
    if (next.enabled) clearAudioNotice();
    setMusicSettings(next);
    setMusic(musicSettings());
  }

  function updateSound(next: Partial<ReturnType<typeof soundSettings>>) {
    if (next.enabled) clearAudioNotice();
    setSoundSettings(next);
    setSound(soundSettings());
  }

  return (
    <div
      className="music-menu"
      ref={root}
      onKeyDown={(e) => {
        if (e.key === "Escape" && open) {
          e.stopPropagation();
          setOpen(false);
          root.current?.querySelector<HTMLButtonElement>(".music-menu-btn")?.focus();
        }
      }}
    >
      <button
        className="n-btn n-icon-btn music-menu-btn"
        title={enabled && title ? `Musique : ${title}` : "Musique et bruitages"}
        aria-label="Musique et bruitages"
        aria-expanded={open}
        onClick={() => setOpen((o) => !o)}
      >
        <svg viewBox="0 0 24 24" width="18" height="18" aria-hidden="true">
          <path d="M9 18V5l11-2v13" fill="none" stroke="currentColor" strokeWidth="1.8" strokeLinejoin="round" />
          <circle cx="6" cy="18" r="3" fill="none" stroke="currentColor" strokeWidth="1.8" />
          <circle cx="17" cy="16" r="3" fill="none" stroke="currentColor" strokeWidth="1.8" />
        </svg>
      </button>
      {open && (
        <div className="music-panel n-marquee-host" role="dialog" aria-label="Musique et bruitages">
          <div className="music-title">
            {music.enabled ? <Marquee text={title ? `♪ ${title}` : "♪ …"} /> : <span className="n-hint">Musique coupée</span>}
          </div>
          {music.enabled ? (
            <button className="n-btn n-btn--sm music-next" onClick={() => void nextTrack()} disabled={switching}>
              {switching ? "Changement…" : "Changer de musique"}
            </button>
          ) : (
            <button className="n-btn n-btn--sm music-next" onClick={() => updateMusic({ enabled: true })}>
              Remettre la musique
            </button>
          )}
          <label className="music-range">
            Musique
            <input
              type="range"
              min={0}
              max={1}
              step={0.05}
              value={music.volume}
              disabled={!music.enabled}
              onChange={(e) => updateMusic({ volume: Number(e.target.value) })}
            />
          </label>
          <label className="music-range">
            Bruitages
            <input
              type="range"
              min={0}
              max={1}
              step={0.05}
              value={sound.enabled ? sound.volume : 0}
              onChange={(e) => {
                const volume = Number(e.target.value);
                updateSound(volume > 0 ? { enabled: true, volume } : { volume: 0 });
              }}
              onPointerUp={() => playSound("deplacementOption")}
            />
          </label>
          <div className="music-mutes">
            <label className="music-check">
              <input type="checkbox" checked={!music.enabled} onChange={(e) => updateMusic({ enabled: !e.target.checked })} />
              Couper la musique
            </label>
            <label className="music-check">
              <input
                type="checkbox"
                checked={!sound.enabled || sound.volume === 0}
                onChange={(e) => {
                  if (e.target.checked) updateSound({ enabled: false });
                  else {
                    updateSound({ enabled: true, volume: sound.volume > 0 ? sound.volume : 0.5 });
                    playSound("deplacementOption");
                  }
                }}
              />
              Couper les bruitages
            </label>
          </div>
        </div>
      )}
    </div>
  );
}
