import { useEffect, useState } from "react";
import { backend, formatPlaytime, hasUpdate, formatSize, Game, GameStopped, InstallPhase, Target, TARGET_LABELS } from "../api/backend";
import { InstallsState } from "../hooks/useInstalls";
import { Dialog } from "../components/Dialog";

const PHASE_LABELS: Record<InstallPhase, string> = {
  prepare: "Préparation…",
  download: "Téléchargement",
  verify: "Vérification…",
  backup: "Sauvegarde des saves…",
  extract: "Extraction",
  finalize: "Finalisation…",
};

interface Props {
  game: Game;
  target: Target | null;
  installs: InstallsState;
  onError: (e: unknown) => Promise<string>;
}


export function InstallPanel({ game, target, installs, onError }: Props) {
  const inst = installs.installed.get(game.id);
  const progress = installs.progress.get(game.id);
  const error = installs.errors.get(game.id);
  const [message, setMessage] = useState<string | null>(null);
  const [dialog, setDialog] = useState<"exe" | "uninstall" | null>(null);
  const [candidates, setCandidates] = useState<string[]>([]);

  const downloadSize = game.myArchive ? game.myArchive.file.size + game.romFiles.reduce((s, f) => s + f.size, 0) : 0;
  const update = hasUpdate(game, inst);
  const pendingChoice = !!inst && !inst.executable && !progress;
  const runningSince = installs.running.get(game.id);
  const last = installs.lastSession.get(game.id);

  async function run<T>(f: () => Promise<T>): Promise<T | undefined> {
    setMessage(null);
    try {
      return await f();
    } catch (e) {
      setMessage(await onError(e));
      return undefined;
    }
  }

  async function openExeChoice() {
    const list = inst?.executable ? await run(() => backend.executableCandidates(game.id)) : inst?.pendingCandidates;
    if (list) {
      setCandidates(list);
      setDialog("exe");
    }
  }

  return (
    <div className="n-panel gd-actions">
      <div className="n-panel-header">{inst?.executable ? "Jeu installé" : "Installation"}</div>
      <div className="n-panel-body gd-actions-body">
        {progress ? (
          <ProgressView phase={progress.phase} done={progress.done} total={progress.total} onCancel={() => backend.cancelInstall(game.id)} />
        ) : !game.myArchive && !game.detailsLoaded ? (
          <p>
            <span className="n-spinner" /> Lecture des fichiers sur le serveur…
          </p>
        ) : inst?.executable ? (
          <>
            {runningSince !== undefined ? (
              <div className="gd-running">
                <span className="n-spinner" /> En cours de jeu · <Elapsed since={runningSince} />
              </div>
            ) : (
              <button
                className="n-btn n-btn--primary n-btn--block n-cursor"
                onClick={async () => {
                  setMessage(null);
                  const err = await installs.play(game.id);
                  if (err) setMessage(err);
                }}
              >
                Jouer
              </button>
            )}
            {last && <p className="n-hint">{sessionSummary(last)}</p>}
            {update && runningSince === undefined && (
              <button className="n-btn n-btn--block gd-update" onClick={() => installs.start(game.id)}>
                Mettre à jour ({inst.versionRaw ? inst.version : "?"} → {game.serverVersion})
              </button>
            )}
          </>
        ) : pendingChoice ? (
          <>
            <p className="n-notice">Plusieurs exécutables ont été trouvés. Choisissez celui qui lance le jeu.</p>
            <button className="n-btn n-btn--primary n-btn--block" onClick={openExeChoice}>
              Choisir l'exécutable
            </button>
          </>
        ) : game.myArchive ? (
          <>
            <button className="n-btn n-btn--primary n-btn--block" onClick={() => installs.start(game.id)}>
              Installer
            </button>
            {installs.installRoot && <p className="n-hint">Dans {installs.installRoot}</p>}
          </>
        ) : (
          <p className="n-alert">Non disponible pour votre système{target ? ` (${TARGET_LABELS[target]})` : ""}.</p>
        )}

        {error && <p className="n-alert">{error.message}</p>}
        {message && <p className="n-alert">{message}</p>}

        <dl className="gd-facts">
          <dt>Version serveur</dt>
          <dd>{game.serverVersion ?? "inconnue"}</dd>
          <dt>Version installée</dt>
          <dd>{inst?.executable ? inst.version ?? "inconnue" : "Non installé"}</dd>
          {inst?.executable && (
            <>
              <dt>Exécutable</dt>
              <dd>{inst.executable}</dd>
            </>
          )}
          {!inst && game.myArchive && (
            <>
              <dt>Téléchargement</dt>
              <dd>{formatSize(downloadSize)}</dd>
            </>
          )}
        </dl>

        {inst && !progress && runningSince === undefined && (
          <div className="gd-inst-actions">
            <button className="n-link" onClick={() => run(() => backend.openGameFolder(game.id))}>
              Ouvrir le dossier du jeu
            </button>
            {inst.executable && (
              <button className="n-link" onClick={openExeChoice}>
                Changer l'exécutable
              </button>
            )}
            <button className="n-link" onClick={() => setDialog("uninstall")}>
              Désinstaller
            </button>
          </div>
        )}

        <div>
          <p className="n-label">Plateformes disponibles</p>
          <ul className="gd-platforms">
            {game.archives.length === 0 && <li className="n-hint">aucune</li>}
            {game.archives.map((a) => (
              <li key={a.target} className={a.target === game.myArchive?.target ? "is-mine" : undefined}>
                {TARGET_LABELS[a.target]}
              </li>
            ))}
          </ul>
        </div>
      </div>

      {dialog === "exe" && inst && (
        <ExeChoiceDialog
          candidates={candidates}
          current={inst.executable}
          onClose={() => setDialog(null)}
          onChoose={async (exe) => {
            const updated = await run(() => backend.chooseExecutable(game.id, exe));
            if (updated) {
              installs.setInstalled(updated);
              setDialog(null);
            }
          }}
        />
      )}
      {dialog === "uninstall" && inst && (
        <UninstallDialog
          name={game.name}
          onClose={() => setDialog(null)}
          onConfirm={async (keepSaves) => {
            setDialog(null);
            const ok = await run(() => backend.uninstallGame(game.id, keepSaves).then(() => true));
            if (ok) installs.remove(game.id);
          }}
        />
      )}
    </div>
  );
}

