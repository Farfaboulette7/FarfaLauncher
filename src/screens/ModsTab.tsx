import { useCallback, useEffect, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { backend, formatSize, Game, MOD_FINISHED_EVENT, MOD_PROGRESS_EVENT, ModFinished, ModProgress } from "../api/backend";
import { InstallsState } from "../hooks/useInstalls";

interface Props {
  game: Game;
  installs: InstallsState;
  onError: (e: unknown) => Promise<string>;
}

/** Archives du dossier MODS/ : téléchargées telles quelles dans mods_downloads/. */
export function ModsTab({ game, installs, onError }: Props) {
  const installed = !!installs.installed.get(game.id)?.executable;
  const [downloaded, setDownloaded] = useState<Set<string>>(new Set());
  const [progress, setProgress] = useState(new Map<number, ModProgress>());
  const [message, setMessage] = useState<string | null>(null);

  const reload = useCallback(() => {
    if (installed) backend.downloadedMods(game.id).then((names) => setDownloaded(new Set(names)));
  }, [game.id, installed]);

  useEffect(() => {
    reload();
    const subs = [
      listen<ModProgress>(MOD_PROGRESS_EVENT, ({ payload }) => {
        if (payload.romId === game.id) setProgress((p) => new Map(p).set(payload.fileId, payload));
      }),
      listen<ModFinished>(MOD_FINISHED_EVENT, async ({ payload }) => {
        if (payload.romId !== game.id) return;
        setProgress((p) => {
          const next = new Map(p);
          next.delete(payload.fileId);
          return next;
        });
        if (payload.error) setMessage(await onError(payload.error));
        reload();
      }),
    ];
    return () => subs.forEach((s) => s.then((f) => f()));
  }, [game.id, reload, onError]);

  async function download(fileId: number, size: number) {
    setMessage(null);
    setProgress((p) => new Map(p).set(fileId, { romId: game.id, fileId, done: 0, total: size }));
    try {
      await backend.downloadMod(game.id, fileId);
    } catch (e) {
      setProgress((p) => {
        const next = new Map(p);
        next.delete(fileId);
        return next;
      });
      setMessage(await onError(e));
    }
  }

  if (game.mods.length === 0) {
    return game.detailsLoaded ? (
      <p className="n-hint">Aucun mod disponible.</p>
    ) : (
      <p>
        <span className="n-spinner" /> Lecture des fichiers sur le serveur…
      </p>
    );
  }

  return (
    <div className="gd-mods-tab">
      <p className="n-hint">
        Les mods sont téléchargés dans le dossier <strong>mods_downloads</strong> du jeu, sans être installés : placez-les
        vous-même selon les instructions du mod.
      </p>
      {!installed && <p className="n-notice">Installez d'abord le jeu pour télécharger ses mods.</p>}
      {message && <p className="n-alert">{message}</p>}
      <ul className="gd-mods">
        {game.mods.map((m) => {
          const p = progress.get(m.id);
          const isDone = downloaded.has(m.fileName);
          return (
            <li key={m.id}>
              <div className="gd-mod-info">
                <span className="gd-mod-name">{m.fileName}</span>
                <span className="n-hint">
                  {formatSize(m.size)}
                  {isDone && !p && " · téléchargé"}
                </span>
              </div>
              {p ? (
                <span className="gd-mod-progress">{p.total > 0 ? `${Math.round((p.done / p.total) * 100)} %` : "…"}</span>
              ) : (
                <button className="n-btn" disabled={!installed || isDone} onClick={() => download(m.id, m.size)}>
                  {isDone ? "Téléchargé" : "Télécharger"}
                </button>
              )}
            </li>
          );
        })}
      </ul>
      {installed && (
        <div className="gd-inst-actions">
          <button className="n-link" onClick={() => backend.openGameFolder(game.id).catch(onError)}>
            Ouvrir le dossier du jeu
          </button>
          <button className="n-link" onClick={() => backend.openGameFolder(game.id, "mods_downloads").catch(onError)}>
            Ouvrir le dossier des mods téléchargés
          </button>
        </div>
      )}
    </div>
  );
}
