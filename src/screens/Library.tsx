import { Emblem } from "../components/Emblem";
import { CSSProperties, ReactNode, useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import {
  AUTH_ERROR_CODES,
  backend,
  errorMessage,
  Game,
  hasUpdate,
  isAppError,
  LIBRARY_DONE_EVENT,
  LIBRARY_GAME_EVENT,
  LibraryDone,
  LibrarySnapshot,
  Platform,
  SessionInfo,
  Target,
} from "../api/backend";
import { useFirstImage } from "../hooks/useCachedImage";
import { BoxArt } from "../components/BoxArt";
import { loadSounds, play as playSound } from "../sounds";
import { loadMusic, setMusicMuted, setMusicVersion, stopMusic } from "../music";
import { useFavorites } from "../hooks/useFavorites";
import { InstallsState, useInstalls } from "../hooks/useInstalls";
import { GameDetail } from "./GameDetail";
import { Settings } from "./Settings";
import "./Library.css";
import "./Hack.css";

type Tab = "recent" | "library" | "favorites";

/** Durées du passage vers la fiche d'un jeu (doivent suivre Hack.css). */
const HACK_OUT_MS = 380;
const HACK_IN_MS = 420;

interface Hack {
  /** Centre de la jaquette cliquée. */
  x: number;
  y: number;
  /** Aller : out puis in ; retour : back-out puis back-in. */
  phase: "out" | "in" | "back-out" | "back-in";
}

const TABS: { id: Tab; label: string }[] = [
  { id: "recent", label: "Récent" },
  { id: "library", label: "Bibliothèque" },
  { id: "favorites", label: "Favoris" },
];

/** Nombre de jeux affichés dans « Récent ». */
const RECENT_COUNT = 24;

interface Props {
  session: SessionInfo;
  /** Déconnexion volontaire (notice = null) ou forcée (token révoqué…). */
  onLogout: (notice: string | null) => void;
  onCheckUpdates: () => Promise<string>;
}

export function Library({ session, onLogout, onCheckUpdates }: Props) {
  const [snapshot, setSnapshot] = useState<LibrarySnapshot | null>(null);
  const [target, setTarget] = useState<Target | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [warning, setWarning] = useState<string | null>(null);
  const [query, setQuery] = useState("");
  const [tab, setTab] = useState<Tab>("recent");
  const [platformId, setPlatformId] = useState<number | null>(null);
  const [selectedId, setSelectedId] = useState<number | null>(null);
  const [showSettings, setShowSettings] = useState(false);
  const [hack, setHack] = useState<Hack | null>(null);
  /** Centre de la jaquette d'où l'on est parti (point de retour du zoom). */
  const openedFrom = useRef({ x: window.innerWidth / 2, y: window.innerHeight / 2 });
  /** Dernier jeu ouvert : sa jaquette sert de cible au zoom arrière. */
  const lastSelected = useRef<number | null>(null);

  /** Erreur d'appel : déconnexion propre si l'accès a été retiré. */
  const handleError = useCallback(
    async (e: unknown): Promise<string> => {
      if (isAppError(e) && AUTH_ERROR_CODES.has(e.code)) {
        await backend.logout().catch(() => undefined);
        onLogout(e.message);
      }
      return errorMessage(e);
    },
    [onLogout],
  );
  const installs = useInstalls(handleError);
  const favorites = useFavorites(handleError, !!session.connected);
  const reloadInstalls = installs.reload;
  const reloadFavorites = favorites.reload;

  const refresh = useCallback(async () => {
    setLoading(true);
    setError(null);
    setWarning(null);
    try {
      const r = await backend.libraryRefresh();
      // Rafraîchissement déjà en cours sans liste encore reçue : l'autre appel s'en charge.
      if (r.snapshot) setSnapshot(r.snapshot);
      // Les jeux déplacés sur le serveur ont pu être rattachés à leur installation.
      reloadInstalls().catch(() => undefined);
      // Les détails arrivent ensuite par événements ; `loading` retombe sur library-done.
    } catch (e) {
      setError(await handleError(e));
      setLoading(false);
    }
  }, [handleError, reloadInstalls]);

  useEffect(() => {
    const unlisten = [
      listen<Game>(LIBRARY_GAME_EVENT, ({ payload }) =>
        setSnapshot((s) => s && { ...s, games: s.games.map((g) => (g.id === payload.id ? payload : g)) }),
      ),
      listen<LibraryDone>(LIBRARY_DONE_EVENT, ({ payload }) => {
        setLoading(false);
        if (payload.failed.length > 0) {
          setWarning(`Le serveur n'a pas répondu à temps pour : ${payload.failed.join(", ")}. Réessayez plus tard.`);
        }
      }),
    ];
    return () => unlisten.forEach((p) => p.then((f) => f()));
  }, []);

  // Musique : réservée aux utilisateurs connectés, arrêtée à la déconnexion.
  useEffect(() => {
    loadMusic();
    return stopMusic;
  }, []);
  // Version 8-bit sur la fiche d'un jeu, normale partout ailleurs.
  useEffect(() => setMusicVersion(selectedId !== null ? "eightBit" : "normal"), [selectedId]);
  // Silence pendant qu'un jeu tourne.
  const gameRunning = installs.running.size > 0;
  useEffect(() => setMusicMuted(gameRunning), [gameRunning]);

  useEffect(() => {
    backend.systemTarget().then(setTarget);
    loadSounds();
    // Temps de jeu resté en attente (serveur injoignable lors d'une session).
    if (!session.offline) {
      backend.flushPlaySessions().catch((e) => console.warn("sessions en attente non envoyées", e));
    }
    // Cache d'abord ; le serveur n'est interrogé que si le cache est vide
    // (sinon : bouton « Rafraîchir »).
    backend
      .libraryCached()
      .then((snap) => {
        if (snap) {
          setSnapshot(snap);
          setLoading(false);
        } else if (session.offline) {
          setLoading(false);
        } else {
          refresh();
        }
      })
      .catch(async (e) => {
        setError(await handleError(e));
        setLoading(false);
      });
  }, [refresh, handleError, session.offline]);

  const allGames = useMemo(() => snapshot?.games ?? [], [snapshot]);
  // Plateformes sans jeu (dossier encore vide sur le serveur) : masquées.
  const platforms = useMemo(
    () =>
      (snapshot?.platforms ?? [])
        .map((p) => ({ platform: p, count: allGames.filter((g) => g.platformId === p.id).length }))
        .filter((p) => p.count > 0),
    [snapshot, allGames],
  );
  const platform = snapshot?.platforms.find((p) => p.id === platformId) ?? null;

  /** Jeux de la vue courante, avant recherche. */
  const viewGames = useMemo(() => {
    if (tab === "recent") {
      return [...allGames].sort((a, b) => b.createdAt.localeCompare(a.createdAt) || b.id - a.id).slice(0, RECENT_COUNT);
    }
    if (tab === "favorites") return allGames.filter((g) => favorites.ids.has(g.id));
    return platformId === null ? allGames : allGames.filter((g) => g.platformId === platformId);
  }, [tab, allGames, favorites.ids, platformId]);

  const q = query.trim().toLocaleLowerCase("fr");
  // Recherche : dans la vue courante (toute la bibliothèque depuis la liste des plateformes).
  const games = q ? viewGames.filter((g) => g.name.toLocaleLowerCase("fr").includes(q)) : viewGames;
  const showPlatforms = tab === "library" && platformId === null && !q;

  function openTab(t: Tab) {
    setTab(t);
    setPlatformId(null);
    setQuery("");
  }

  /** Joue une transition en deux temps ; `swap` change d'écran entre les deux. */
  function transition(at: { x: number; y: number }, out: Hack["phase"], into: Hack["phase"], swap: () => void) {
    setHack({ ...at, phase: out });
    setTimeout(() => {
      swap();
      setHack({ ...at, phase: into });
    }, HACK_OUT_MS);
    setTimeout(() => setHack(null), HACK_OUT_MS + HACK_IN_MS);
  }

  /** Ouvre la fiche d'un jeu en zoomant vers sa jaquette. */
  function openGame(id: number, from: HTMLElement) {
    if (hack) return;
    const r = from.getBoundingClientRect();
    openedFrom.current = { x: r.left + r.width / 2, y: r.top + r.height / 2 };
    lastSelected.current = id;
    setMusicVersion("eightBit"); // le fondu musical part avec le zoom
    transition(openedFrom.current, "out", "in", () => setSelectedId(id));
  }

  /** Retour à la bibliothèque : le même zoom, à l'envers. */
  function closeGame() {
    if (hack) return;
    playSound("retourSelection");
    setMusicVersion("normal");
    transition(openedFrom.current, "back-out", "back-in", () => setSelectedId(null));
  }

  // La bibliothèque vient d'être recréée : on ramène la jaquette du jeu quitté à
  // l'écran et on recentre le zoom arrière sur elle, avant le premier affichage.
  const returningTo = hack?.phase === "back-in" ? lastSelected.current : null;
  useLayoutEffect(() => {
    if (returningTo === null) return;
    const card = document.querySelector<HTMLElement>(`[data-game-id="${returningTo}"]`);
    if (!card) return;
    card.scrollIntoView({ block: "nearest" });
    const r = card.getBoundingClientRect();
    setHack((h) => h && { ...h, x: r.left + r.width / 2, y: r.top + r.height / 2 });
  }, [returningTo]);

  /**
   * Écran courant, animé pendant le passage vers une fiche. L'enveloppe est
   * toujours là : l'ajouter en cours de route remonterait l'écran (défilement perdu).
   */
  function stage(node: ReactNode) {
    const vars = hack ? ({ "--hx": `${hack.x}px`, "--hy": `${hack.y}px` } as CSSProperties) : undefined;
    return (
      <div className={hack ? `hack-stage hack-stage--${hack.phase}` : "hack-stage"} style={vars}>
        {node}
      </div>
    );
  }

  if (showSettings) {
    return (
      <Settings
        session={session}
        installs={installs}
        onBack={() => setShowSettings(false)}
        onLogout={onLogout}
        onCheckUpdates={onCheckUpdates}
        onCacheCleared={() => {
          setSnapshot(null);
          if (!session.offline) refresh();
        }}
      />
    );
  }

  const selected = allGames.find((g) => g.id === selectedId) ?? null;
  if (selected) {
    return stage(
      <GameDetail
        game={selected}
        platform={snapshot?.platforms.find((p) => p.id === selected.platformId) ?? null}
        target={target}
        installs={installs}
        favorite={favorites.ids.has(selected.id)}
        onToggleFavorite={favorites.offline || favorites.needsRelogin ? null : () => favorites.toggle(selected.id)}
        favoriteError={favorites.error}
        onBack={closeGame}
        onError={handleError}
      />,
    );
  }

  const fetched = snapshot && new Date(snapshot.fetchedAt * 1000).toLocaleString("fr-FR", { dateStyle: "short", timeStyle: "short" });

  return stage(
    <div className="lib">
      <header className="lib-header">
        <div className="lib-brand">
          <p className="n-subtitle lib-brand-name">
            <Emblem className="brand-logo" />
            FaRFa project
          </p>
          <nav className="lib-tabs" role="tablist">
            {TABS.map((t) => (
              <button
                key={t.id}
                role="tab"
                className="lib-tab"
                aria-selected={tab === t.id}
                onClick={() => openTab(t.id)}
              >
                {t.label}
              </button>
            ))}
          </nav>
        </div>
        <div className="lib-status">
          <span className="lib-user">{session.offline ? "Hors ligne" : session.username}</span>
          <button
            className="n-btn"
            onClick={() => {
              refresh();
              reloadFavorites();
            }}
            disabled={loading || session.offline}
          >
            {loading ? "Chargement…" : "Rafraîchir"}
          </button>
          <button className="n-btn" onClick={() => setShowSettings(true)}>
            Paramètres
          </button>
        </div>
      </header>
      <div className="n-rule lib-rule" />

      <div className="lib-toolbar">
        {tab === "library" && platform && (
          <button className="n-link" onClick={() => setPlatformId(null)}>
            ← Plateformes
          </button>
        )}
        <h2 className="n-section-title lib-view-title">{viewTitle(tab, platform)}</h2>
        <input
          className="n-input lib-search"
          type="search"
          placeholder="Rechercher un jeu…"
          value={query}
          onChange={(e) => setQuery(e.target.value)}
        />
        <span className="n-hint">
          {snapshot && !showPlatforms && `${games.length} jeu${games.length > 1 ? "x" : ""}`}
          {snapshot && showPlatforms && `${platforms.length} plateforme${platforms.length > 1 ? "s" : ""}`}
          {fetched && ` · mis à jour ${fetched}`}
        </span>
      </div>

      {session.offline && <p className="n-notice lib-msg">Serveur injoignable : affichage de la dernière liste connue.</p>}
      {error && <p className="n-alert lib-msg">{error}</p>}
      {warning && <p className="n-notice lib-msg">{warning}</p>}
      {tab === "favorites" && favorites.error && (
        <div className="n-notice lib-msg lib-msg--row">
          <span>{favorites.error}</span>
          {favorites.needsRelogin && (
            <button
              className="n-btn"
              onClick={async () => {
                await backend.logout().catch(() => undefined);
                onLogout("Reconnectez-vous pour activer les favoris (une seule fois).");
              }}
            >
              Se reconnecter
            </button>
          )}
        </div>
      )}

      <main className="lib-grid-wrap n-appear" key={`${tab}:${platformId}`}>
        {!snapshot && loading && (
          <p className="lib-empty">
            <span className="n-spinner" /> Chargement de la bibliothèque…
          </p>
        )}
        {showPlatforms && snapshot && (
          <ul className="lib-platforms">
            {platforms.map(({ platform: p, count }) => (
              <li key={p.id}>
                <PlatformCard platform={p} count={count} onOpen={() => setPlatformId(p.id)} />
              </li>
            ))}
          </ul>
        )}
        {!showPlatforms && snapshot && games.length === 0 && <p className="lib-empty">{emptyMessage(tab, q, favorites.loading)}</p>}
        {!showPlatforms && (
          <ul className="lib-grid">
            {games.map((g) => (
              <li key={g.id}>
                <GameCard
                  game={g}
                  installs={installs}
                  favorite={favorites.ids.has(g.id)}
                  caption={tab === "recent" ? addedOn(g) : undefined}
                  onOpen={(el) => openGame(g.id, el)}
                />
              </li>
            ))}
          </ul>
        )}
      </main>
    </div>,
  );
}

function viewTitle(tab: Tab, platform: Platform | null): string {
  if (tab === "recent") return "Derniers ajouts";
  if (tab === "favorites") return "Mes favoris";
  return platform ? platform.name : "Plateformes";
}

function emptyMessage(tab: Tab, q: string, favoritesLoading: boolean): string {
  if (q) return "Aucun jeu ne correspond.";
  if (tab === "favorites") return favoritesLoading ? "Chargement des favoris…" : "Aucun favori pour l'instant : ouvrez un jeu et cliquez « ☆ Favori ».";
  return "Aucun jeu sur le serveur.";
}

function addedOn(game: Game): string | undefined {
  const d = new Date(game.createdAt);
  return isNaN(d.getTime()) ? undefined : `Ajouté le ${d.toLocaleDateString("fr-FR")}`;
}

function PlatformCard({ platform, count, onOpen }: { platform: Platform; count: number; onOpen: () => void }) {
  const { src } = useFirstImage(platform.iconUrls);
  return (
    <button className="pcard" onClick={onOpen} title={platform.name}>
      <span className="pcard-icon">{src ? <img src={src} alt="" draggable={false} /> : <span className="pcard-glyph">◆</span>}</span>
      <span className="pcard-name">{platform.name}</span>
      <span className="pcard-count">
        {count} jeu{count > 1 ? "x" : ""}
      </span>
    </button>
  );
}

function GameCard({
  game,
  installs,
  favorite,
  caption,
  onOpen,
}: {
  game: Game;
  installs: InstallsState;
  favorite: boolean;
  caption?: string;
  onOpen: (el: HTMLElement) => void;
}) {
  const inst = installs.installed.get(game.id);
  const busy = installs.progress.has(game.id);
  const badge = busy
    ? { label: "Installation…", cls: "" }
    : hasUpdate(game, inst)
      ? { label: "Mise à jour disponible", cls: "" }
      : inst?.executable
        ? { label: "Installé", cls: "card-badge--ok" }
        : !game.myArchive && game.detailsLoaded
          ? { label: "Non disponible", cls: "card-badge--muted" }
          : null;
  const [kind, setKind] = useState<"box" | "flat" | null>(null);
  return (
    <button className={`card${kind === "box" ? " card--box3d" : ""}`} data-sound-hover="game" data-game-id={game.id} onClick={(e) => onOpen(e.currentTarget)} title={game.name}>
      <div className="card-cover">
        <BoxArt game={game} onKind={setKind} />
        {kind === null && !game.boxFaces && <span className="card-placeholder">{game.name}</span>}
        {badge && <span className={`card-badge ${badge.cls}`}>{badge.label}</span>}
        {favorite && (
          <span className="card-fav" title="Favori">
            ★
          </span>
        )}
      </div>
      <span className="card-name">{game.name}</span>
      {caption && <span className="card-caption">{caption}</span>}
    </button>
  );
}
