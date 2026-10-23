import { useCallback, useEffect, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import {
  AppError,
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
}

/** État des installations, tenu à jour par les événements du Rust. */
export function useInstalls(onAuthError: (e: unknown) => Promise<string>): InstallsState {
  const [installRoot, setInstallRoot] = useState<string | null>(null);
  const [installed, setInstalledMap] = useState(new Map<number, InstalledGame>());
  const [progress, setProgress] = useState(new Map<number, InstallProgress>());
  const [errors, setErrors] = useState(new Map<number, AppError>());

  const reload = useCallback(async () => {
    const info = await backend.installInfo();
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

  return { installRoot, installed, progress, errors, reload, start, setInstalled, remove };
}
