import { useEffect, useRef, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { backend, formatPlaytime, Game } from "../api/backend";
import { BoxArt } from "./BoxArt";
import { Dialog } from "./Dialog";
import "./InGameMenu.css";

/** Ouverture du menu (Select + Start maintenus pendant la partie, voir ingame.rs). */
interface Opened {
  romId: number;
  /** Durée de la partie au moment de l'ouverture. */
  sessionMs: number;
}

interface Props {
  /** Fiche d'un jeu de la bibliothèque. */
  known: (id: number) => Game | undefined;
  platformName: (game: Game) => string | null;
}

/** Durée de la partie en cours : « 12:04 » ou « 1:02:09 ». */
function clock(ms: number): string {
  const s = Math.floor(ms / 1000);
  const two = (n: number) => String(n).padStart(2, "0");
  const h = Math.floor(s / 3600);
  return h > 0 ? `${h}:${two(Math.floor(s / 60) % 60)}:${two(s % 60)}` : `${Math.floor(s / 60)}:${two(s % 60)}`;
}

/** Menu en jeu : infos de la partie, reprendre ou quitter le jeu. */
export function InGameMenu({ known, platformName }: Props) {
  const [menu, setMenu] = useState<(Opened & { at: number }) | null>(null);
  const [total, setTotal] = useState<number | null>(null);
  const [quitting, setQuitting] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [now, setNow] = useState(Date.now());
  const resumeButton = useRef<HTMLButtonElement>(null);

  useEffect(() => {
    const subs = [
      listen<Opened>("ingame-menu", ({ payload }) => {
        setMenu({ ...payload, at: Date.now() });
        setQuitting(false);
        setError(null);
      }),
      listen<{ romId: number }>("game-stopped", ({ payload }) => setMenu((m) => (m && m.romId === payload.romId ? null : m))),
    ];
    return () => subs.forEach((s) => s.then((f) => f()));
  }, []);

  const romId = menu?.romId;
  const openedAt = menu?.at;
  useEffect(() => {
    if (romId === undefined) return;
    let live = true;
    setTotal(null);
    backend
      .gamePlaytime(romId)
      .then((ms) => live && setTotal(ms))
      .catch(() => undefined);
    setNow(Date.now());
    const tick = window.setInterval(() => setNow(Date.now()), 1000);
    resumeButton.current?.focus();
    document.documentElement.classList.add("pad-nav");
    return () => {
      live = false;
      window.clearInterval(tick);
    };
  }, [romId, openedAt]);

  if (!menu) return null;
  const game = known(menu.romId);
  const session = menu.sessionMs + (now - menu.at);

  const resume = () => {
    setMenu(null);
    backend.ingameResume().catch(() => undefined);
  };
  const quit = () => {
    setQuitting(true);
    backend.quitGame(menu.romId).catch(() => {
      setQuitting(false);
      setError("Le jeu n'a pas pu être fermé.");
    });
  };

  return (
    <Dialog
      title="Menu en jeu"
      onClose={quitting ? undefined : resume}
      actions={
        quitting ? (
          <p className="igm-quitting">
            <span className="n-spinner" /> Fermeture du jeu…
          </p>
        ) : (
          <>
            <button className="n-btn" ref={resumeButton} onClick={resume}>
              Reprendre
            </button>
            <button className="n-btn" onClick={quit}>
              Quitter le jeu
            </button>
          </>
        )
      }
    >
      <div className="igm">
        {game && (
          <div className="igm-cover">
            <BoxArt game={game} />
          </div>
        )}
        <div className="igm-info">
          <h2 className="igm-title">{game?.name ?? "Jeu en cours"}</h2>
          {game && platformName(game) && <p className="n-hint">{platformName(game)}</p>}
          <dl className="gd-facts">
            <dt>Partie en cours</dt>
            <dd>{clock(session)}</dd>
            <dt>Temps de jeu total</dt>
            <dd>{total === null ? "…" : formatPlaytime(total + session)}</dd>
            {game?.releaseYear && (
              <>
                <dt>Sortie</dt>
                <dd>{game.releaseYear}</dd>
              </>
            )}
          </dl>
          {error && <p className="n-alert">{error}</p>}
        </div>
      </div>
    </Dialog>
  );
}
