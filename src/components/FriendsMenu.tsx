// Panneau « Amis » : un bouton-icône dans l'en-tête, qui ouvre la liste des joueurs du serveur
// (en jeu, en ligne, hors ligne) et l'option « Apparaître hors ligne ». La présence vient de
// FarfaRomManager ; un gestionnaire trop ancien ne la connaît pas : le bouton n'apparaît pas.
import { useEffect, useRef, useState } from "react";
import { convertFileSrc } from "@tauri-apps/api/core";
import { backend, type Friend, type Friends } from "../api/backend";
import "./FriendsMenu.css";

/** Liste relue toutes les minutes (le launcher signale sa présence au même rythme). */
const REFRESH_MS = 60_000;
/** Panneau ouvert : relue plus souvent. */
const REFRESH_OPEN_MS = 20_000;

const avatars = new Map<number, Promise<string | null>>();
function avatarOf(id: number): Promise<string | null> {
  let p = avatars.get(id);
  if (!p) {
    p = backend
      .friendAvatar(id)
      .then((path) => convertFileSrc(path))
      .catch(() => null);
    avatars.set(id, p);
  }
  return p;
}

/** Avatar RomM d'un joueur (servi par le gestionnaire), ou l'initiale de son pseudo. */
export function PlayerAvatar({ player: friend }: { player: { id: number; username: string; avatar: boolean } }) {
  const [src, setSrc] = useState<string | null>(null);
  useEffect(() => {
    if (!friend.avatar) return;
    let alive = true;
    void avatarOf(friend.id).then((s) => alive && setSrc(s));
    return () => {
      alive = false;
    };
  }, [friend.id, friend.avatar]);
  return src ? (
    <img className="friend-avatar" src={src} alt="" draggable={false} />
  ) : (
    <span className="friend-avatar is-letter" aria-hidden="true">
      {friend.username.slice(0, 1).toUpperCase()}
    </span>
  );
}

/** « 12 min », « 2 h », « 3 j ». */
function duration(fromUnix: number): string {
  const s = Math.max(0, Math.floor(Date.now() / 1000 - fromUnix));
  if (s < 60) return "moins d'une minute";
  if (s < 3600) return `${Math.floor(s / 60)} min`;
  if (s < 86_400) return `${Math.floor(s / 3600)} h`;
  return `${Math.floor(s / 86_400)} j`;
}

function statusLine(f: Friend): string {
  if (f.status === "en jeu") return f.since ? `En jeu depuis ${duration(f.since)}` : "En jeu";
  if (f.status === "en ligne") return "En ligne";
  return f.lastSeen ? `Vu il y a ${duration(f.lastSeen)}` : "Hors ligne";
}