function ProgressView({ phase, done, total, onCancel }: { phase: InstallPhase; done: number; total: number; onCancel: () => void }) {
  const pct = total > 0 ? Math.min(100, (done / total) * 100) : null;
  const detail = phase === "download" && total > 0 ? `${formatSize(done)} / ${formatSize(total)}` : pct !== null ? `${Math.round(pct)} %` : "";
  return (
    <div className="gd-progress">
      <div className="gd-progress-label">
        <span>{PHASE_LABELS[phase]}</span>
        <span>{detail}</span>
      </div>
      <div className="gd-progress-bar" role="progressbar" aria-valuenow={pct ?? undefined} aria-valuemin={0} aria-valuemax={100}>
        <div className={pct === null ? "is-indeterminate" : undefined} style={pct === null ? undefined : { width: `${pct}%` }} />
      </div>
      {phase !== "finalize" && (
        <button className="n-link" onClick={onCancel}>
          Annuler
        </button>
      )}
    </div>
  );
}

function ExeChoiceDialog({
  candidates,
  current,
  onClose,
  onChoose,
}: {
  candidates: string[];
  current: string | null;
  onClose: () => void;
  onChoose: (exe: string) => Promise<void>;
}) {
  const [choice, setChoice] = useState(current ?? candidates[0] ?? "");
  const [busy, setBusy] = useState(false);
  return (
    <Dialog
      title="Exécutable du jeu"
      onClose={onClose}
      actions={
        <>
          <button className="n-btn" onClick={onClose}>
            Annuler
          </button>
          <button
            className="n-btn"
            disabled={!choice || busy}
            onClick={async () => {
              setBusy(true);
              await onChoose(choice);
              setBusy(false);
            }}
          >
            Valider
          </button>
        </>
      }
    >
      <p>Quel fichier lance le jeu ? Ce choix est mémorisé et reste modifiable dans la fiche du jeu.</p>
      {candidates.length === 0 ? (
        <p className="n-alert">Aucun exécutable trouvé.</p>
      ) : (
        <ul className="choice-list">
          {candidates.map((c) => (
            <li key={c}>
              <label>
                <input type="radio" name="exe" checked={choice === c} onChange={() => setChoice(c)} />
                {c}
              </label>
            </li>
          ))}
        </ul>
      )}
    </Dialog>
  );
}

function UninstallDialog({ name, onClose, onConfirm }: { name: string; onClose: () => void; onConfirm: (keepSaves: boolean) => void }) {
  const [keepSaves, setKeepSaves] = useState(true);
  return (
    <Dialog
      title="Désinstaller"
      onClose={onClose}
      actions={
        <>
          <button className="n-btn" onClick={onClose}>
            Annuler
          </button>
          <button className="n-btn" onClick={() => onConfirm(keepSaves)}>
            Désinstaller
          </button>
        </>
      }
    >
      <p>
        Supprimer <strong>{name}</strong> de cet ordinateur ?
      </p>
      <label className="n-check">
        <input type="checkbox" checked={keepSaves} onChange={(e) => setKeepSaves(e.target.checked)} />
        Conserver les sauvegardes (dossiers save / saves)
      </label>
    </Dialog>
  );
}


function sessionSummary(s: GameStopped): string {
  if (!s.recorded) return "Session trop courte : non comptée.";
  const base = `Dernière session : ${formatPlaytime(s.durationMs)}`;
  return s.pending > 0 ? `${base} (sera envoyée à RomM dès que le serveur répondra)` : `${base}, enregistrée.`;
}

function Elapsed({ since }: { since: number }) {
  const [now, setNow] = useState(Date.now());
  useEffect(() => {
    const t = window.setInterval(() => setNow(Date.now()), 1000);
    return () => window.clearInterval(t);
  }, []);
  const s = Math.max(0, Math.floor((now - since) / 1000));
  const h = Math.floor(s / 3600);
  const m = Math.floor((s % 3600) / 60);
  const sec = s % 60;
  return <span>{h > 0 ? `${h}:${String(m).padStart(2, "0")}` : m}:{String(sec).padStart(2, "0")}</span>;
}
