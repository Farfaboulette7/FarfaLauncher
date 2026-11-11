import { getVersion } from "@tauri-apps/api/app";
import type { Update } from "@tauri-apps/plugin-updater";
import { Emblem } from "../components/Emblem";
import { CSSProperties, memo, ReactNode, useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import {
  AUTH_ERROR_CODES,
  backend,
  errorMessage,
  emulatorReady,
  Game,
  hasUpdate,
  InstalledGame,
  isAvailable,
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
import { Marquee } from "../components/Marquee";
import { loadSounds, play as playSound } from "../sounds";
import { loadMusic, setMusicMuted, setMusicVersion, stopMusic } from "../music";
import { useFavorites } from "../hooks/useFavorites";
import { useInstalls } from "../hooks/useInstalls";
import { GameDetail } from "./GameDetail";
import { Settings } from "./Settings";
import { useReveal } from "../reveal";
import { beginAudio } from "../audioGuard";
import "./Library.css";
import "./Hack.css";
import { FullscreenButton } from "../components/FullscreenButton";

type Tab = "recent" | "library" | "favorites";
/** Filtre de la bibliothèque, des favoris et de la recherche. */
type Kind = "all" | "installed" | "recomp" | "emulation";

const KINDS: { id: Kind; label: string }[] = [
  { id: "all", label: "Tous" },
  { id: "installed", label: "Installés" },
  { id: "recomp", label: "Recomp" },
  { id: "emulation", label: "Émulation" },
];

/** Une console, toutes plateformes RomM confondues (`gba` et `portage-gba`). */
interface ConsoleGroup {
  key: string;
  name: string;
  iconUrls: string[];
  /** Plateformes chargées à la demande : leurs jeux ne sont pas dans la liste de départ. */
  lazyIds: number[];
  lazyCount: number;
  hasEmulator: boolean;
}

/** Jeux chargés d'une console. */
interface ConsolePage {
  ids: number[];
  hasMore: boolean;
  loading: boolean;
}

const collator = new Intl.Collator("fr");

/** Comment ce jeu se joue sur ce système : recomp, émulateur, ou les deux. */
function playModes(game: Game): { recomp: boolean; emulation: boolean } {
  if (game.pc) return { recomp: false, emulation: false };
  return { recomp: game.recomp && (isAvailable(game) || !game.detailsLoaded), emulation: emulatorReady(game) };
}

function matchesKind(game: Game, kind: Kind, installed: Map<number, unknown>): boolean {
  if (kind === "installed") return installed.has(game.id);
  if (kind === "all") return true;
  return playModes(game)[kind];
}

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
/** Nom donné par le launcher à la plateforme des jeux PC. */
const PC_PLATFORM = "Ordinateur";
/** Distance au bas de la liste à laquelle la page suivante est demandée. */
const LOAD_MORE_MARGIN_PX = 600;
/** Rangées dessinées en plus au-dessus et en dessous de l'écran. */
const OVERSCAN_ROWS = 2;
/** Attente après la dernière frappe avant d'interroger le serveur. */
const SEARCH_DELAY_MS = 300;
/** Un jeu reste « nouveau » (badge NEW, compteur des plateformes) un jour,
 *  ou jusqu'à ce que sa fiche soit ouverte. */
const NEW_FOR_MS = 24 * 3600 * 1000;
/** Jeux nouveaux déjà ouverts : id → date d'ouverture (ms). */
const SEEN_KEY = "farfa.seenGames";

function loadSeen(): Map<number, number> {
  try {
    const raw = JSON.parse(localStorage.getItem(SEEN_KEY) ?? "{}") as Record<string, number>;
    return new Map(Object.entries(raw).map(([id, at]) => [Number(id), at]));
  } catch {
    return new Map();
  }
}

function saveSeen(seen: Map<number, number>) {
  // Au-delà de la durée « nouveau », inutile de s'en souvenir.
  const keep = [...seen].filter(([, at]) => Date.now() - at < NEW_FOR_MS * 2);
  try {
    localStorage.setItem(SEEN_KEY, JSON.stringify(Object.fromEntries(keep)));
  } catch {
    // tant pis : le badge reviendra jusqu'à la fin de la journée
  }
}

function isNew(game: Game, seen: Map<number, number>): boolean {
  if (seen.has(game.id)) return false;
  const t = Date.parse(game.createdAt);
  return !isNaN(t) && Date.now() - t < NEW_FOR_MS;
}

/** Jour d'ajout (« 24.10.2026 »), pour regrouper l'onglet Récent. */
function dayOf(game: Game): string {
  const d = new Date(game.createdAt);
  if (isNaN(d.getTime())) return "Date inconnue";
  return `${String(d.getDate()).padStart(2, "0")}.${String(d.getMonth() + 1).padStart(2, "0")}.${d.getFullYear()}`;
}

interface Props {
  session: SessionInfo;
  /** Déconnexion volontaire (notice = null) ou forcée (token révoqué…). */
  onLogout: (notice: string | null) => void;
  onCheckUpdates: () => Promise<string>;
  /** Nouvelle version sortie pendant que le launcher tourne. */
  update: Update | null;
  onUpdate: () => void;
}

export function Library({ session, onLogout, onCheckUpdates, update, onUpdate }: Props) {
  const [appVersion, setAppVersion] = useState<string | null>(null);
  useEffect(() => {
    getVersion().then(setAppVersion).catch(() => undefined);
  }, []);
  const [snapshot, setSnapshot] = useState<LibrarySnapshot | null>(null);
  const [target, setTarget] = useState<Target | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [warning, setWarning] = useState<string | null>(null);
  const [query, setQuery] = useState("");
  const [tab, setTab] = useState<Tab>("recent");
  const [consoleKey, setConsoleKey] = useState<string | null>(null);
  const [kind, setKind] = useState<Kind>("all");
  /** Jeux chargés à la demande, hors de la liste de départ. */
  const [extra, setExtra] = useState(new Map<number, Game>());
  const [consolePages, setConsolePages] = useState(new Map<string, ConsolePage>());
  const [searchIds, setSearchIds] = useState<number[]>([]);
  const [searching, setSearching] = useState(false);
  const [recentIds, setRecentIds] = useState<number[]>([]);
  const [selectedId, setSelectedId] = useState<number | null>(null);
  const [seen, setSeen] = useState(loadSeen);
  const [showSettings, setShowSettings] = useState(false);
  const [hack, setHack] = useState<Hack | null>(null);
  /** Centre de la jaquette d'où l'on est parti (point de retour du zoom). */
  const openedFrom = useRef({ x: window.innerWidth / 2, y: window.innerHeight / 2 });
  /** Dernier jeu ouvert : sa jaquette sert de cible au zoom arrière. */
  const lastSelected = useRef<number | null>(null);
  const root = useRef<HTMLDivElement>(null);
  const grid = useRef<HTMLElement>(null);

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
  const addExtra = useCallback((games: Game[]) => {
    setExtra((m) => {
      const next = new Map(m);
      for (const g of games) {
        // Ne pas remplacer une fiche complète par une fiche légère de la liste.
        if (g.detailsLoaded || !next.get(g.id)?.detailsLoaded) next.set(g.id, g);
      }
      return next;
    });
  }, []);
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
    // Filet de sécurité : son coupé si le lancement précédent s'est figé dessus.
    beginAudio();
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
    // Cache d'abord pour un affichage immédiat, puis rafraîchissement auprès
    // du serveur pour faire apparaître les nouveaux jeux.
    backend
      .libraryCached()
      .then((snap) => {
        if (snap) setSnapshot(snap);
        if (session.offline) {
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
  const known = useCallback((id: number) => allGames.find((g) => g.id === id) ?? extra.get(id), [allGames, extra]);
  /** Console d'une plateforme : son slug, ou « Ordinateur » pour les jeux PC. */
  const keyOf = useCallback((p: Platform) => (p.name === PC_PLATFORM ? PC_PLATFORM : p.slug), []);
  const pcPlatforms = useMemo(() => new Set((snapshot?.platforms ?? []).filter((p) => p.name === PC_PLATFORM).map((p) => p.id)), [snapshot]);
  const gameKey = useCallback((g: Game) => (g.pc || pcPlatforms.has(g.platformId) ? PC_PLATFORM : g.console), [pcPlatforms]);
  const groups = useMemo(() => {
    const byKey = new Map<string, ConsoleGroup>();
    for (const p of snapshot?.platforms ?? []) {
      const key = keyOf(p);
      const portage = p.fsSlug.toLowerCase().startsWith("portage");
      const group = byKey.get(key) ?? { key, name: shortPlatformName(p.name), iconUrls: p.iconUrls, lazyIds: [], lazyCount: 0, hasEmulator: false };
      if (!portage && key !== PC_PLATFORM) {
        // Le dossier principal de la console donne son nom (`nes`, pas sa variante `nes-hack`).
        if (group.lazyIds.length === 0 || p.fsSlug === p.slug) {
          group.name = p.name;
          group.iconUrls = p.iconUrls;
        }
        group.lazyIds.push(p.id);
        group.lazyCount += p.romCount;
      }
      group.hasEmulator = group.hasEmulator || p.hasEmulator;
      byKey.set(key, group);
    }
    return byKey;
  }, [snapshot, keyOf]);
  // Consoles sans jeu : masquées. Le filtre ne garde que celles qui ont de quoi jouer.
  const platforms = useMemo(
    () =>
      [...groups.values()]
        .map((group) => {
          // Le compteur suit le filtre. Toutes les recomps sont dans la liste de départ ; un
          // jeu chargé à la demande a l'émulateur de sa console, s'il y en a un.
          const own = allGames.filter((g) => gameKey(g) === group.key && matchesKind(g, kind, installs.installed));
          // Les jeux de la liste de départ rangés dans la console (recomps) sont aussi dans son total RomM.
          const counted = allGames.filter((g) => group.lazyIds.includes(g.platformId)).length;
          const rest = Math.max(0, group.lazyCount - counted);
          const lazy = kind === "recomp" || kind === "installed" || (kind === "emulation" && !group.hasEmulator) ? 0 : rest;
          return { group, count: own.length + lazy, fresh: own.filter((g) => isNew(g, seen)).length };
        })
        .filter((p) => p.count > 0)
        .sort((a, b) => collator.compare(a.group.name, b.group.name)),
    [groups, allGames, gameKey, seen, kind, installs.installed],
  );
  const group = consoleKey ? (groups.get(consoleKey) ?? null) : null;

  /** Charge la page suivante d'une console. */
  const loadConsole = useCallback(
    async (target: ConsoleGroup, offset: number) => {
      if (target.lazyIds.length === 0) return;
      setConsolePages((m) => new Map(m).set(target.key, { ids: m.get(target.key)?.ids ?? [], hasMore: m.get(target.key)?.hasMore ?? true, loading: true }));
      try {
        const page = await backend.consoleGames(target.lazyIds, offset);
        addExtra(page.games);
        setConsolePages((m) =>
          new Map(m).set(target.key, { ids: [...(m.get(target.key)?.ids ?? []), ...page.games.map((g) => g.id)], hasMore: page.hasMore, loading: false }),
        );
      } catch (e) {
        setWarning(await handleError(e));
        setConsolePages((m) => new Map(m).set(target.key, { ids: m.get(target.key)?.ids ?? [], hasMore: false, loading: false }));
      }
    },
    [addExtra, handleError],
  );
  /** Avec ce filtre, cette console a-t-elle des jeux à charger ? Sinon, rien n'est demandé. */
  const lazyUseful = !!group && kind !== "recomp" && kind !== "installed" && (kind !== "emulation" || group.hasEmulator);
  // À l'ouverture d'une console : sa première page, une seule fois.
  useEffect(() => {
    if (group && lazyUseful && !consolePages.has(group.key) && !session.offline) loadConsole(group, 0);
  }, [group, lazyUseful, consolePages, loadConsole, session.offline]);

  // « Récent » : les derniers ajouts de tout RomM, pas seulement les recomps.
  useEffect(() => {
    if (tab !== "recent" || session.offline) return;
    backend
      .recentGames()
      .then((list) => {
        addExtra(list);
        setRecentIds(list.map((g) => g.id));
      })
      .catch((e) => console.warn("derniers ajouts non chargés", e));
  }, [tab, session.offline, addExtra, snapshot?.fetchedAt]);

  // Favoris hors de la liste de départ : lus sur le serveur.
  const favoriteIds = favorites.ids;
  useEffect(() => {
    if (tab !== "favorites" || session.offline) return;
    const missing = [...favoriteIds].filter((id) => !allGames.some((g) => g.id === id) && !extra.has(id));
    if (missing.length > 0) backend.gamesByIds(missing).then(addExtra).catch((e) => console.warn("favoris non chargés", e));
  }, [tab, favoriteIds, allGames, extra, addExtra, session.offline]);

  // « Installés » : les jeux installés hors de la liste de départ (émulation…), lus sur le serveur.
  // Identifiants négatifs : programmes partagés (émulateurs), sans fiche RomM.
  const installedIds = useMemo(() => [...installs.installed.keys()].filter((id) => id > 0), [installs.installed]);
  useEffect(() => {
    if (tab !== "library" || kind !== "installed" || session.offline) return;
    const missing = installedIds.filter((id) => !allGames.some((g) => g.id === id) && !extra.has(id));
    if (missing.length > 0) backend.gamesByIds(missing).then(addExtra).catch((e) => console.warn("jeux installés non chargés", e));
  }, [tab, kind, installedIds, allGames, extra, addExtra, session.offline]);

  // Fiche d'un jeu chargé à la demande : ses fichiers, à l'ouverture de sa page.
  useEffect(() => {
    if (selectedId === null || session.offline) return;
    if (allGames.some((g) => g.id === selectedId) || extra.get(selectedId)?.detailsLoaded) return;
    backend
      .gameDetails(selectedId)
      .then((g) => addExtra([g]))
      .catch(async (e) => setWarning(await handleError(e)));
  }, [selectedId, allGames, extra, addExtra, handleError, session.offline]);

  // Recherche sur tout RomM, en plus de ce qui est déjà chargé.
  const term = query.trim();
  useEffect(() => {
    setSearchIds([]);
    if (term.length < 2 || session.offline || tab !== "library") {
      setSearching(false);
      return;
    }
    setSearching(true);
    let alive = true;
    const timer = window.setTimeout(() => {
      backend
        .searchGames(term)
        .then((list) => {
          if (!alive) return;
          addExtra(list);
          setSearchIds(list.map((g) => g.id));
        })
        .catch((e) => console.warn("recherche impossible", e))
        .finally(() => alive && setSearching(false));
    }, SEARCH_DELAY_MS);
    return () => {
      alive = false;
      window.clearTimeout(timer);
    };
  }, [term, tab, session.offline, addExtra]);
  // Écran révélé à chaque retour dessus, la grille à chaque changement de vue.
  useReveal(root, [showSettings, selectedId === null], { screen: true });
  useReveal(grid, [tab, consoleKey, !!snapshot], { skipFirst: true });

  /** Jeux de la vue courante, avant recherche. */
  const viewGames = useMemo(() => {
    if (tab === "recent") {
      const recents = recentIds.map((id) => extra.get(id)).filter((g): g is Game => !!g && !allGames.some((a) => a.id === g.id));
      return [...allGames, ...recents].sort((a, b) => b.createdAt.localeCompare(a.createdAt) || b.id - a.id).slice(0, RECENT_COUNT);
    }
    if (tab === "favorites") return [...favorites.ids].map((id) => known(id)).filter((g): g is Game => !!g);
    // « Installés » : tous les jeux installés, toutes consoles confondues.
    if (kind === "installed" && consoleKey === null)
      return installedIds.map((id) => known(id)).filter((g): g is Game => !!g).sort((a, b) => collator.compare(a.name, b.name));
    const own = consoleKey === null ? allGames : allGames.filter((g) => gameKey(g) === consoleKey);
    const ownIds = new Set(own.map((g) => g.id));
    const page = consoleKey ? (consolePages.get(consoleKey)?.ids ?? []) : [];
    const loaded = page.map((id) => extra.get(id)).filter((g): g is Game => !!g && !ownIds.has(g.id));
    return [...own, ...loaded].sort((a, b) => collator.compare(a.name, b.name));
  }, [tab, allGames, favorites.ids, consoleKey, consolePages, extra, recentIds, known, gameKey, kind, installedIds]);

  const q = term.toLocaleLowerCase("fr");
  // « Récent » montre tous les derniers ajouts, sans filtre.
  const shownKind: Kind = tab === "recent" ? "all" : kind;
  // Recherche : dans la vue courante, plus les résultats de RomM (toute la bibliothèque ou la console ouverte).
  const games = useMemo(() => {
    const local = q ? viewGames.filter((g) => g.name.toLocaleLowerCase("fr").includes(q)) : viewGames;
    const localIds = new Set(local.map((g) => g.id));
    const found = q && tab === "library"
      ? searchIds.map((id) => extra.get(id)).filter((g): g is Game => !!g && !localIds.has(g.id) && (consoleKey === null || gameKey(g) === consoleKey))
      : [];
    return [...local, ...found].filter((g) => matchesKind(g, shownKind, installs.installed));
  }, [q, viewGames, tab, searchIds, extra, consoleKey, gameKey, shownKind, installs.installed]);
  const showPlatforms = tab === "library" && consoleKey === null && !q && kind !== "installed";
  const page = consoleKey ? consolePages.get(consoleKey) : undefined;

  function openTab(t: Tab) {
    listScroll.current = 0;
    setTab(t);
    setConsoleKey(null);
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
  const listScroll = useRef(0);
  const openGameLatest = useRef<(id: number, from: HTMLElement) => void>(() => undefined);
  openGameLatest.current = openGame;
  // Identité stable : une carte ne se redessine pas quand une autre change.
  const onOpenCard = useCallback((id: number, el: HTMLElement) => openGameLatest.current(id, el), []);
  function openGame(id: number, from: HTMLElement) {
    if (hack) return;
    // La liste est recréée au retour : elle reprendra à cette position.
    listScroll.current = grid.current?.scrollTop ?? 0;
    const r = from.getBoundingClientRect();
    openedFrom.current = { x: r.left + r.width / 2, y: r.top + r.height / 2 };
    lastSelected.current = id;
    if (!seen.has(id)) {
      const next = new Map(seen).set(id, Date.now());
      setSeen(next);
      saveSeen(next);
    }
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

  const selected = selectedId === null ? null : (known(selectedId) ?? null);
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
    <div className="lib" ref={root}>
      <header className="lib-header">
        <div className="lib-brand">
          <p className="n-subtitle lib-brand-name">
            <Emblem className="brand-logo" />
            FaRFa project
            {appVersion && <span className="lib-version">v{appVersion}</span>}
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
          {update && (
            <button className="n-btn lib-update" onClick={onUpdate} title={`Installer la version ${update.version} (le launcher redémarre)`}>
              Mettre à jour ({update.version})
            </button>
          )}
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
          <FullscreenButton />
          <button className="n-btn" onClick={() => setShowSettings(true)}>
            Paramètres
          </button>
        </div>
      </header>
      <div className="n-rule lib-rule" />

      <div className="lib-toolbar">
        <div className="lib-toolbar-start">
          {tab === "library" && group && (
            <button
              className="n-link"
              onClick={() => {
                listScroll.current = 0;
                setConsoleKey(null);
              }}
            >
              ← Plateformes
            </button>
          )}
          <h2 className="n-section-title lib-view-title">{viewTitle(tab, group)}</h2>
        </div>
        <input
          className="n-input lib-search n-input--soft"
          type="search"
          placeholder="Rechercher un jeu…"
          value={query}
          onChange={(e) => setQuery(e.target.value)}
        />
        <span className="n-hint lib-count">
          {searching && <span className="n-spinner" />}
          {snapshot && !showPlatforms && `${games.length} jeu${games.length > 1 ? "x" : ""}${page?.hasMore ? "+" : ""}`}
          {snapshot && showPlatforms && `${platforms.length} plateforme${platforms.length > 1 ? "s" : ""}`}
          {fetched && ` · mis à jour ${fetched}`}
        </span>
      </div>

      {tab !== "recent" && (
        <div className="lib-kinds" role="group" aria-label="Type de jeu">
          {KINDS.map((k) => (
            <button key={k.id} className="n-btn lib-kind" aria-pressed={kind === k.id} onClick={() => setKind(k.id)}>
              {k.label}
            </button>
          ))}
        </div>
      )}

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

      <main className="lib-grid-wrap n-appear" key={`${tab}:${consoleKey}`} ref={grid}>
        {!snapshot && loading && (
          <p className="lib-empty">
            <span className="n-spinner" /> Chargement de la bibliothèque…
          </p>
        )}
        {showPlatforms && snapshot && (
          <ul className="lib-platforms">
            {platforms.map(({ group: g, count, fresh }) => (
              <li key={g.key}>
                <PlatformCard group={g} count={count} fresh={fresh} onOpen={() => {
                  listScroll.current = 0;
                  setConsoleKey(g.key);
                }} />
              </li>
            ))}
          </ul>
        )}
        {!showPlatforms && snapshot && games.length === 0 && !page?.loading && !searching && <p className="lib-empty">{emptyMessage(tab, q, favorites.loading, shownKind)}</p>}
        {!showPlatforms && tab !== "recent" && (
          <VirtualGrid
            games={games}
            initialScroll={listScroll}
            renderCard={(g) => (
              <GameCard game={g} fresh={isNew(g, seen)} inst={installs.installed.get(g.id)} busy={installs.progress.has(g.id)} favorite={favorites.ids.has(g.id)} onOpen={onOpenCard} />
            )}
          />
        )}
        {!showPlatforms &&
          tab === "recent" &&
          groupByDay(games).map(({ day, games: list }) => (
            <section key={day ?? "all"} className="lib-day">
              {day && (
                <h3 className="lib-day-title">
                  {day}
                  <span className="lib-day-count">
                    {list.length} jeu{list.length > 1 ? "x" : ""} ajouté{list.length > 1 ? "s" : ""}
                  </span>
                </h3>
              )}
              <ul className="lib-grid">
                {list.map((g) => (
                  <li key={g.id}>
                    <GameCard game={g} fresh={isNew(g, seen)} inst={installs.installed.get(g.id)} busy={installs.progress.has(g.id)} favorite={favorites.ids.has(g.id)} onOpen={onOpenCard} />
                  </li>
                ))}
              </ul>
            </section>
          ))}
        {!showPlatforms && !q && group && lazyUseful && page?.hasMore && (
          <LoadMore loading={page.loading} onVisible={() => !page.loading && loadConsole(group, page.ids.length)} />
        )}
      </main>
    </div>,
  );
}

function viewTitle(tab: Tab, group: ConsoleGroup | null): string {
  if (tab === "recent") return "Derniers ajouts";
  if (tab === "favorites") return "Mes favoris";
  return group ? group.name : "Plateformes";
}

function emptyMessage(tab: Tab, q: string, favoritesLoading: boolean, kind: Kind): string {
  if (q) return "Aucun jeu ne correspond.";
  if (kind === "installed") return "Aucun jeu installé.";
  if (kind === "recomp") return "Aucune recomp ici.";
  if (kind === "emulation") return "Aucun jeu avec un émulateur ici.";
  if (tab === "favorites") return favoritesLoading ? "Chargement des favoris…" : "Aucun favori pour l'instant : ouvrez un jeu et cliquez « ☆ Favori ».";
  return "Aucun jeu sur le serveur.";
}

/** Jeux déjà triés du plus récent au plus ancien → un groupe par jour d'ajout. */
function groupByDay(games: Game[]): { day: string; games: Game[] }[] {
  const groups: { day: string; games: Game[] }[] = [];
  for (const g of games) {
    const day = dayOf(g);
    const last = groups[groups.length - 1];
    if (last?.day === day) last.games.push(g);
    else groups.push({ day, games: [g] });
  }
  return groups;
}

/** « Portage GameCube » → « GameCube » : tous les jeux sont des portages, le mot n'apprend rien sur la carte. */
function shortPlatformName(name: string): string {
  return name.replace(/^portages?\s+/i, "").trim() || name;
}

/**
 * Grille où seules les rangées proches de l'écran existent : une console compte des
 * milliers de jeux, et les recréer tous au retour d'une fiche figerait la fenêtre.
 * Les rangées absentes sont remplacées par une marge de la même hauteur.
 */
function VirtualGrid({ games, initialScroll, renderCard }: { games: Game[]; initialScroll: { current: number }; renderCard: (g: Game) => ReactNode }) {
  const list = useRef<HTMLUListElement>(null);
  const [metrics, setMetrics] = useState<{ cols: number; rowH: number } | null>(null);
  const [top, setTop] = useState(0);
  const [viewH, setViewH] = useState(window.innerHeight);
  const restored = useRef(false);

  useLayoutEffect(() => {
    const ul = list.current;
    const scroller = ul?.closest(".lib-grid-wrap");
    if (!ul || !scroller) return;
    const measure = () => {
      const first = ul.firstElementChild as HTMLElement | null;
      if (!first) return;
      const style = getComputedStyle(ul);
      const cols = Math.max(1, style.gridTemplateColumns.split(" ").length);
      const rowH = first.offsetHeight + (parseFloat(style.rowGap) || 0);
      setMetrics((m) => (m && m.cols === cols && m.rowH === rowH ? m : { cols, rowH }));
      setViewH(scroller.clientHeight);
    };
    const update = () => setTop(Math.max(0, scroller.scrollTop - ul.offsetTop));
    let frame = 0;
    const onScroll = () => {
      if (!frame) {
        frame = requestAnimationFrame(() => {
          frame = 0;
          update();
        });
      }
    };
    const ro = new ResizeObserver(measure);
    ro.observe(ul);
    ro.observe(scroller);
    scroller.addEventListener("scroll", onScroll, { passive: true });
    measure();
    update();
    return () => {
      ro.disconnect();
      scroller.removeEventListener("scroll", onScroll);
      cancelAnimationFrame(frame);
    };
  }, []);

  const rows = metrics ? Math.ceil(games.length / metrics.cols) : 0;
  let start = 0;
  let end = games.length;
  if (metrics && metrics.rowH > 0) {
    const firstRow = Math.max(0, Math.floor(top / metrics.rowH) - OVERSCAN_ROWS);
    const lastRow = Math.min(rows, Math.ceil((top + viewH) / metrics.rowH) + OVERSCAN_ROWS);
    start = firstRow * metrics.cols;
    end = Math.min(games.length, lastRow * metrics.cols);
  } else {
    // Avant la mesure : juste de quoi mesurer une carte.
    end = Math.min(games.length, 1);
  }
  const before = metrics ? Math.floor(start / metrics.cols) * metrics.rowH : 0;
  const after = metrics ? Math.max(0, (rows - Math.ceil(end / metrics.cols)) * metrics.rowH) : 0;

  // Retour d'une fiche : la position d'avant, une fois la hauteur de la liste connue.
  useLayoutEffect(() => {
    if (restored.current || !metrics) return;
    const scroller = list.current?.closest(".lib-grid-wrap");
    if (!scroller) return;
    restored.current = true;
    if (initialScroll.current > 0 && games.length > 0) {
      scroller.scrollTop = initialScroll.current;
      setTop(Math.max(0, scroller.scrollTop - (list.current?.offsetTop ?? 0)));
    }
  }, [metrics, games.length, initialScroll]);

  return (
    <ul className="lib-grid" ref={list} style={{ paddingTop: before, paddingBottom: after }}>
      {games.slice(start, end).map((g) => (
        <li key={g.id}>{renderCard(g)}</li>
      ))}
    </ul>
  );
}

/** Page suivante d'une console, chargée quand le bas de la liste arrive à l'écran. */
function LoadMore({ loading, onVisible }: { loading: boolean; onVisible: () => void }) {
  const el = useRef<HTMLDivElement>(null);
  const latest = useRef({ loading, onVisible });
  latest.current = { loading, onVisible };
  // Position relue à chaque défilement et à la fin de chaque page : une page peut laisser le bas
  // encore à l'écran, sans qu'aucun changement ne soit signalé.
  const check = useCallback(() => {
    const node = el.current;
    const scroller = node?.closest(".lib-grid-wrap");
    if (!node || !scroller || latest.current.loading) return;
    if (node.getBoundingClientRect().top < scroller.getBoundingClientRect().bottom + LOAD_MORE_MARGIN_PX) latest.current.onVisible();
  }, []);
  useEffect(() => {
    const scroller = el.current?.closest(".lib-grid-wrap");
    scroller?.addEventListener("scroll", check, { passive: true });
    return () => scroller?.removeEventListener("scroll", check);
  }, [check]);
  useEffect(() => {
    if (!loading) requestAnimationFrame(check);
  }, [loading, check]);
  return (
    <div className="lib-more" ref={el}>
      {loading && <span className="n-spinner" />}
    </div>
  );
}

function PlatformCard({ group, count, fresh, onOpen }: { group: ConsoleGroup; count: number; fresh: number; onOpen: () => void }) {
  const { src } = useFirstImage(group.iconUrls);
  return (
    <button className="pcard n-marquee-host" onClick={onOpen} title={group.name}>
      <span className="pcard-icon">{src ? <img src={src} alt="" draggable={false} /> : <span className="pcard-glyph">◆</span>}</span>
      <span className="pcard-name">{shortPlatformName(group.name)}</span>
      <span className="pcard-foot">
        <span className="pcard-count">
          {count} jeu{count > 1 ? "x" : ""}
        </span>
        {fresh > 0 && (
          <span className="pcard-new">
            <span className="n-new">NEW</span>
            {fresh} nouveau{fresh > 1 ? "x" : ""}
          </span>
        )}
      </span>
    </button>
  );
}

/** Comment jouer à ce jeu, s'il n'est pas installé. */
function playBadge(game: Game): { label: string; cls: string } | null {
  if (game.pc) return !isAvailable(game) && game.detailsLoaded ? { label: "Non disponible", cls: "card-badge--muted" } : null;
  const modes = playModes(game);
  if (modes.recomp && modes.emulation) return { label: "Recomp · Émulation", cls: "card-badge--recomp" };
  if (modes.recomp) return { label: "Recomp", cls: "card-badge--recomp" };
  if (modes.emulation) return { label: "Émulation", cls: "card-badge--emu" };
  if (game.recomp) return { label: "Non disponible", cls: "card-badge--muted" };
  return { label: "Pas d'émulateur disponible", cls: "card-badge--muted" };
}

const GameCard = memo(function GameCard({
  game,
  fresh,
  inst,
  busy,
  favorite,
  onOpen,
}: {
  game: Game;
  fresh: boolean;
  inst: InstalledGame | undefined;
  busy: boolean;
  favorite: boolean;
  onOpen: (id: number, el: HTMLElement) => void;
}) {
  const badge = busy
    ? { label: "Installation…", cls: "" }
    : hasUpdate(game, inst)
      ? { label: "Mise à jour disponible", cls: "" }
      : inst?.executable
        ? { label: "Installé", cls: "card-badge--ok" }
        : playBadge(game);
  const [kind, setKind] = useState<"box" | "flat" | null>(null);
  return (
    <button className={`card n-marquee-host${kind === "box" ? " card--box3d" : ""}${fresh ? " card--new" : ""}`} data-sound-hover="game" data-game-id={game.id} onClick={(e) => onOpen(game.id, e.currentTarget)} title={game.variant ? `${game.name} · ${game.variant}` : game.name}>
      <div className="card-cover">
        <BoxArt game={game} onKind={setKind} />
        {kind === null && !game.boxFaces && <span className="card-placeholder">{game.name}</span>}
        {fresh && <span className="n-new card-new">NEW</span>}
        {(game.variant || badge) && (
          <div className="card-tags">
            {game.variant && <span className="card-variant">{game.variant}</span>}
            {badge && <span className={`card-badge ${badge.cls}`}>{badge.label}</span>}
          </div>
        )}
        {favorite && (
          <span className="card-fav" title="Favori">
            ★
          </span>
        )}
      </div>
      <Marquee className="card-name" text={game.name} onHover />
    </button>
  );
});
