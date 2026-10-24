import { useCallback, useEffect, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import {
  AppError,
  GAME_STARTED_EVENT,
  GAME_STOPPED_EVENT,
  GameStopped,
  backend,
  INSTALL_FINISHED_EVENT,
  INSTALL_PROGRESS_EVENT,
  InstallFinished,
  InstalledGame,
  InstallProgress,
} from "../api/backend";

export interface InstallsState {
  installRoot: string | null;
  installed: Map<number, InstalledGame>;
  progress: Map<number, InstallProgress>;
  /** Dernière erreur d'installation par jeu (effacée au prochain essai). */
  errors: Map<number, AppError>;
  reload: () => Promise<void>;
  start: (romId: number) => Promise<void>;
  setInstalled: (game: InstalledGame) => void;
  remove: (romId: number) => void;
  /** Jeux en cours (id → heure de lancement, ms). */
  running: Map<number, number>;
  /** Dernière session terminée par jeu. */
  lastSession: Map<number, GameStopped>;
  play: (romId: number) => Promise<string | null>;
}

/** État des installations, tenu à jour par les événements du Rust. */
export function useInstalls(onAuthError: (e: unknown) => Promise<string>): InstallsState {
  const [installRoot, setInstallRoot] = useState<string | null>(null);
  const [installed, setInstalledMap] = useState(new Map<number, InstalledGame>());
  const [progress, setProgress] = useState(new Map<number, InstallProgress>());
  const [errors, setErrors] = useState(new Map<number, AppError>());
  const [running, setRunning] = useState(new Map<number, number>());
  const [lastSession, setLastSession] = useState(new Map<number, GameStopped>());

  const reload = useCallback(async () => {
    const [info, runningIds] = await Promise.all([backend.installInfo(), backend.runningGames()]);
    setRunning((r) => new Map(runningIds.map((id) => [id, r.get(id) ?? Date.now()])));
    setInstallRoot(info.installRoot);
    setInstalledMap(new Map(info.games.map((g) => [g.romId, g])));
    setProgress((p) => {
      const next = new Map(p);
      for (const id of info.active) if (!next.has(id)) next.set(id, { romId: id, phase: "prepare", done: 0, total: 0 });
      return next;
    });
  }, []);

  useEffect(() => {
    reload();
    const subs = [
      listen<number>(GAME_STARTED_EVENT, ({ payload }) => setRunning((r) => new Map(r).set(payload, Date.now()))),
      listen<GameStopped>(GAME_STOPPED_EVENT, ({ payload }) => {
        setRunning((r) => {
          const next = new Map(r);
          next.delete(payload.romId);
          return next;
        });
        setLastSession((m) => new Map(m).set(payload.romId, payload));
      }),
      listen<InstallProgress>(INSTALL_PROGRESS_EVENT, ({ payload }) =>
        setProgress((p) => new Map(p).set(payload.romId, payload)),
      ),
      listen<InstallFinished>(INSTALL_FINISHED_EVENT, async ({ payload }) => {
        setProgress((p) => {
          const next = new Map(p);
          next.delete(payload.romId);
          return next;
        });
        if (payload.outcome) {
          const game = payload.outcome.game;
          setInstalledMap((m) => new Map(m).set(game.romId, game));
        } else if (payload.error && payload.error.code !== "cancelled") {
          await onAuthError(payload.error);
          const err = payload.error;
          setErrors((m) => new Map(m).set(payload.romId, err));
        }
      }),
    ];
    return () => subs.forEach((s) => s.then((f) => f()));
  }, [reload, onAuthError]);

  const start = useCallback(
    async (romId: number) => {
      setErrors((m) => {
        const next = new Map(m);
        next.delete(romId);
        return next;
      });
      setProgress((p) => new Map(p).set(romId, { romId, phase: "prepare", done: 0, total: 0 }));
      try {
        await backend.installGame(romId);
      } catch (e) {
        setProgress((p) => {
          const next = new Map(p);
          next.delete(romId);
          return next;
        });
        await onAuthError(e);
        if (typeof e === "object" && e && "code" in e) setErrors((m) => new Map(m).set(romId, e as AppError));
      }
    },
    [onAuthError],
  );

  const setInstalled = useCallback((game: InstalledGame) => setInstalledMap((m) => new Map(m).set(game.romId, game)), []);
  const remove = useCallback(
    (romId: number) =>
      setInstalledMap((m) => {
        const next = new Map(m);
        next.delete(romId);
        return next;
      }),
    [],
  );

  /** Lance le jeu ; renvoie un message d'erreur ou null. */
  const play = useCallback(
    async (romId: number): Promise<string | null> => {
      try {
        await backend.playGame(romId);
        return null;
      } catch (e) {
        return onAuthError(e);
      }
    },
    [onAuthError],
  );

  return { installRoot, installed, progress, errors, reload, start, setInstalled, remove, running, lastSession, play };
}
