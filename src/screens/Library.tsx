import { useCallback, useEffect, useMemo, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import {
  AUTH_ERROR_CODES,
  backend,
  errorMessage,
  Game,
  isAppError,
  LIBRARY_DONE_EVENT,
  LIBRARY_GAME_EVENT,
  LibraryDone,
  LibrarySnapshot,
  SessionInfo,
  Target,
} from "../api/backend";
import { useCachedImage } from "../hooks/useCachedImage";
import { InstallsState, useInstalls } from "../hooks/useInstalls";
import { hasUpdate } from "../api/backend";
import { GameDetail } from "./GameDetail";
import { Settings } from "./Settings";
import "./Library.css";

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
  const [selectedId, setSelectedId] = useState<number | null>(null);
  const [showSettings, setShowSettings] = useState(false);

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

  const refresh = useCallback(async () => {
    setLoading(true);
    setError(null);
    setWarning(null);
    try {
      const r = await backend.libraryRefresh();
      setSnapshot(r.snapshot);
      // Les détails arrivent ensuite par événements ; `loading` retombe sur library-done.
    } catch (e) {
      setError(await handleError(e));
      setLoading(false);
    }
  }, [handleError]);

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

  useEffect(() => {
    backend.systemTarget().then(setTarget);
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

  const games = useMemo(() => {
    const all = snapshot?.games ?? [];
    const q = query.trim().toLocaleLowerCase("fr");
    return q ? all.filter((g) => g.name.toLocaleLowerCase("fr").includes(q)) : all;
  }, [snapshot, query]);

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

  const selected = snapshot?.games.find((g) => g.id === selectedId) ?? null;
  if (selected) {
    return (
      <GameDetail game={selected} target={target} installs={installs} onBack={() => setSelectedId(null)} onError={handleError} />
    );
  }

  return (
    <div className="lib">
      <header className="lib-header">
        <div>
          <p className="n-subtitle">FarfaLauncher</p>
          <h1 className="n-title">Bibliothèque</h1>
        </div>
        <div className="lib-status">
          <span className="lib-user">
            {session.offline ? "Hors ligne" : session.username}
          </span>
          <button className="n-btn" onClick={refresh} disabled={loading || session.offline}>
            {loading ? "Chargement…" : "Rafraîchir"}
          </button>
          <button className="n-btn" onClick={() => setShowSettings(true)}>
            Paramètres
          </button>
        </div>
      </header>
      <div className="n-rule lib-rule" />

      <div className="lib-toolbar">
        <input
          className="n-input lib-search"
          type="search"
          placeholder="Rechercher un jeu…"
          value={query}
          onChange={(e) => setQuery(e.target.value)}
        />
        <span className="n-hint">
          {snapshot ? `${games.length} jeu${games.length > 1 ? "x" : ""}` : ""}
          {snapshot && ` · mis à jour ${new Date(snapshot.fetchedAt * 1000).toLocaleString("fr-FR", { dateStyle: "short", timeStyle: "short" })}`}
        </span>
      </div>

      {session.offline && <p className="n-notice lib-msg">Serveur injoignable : affichage de la dernière liste connue.</p>}
      {error && <p className="n-alert lib-msg">{error}</p>}
      {warning && <p className="n-notice lib-msg">{warning}</p>}

      <main className="lib-grid-wrap">
        {!snapshot && loading && (
          <p className="lib-empty">
            <span className="n-spinner" /> Chargement de la bibliothèque…
          </p>
        )}
        {snapshot && games.length === 0 && <p className="lib-empty">{query ? "Aucun jeu ne correspond." : "Aucun jeu sur le serveur."}</p>}
        <ul className="lib-grid">
          {games.map((g) => (
            <li key={g.id}>
              <GameCard game={g} installs={installs} onOpen={() => setSelectedId(g.id)} />
            </li>
          ))}
        </ul>
      </main>
    </div>
  );
}

function GameCard({ game, installs, onOpen }: { game: Game; installs: InstallsState; onOpen: () => void }) {
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
  const { src } = useCachedImage(game.coverUrl);
  return (
    <button className="card" onClick={onOpen} title={game.name}>
      <div className="card-cover">
        {src ? <img src={src} alt="" draggable={false} /> : <span className="card-placeholder">{game.name}</span>}
        {badge && <span className={`card-badge ${badge.cls}`}>{badge.label}</span>}
      </div>
      <span className="card-name">{game.name}</span>
    </button>
  );
}