export function FriendsMenu({
  offline,
  onOpenGame,
  onOpenStats,
}: {
  offline: boolean;
  /** Ouvre les statistiques d'un joueur (`null` : les siennes). */
  onOpenStats: (id: number | null) => void;
  /** Ouvre la fiche d'un jeu de la bibliothèque ; `false` si le jeu n'y est pas. */
  onOpenGame: (romId: number, from: HTMLElement) => boolean;
}) {
  const [open, setOpen] = useState(false);
  const [data, setData] = useState<Friends | null>(null);
  /** Le gestionnaire connaît la présence (sinon, rien n'est montré). */
  const [supported, setSupported] = useState(true);
  const [saving, setSaving] = useState(false);
  const [notice, setNotice] = useState<string | null>(null);
  const root = useRef<HTMLDivElement>(null);

  useEffect(() => {
    if (offline) return;
    let alive = true;
    const load = () =>
      backend
        .friends()
        .then((f) => {
          if (!alive) return;
          setSupported(f !== null);
          setData(f);
        })
        .catch(() => undefined);
    void load();
    const timer = window.setInterval(load, open ? REFRESH_OPEN_MS : REFRESH_MS);
    return () => {
      alive = false;
      window.clearInterval(timer);
    };
  }, [offline, open]);

  // Clic ailleurs ou Échap (B à la manette) : le panneau se ferme.
  useEffect(() => {
    if (!open) return;
    const away = (e: MouseEvent) => {
      if (!root.current?.contains(e.target as Node)) setOpen(false);
    };
    window.addEventListener("mousedown", away);
    root.current?.querySelector<HTMLElement>(".friends-panel button, .friends-panel input")?.focus({ preventScroll: true });
    return () => window.removeEventListener("mousedown", away);
  }, [open]);

  if (offline || !supported) return null;

  const amis = data?.amis ?? [];
  const present = amis.filter((f) => f.status !== "hors ligne");
  const playing = amis.filter((f) => f.status === "en jeu").length;
  const hidden = data?.hidden ?? false;

  async function toggleHidden(on: boolean) {
    setSaving(true);
    setNotice(null);
    try {
      await backend.setPresenceHidden(on);
      setData((d) => (d ? { ...d, hidden: on } : d));
      setNotice(on ? "Vous apparaissez hors ligne ✓" : "Vos amis vous voient de nouveau ✓");
    } catch {
      setNotice("Réglage non enregistré, réessayez.");
    } finally {
      setSaving(false);
    }
  }

  return (
    <div
      className="friends-menu"
      ref={root}
      onKeyDown={(e) => {
        if (e.key === "Escape" && open) {
          e.stopPropagation();
          setOpen(false);
          root.current?.querySelector<HTMLButtonElement>(".friends-menu-btn")?.focus();
        }
      }}
    >
      <button
        className="n-btn n-icon-btn friends-menu-btn"
        title={present.length > 0 ? `Amis : ${present.length} en ligne${playing ? `, dont ${playing} en jeu` : ""}` : "Amis"}
        aria-label="Amis"
        aria-expanded={open}
        onClick={() => setOpen((o) => !o)}
      >
        <svg viewBox="0 0 24 24" width="18" height="18" aria-hidden="true">
          <circle cx="9" cy="8" r="3.2" fill="none" stroke="currentColor" strokeWidth="1.8" />
          <path d="M3 19c0-3.3 2.7-5.5 6-5.5s6 2.2 6 5.5" fill="none" stroke="currentColor" strokeWidth="1.8" />
          <circle cx="17" cy="9" r="2.4" fill="none" stroke="currentColor" strokeWidth="1.6" />
          <path d="M16.5 13.6c2.6.1 4.5 1.9 4.5 4.6" fill="none" stroke="currentColor" strokeWidth="1.6" />
        </svg>
        {present.length > 0 && <span className="n-new friends-count">{present.length}</span>}
      </button>
      {open && (
        <div className="friends-panel" role="dialog" aria-label="Amis">
          <div className="friends-head">
            <span className="friends-title">Amis</span>
            <span className="n-hint">{present.length > 0 ? `${present.length} en ligne` : "Personne en ligne"}</span>
          </div>
          {data === null ? (
            <p className="n-hint">Chargement…</p>
          ) : amis.length === 0 ? (
            <p className="n-hint">Aucun autre joueur sur ce serveur.</p>
          ) : (
            <ul className="friends-list">
              {amis.map((f) => (
                <li key={f.id} className={`friend is-${f.status === "en jeu" ? "playing" : f.status === "en ligne" ? "online" : "offline"}`}>
                  <PlayerAvatar player={f} />
                  <span className="friend-text">
                    <button
                      className="friend-name"
                      title="Voir ses statistiques"
                      onClick={() => {
                        setOpen(false);
                        onOpenStats(f.id);
                      }}
                    >
                      <span className="friend-dot" aria-hidden="true" />
                      {f.username}
                    </button>
                    {f.game && (
                      <button
                        className="n-link friend-game"
                        title="Voir le jeu"
                        onClick={(e) => {
                          if (onOpenGame(f.game!.romId, e.currentTarget)) setOpen(false);
                        }}
                      >
                        {f.game.name || `Jeu ${f.game.romId}`}
                      </button>
                    )}
                    <span className="friend-status">{statusLine(f)}</span>
                  </span>
                </li>
              ))}
            </ul>
          )}
          <button
            className="n-link friends-stats"
            onClick={() => {
              setOpen(false);
              onOpenStats(null);
            }}
          >
            Mes statistiques et classement →
          </button>
          <label className="friends-check">
            <input type="checkbox" checked={hidden} disabled={saving || data === null} onChange={(e) => void toggleHidden(e.target.checked)} />
            Apparaître hors ligne
          </label>
          <p className="n-hint friends-hint">
            {notice ?? (hidden ? "Vos amis ne voient ni votre présence ni vos parties." : "Vos amis voient quand vous êtes là et à quoi vous jouez.")}
          </p>
        </div>
      )}
    </div>
  );
}
