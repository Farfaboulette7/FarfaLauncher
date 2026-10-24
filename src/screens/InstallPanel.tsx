import { useEffect, useState } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import {
  backend,
  formatPlaytime,
  hasUpdate,
  formatSize,
  Game,
  GameStopped,
  InstallPhase,
  isAppError,
  SaveChoice,
  Target,
  TARGET_LABELS,
} from "../api/backend";
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
  /** Question sur les sauvegardes avant de jouer. */
  const [saveAsk, setSaveAsk] = useState<
    | { kind: "where"; suggestions: string[]; thenPlay: boolean }
    | { kind: "conflict"; localModified: number | null; serverModified: string }
    | null
  >(null);
  const [launching, setLaunching] = useState(false);
  const [saveNotice, setSaveNotice] = useState<string | null>(null);
  const [candidates, setCandidates] = useState<string[]>([]);

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

  async function launch(choice: SaveChoice) {
    setSaveAsk(null);
    setLaunching(true);
    const err = await installs.play(game.id, choice);
    setLaunching(false);
    if (err) setMessage(err);
  }

  /** « Jouer » : compare d'abord la sauvegarde locale à celle du serveur. */
  async function startPlay() {
    setMessage(null);
    setSaveNotice(null);
    setLaunching(true);
    try {
      const check = await backend.saveCheck(game.id);
      setLaunching(false);
      if (check.status === "unknown") setSaveAsk({ kind: "where", suggestions: check.suggestions, thenPlay: true });
      else if (check.status === "conflict") setSaveAsk({ kind: "conflict", ...check });
      else await launch("unchanged");
    } catch (e) {
      // Sauvegardes indisponibles (droits, serveur…) : on joue quand même.
      setLaunching(false);
      const msg = await onError(e);
      setSaveNotice(isAppError(e) && e.code === "scope_missing" ? msg : `Sauvegardes non vérifiées : ${msg}`);
      await launch("unchanged");
    }
  }

  async function chooseSaveDir(dir: string | null, off: boolean, thenPlay: boolean) {
    const updated = await run(() => backend.setSaveLocation(game.id, dir, off));
    if (!updated) return;
    installs.setInstalled(updated);
    setSaveAsk(null);
    if (thenPlay) await startPlay();
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
                data-sound="none"
                disabled={launching}
                onClick={startPlay}
              >
                {launching ? "Vérification des sauvegardes…" : "Jouer"}
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
            <p className="n-hint">
              {formatSize(downloadSize(game))}
              {installs.installRoot && ` · dans ${installs.installRoot}`}
            </p>
          </>
        ) : (
          <p className="n-alert" data-sound="none">Non disponible pour votre système{target ? ` (${TARGET_LABELS[target]})` : ""}.</p>
        )}

        {error && <p className="n-alert">{error.message}</p>}
        {message && <p className="n-alert">{message}</p>}
        {saveNotice && <p className="n-notice">{saveNotice}</p>}

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
            {inst.executable && inst.saves.dir && !inst.saves.off && (
              <button className="n-link" onClick={() => run(() => backend.openSaveFolder(game.id))}>
                Ouvrir les sauvegardes
              </button>
            )}
            {inst.executable && (
              <button className="n-link" onClick={() => setSaveAsk({ kind: "where", suggestions: [], thenPlay: false })}>
                Dossier des sauvegardes…
              </button>
            )}
            <button className="n-link" onClick={() => setDialog("uninstall")}>
              Désinstaller
            </button>
          </div>
        )}
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
      {saveAsk?.kind === "where" && (
        <SaveDirDialog
          name={game.name}
          suggestions={saveAsk.suggestions}
          current={inst?.saves.dir ?? null}
          playing={saveAsk.thenPlay}
          onClose={() => setSaveAsk(null)}
          onChoose={(dir, off) => chooseSaveDir(dir, off, saveAsk.thenPlay)}
        />
      )}
      {saveAsk?.kind === "conflict" && (
        <SaveConflictDialog
          localModified={saveAsk.localModified}
          serverModified={saveAsk.serverModified}
          onClose={() => setSaveAsk(null)}
          onChoose={launch}
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

function downloadSize(game: Game): number {
  return game.myArchive ? game.myArchive.file.size + game.romFiles.reduce((s, f) => s + f.size, 0) : 0;
}

/** Détails de l'installation, affichés dans l'onglet « Informations ». */
export function InstallFacts({ game, installs }: { game: Game; installs: InstallsState }) {
  const inst = installs.installed.get(game.id);
  return (
    <>
      <dt>Version serveur</dt>
      <dd>{game.serverVersion ?? "inconnue"}</dd>
      <dt>Version installée</dt>
      <dd>{inst?.executable ? inst.version ?? "inconnue" : "non installé"}</dd>
      {inst?.executable && (
        <>
          <dt>Exécutable</dt>
          <dd>{inst.executable}</dd>
          <dt>Sauvegardes</dt>
          <dd className="gd-savedir" title={inst.saves.dir ?? undefined}>
            {inst.saves.off ? "non synchronisées" : inst.saves.dir ? shortPath(inst.saves.dir) : "dossier inconnu (demandé au lancement)"}
          </dd>
        </>
      )}
      {!inst && game.myArchive && (
        <>
          <dt>Téléchargement</dt>
          <dd>{formatSize(downloadSize(game))}</dd>
        </>
      )}
      <dt>Plateformes</dt>
      <dd>
        <ul className="gd-platforms">
          {game.archives.length === 0 && <li className="n-hint">aucune</li>}
          {game.archives.map((a) => (
            <li key={a.target} className={a.target === game.myArchive?.target ? "is-mine" : undefined}>
              {TARGET_LABELS[a.target]}
            </li>
          ))}
        </ul>
      </dd>
    </>
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
  const time = !s.recorded
    ? "Session trop courte : non comptée."
    : s.pending > 0
      ? `Dernière session : ${formatPlaytime(s.durationMs)} (sera envoyée à RomM dès que le serveur répondra).`
      : `Dernière session : ${formatPlaytime(s.durationMs)}, enregistrée.`;
  const save =
    s.save === "uploaded"
      ? " Sauvegarde envoyée sur le serveur."
      : s.save === "failed"
        ? ` Sauvegarde non envoyée : ${s.saveError?.message ?? "erreur inconnue"}`
        : "";
  return time + save;
}

function shortPath(p: string): string {
  const parts = p.split(/[\\/]/).filter(Boolean);
  return parts.length > 3 ? `…/${parts.slice(-3).join("/")}` : p;
}

function formatDate(d: Date): string {
  return d.toLocaleString("fr-FR", { dateStyle: "short", timeStyle: "short" });
}

function SaveConflictDialog({
  localModified,
  serverModified,
  onClose,
  onChoose,
}: {
  localModified: number | null;
  serverModified: string;
  onClose: () => void;
  onChoose: (choice: SaveChoice) => void;
}) {
  const server = new Date(serverModified);
  return (
    <Dialog
      title="Sauvegardes différentes"
      onClose={onClose}
      actions={
        <>
          <button className="n-btn" onClick={() => onChoose("server")}>
            Prendre celle du serveur
          </button>
          <button className="n-btn" autoFocus onClick={() => onChoose("local")}>
            Garder la locale
          </button>
        </>
      }
    >
      <p>La sauvegarde de cet appareil n'est pas la même que celle du serveur.</p>
      <dl className="gd-facts">
        <dt>Sur cet appareil</dt>
        <dd>{localModified ? `modifiée le ${formatDate(new Date(localModified * 1000))}` : "aucune sauvegarde"}</dd>
        <dt>Sur le serveur</dt>
        <dd>{isNaN(server.getTime()) ? "présente" : `envoyée le ${formatDate(server)}`}</dd>
      </dl>
      <p className="n-hint">
        « Garder la locale » : vous continuez votre partie d'ici, et le serveur est mis à jour à la fin. « Prendre celle
        du serveur » remplace la sauvegarde de cet appareil (une copie de secours est gardée dans le dossier du jeu).
      </p>
    </Dialog>
  );
}

function SaveDirDialog({
  name,
  suggestions,
  current,
  playing,
  onClose,
  onChoose,
}: {
  name: string;
  suggestions: string[];
  current: string | null;
  playing: boolean;
  onClose: () => void;
  onChoose: (dir: string | null, off: boolean) => void;
}) {
  async function browse() {
    const dir = await open({
      directory: true,
      multiple: false,
      title: `Dossier des sauvegardes de ${name}`,
      defaultPath: suggestions[0] ?? current ?? undefined,
    });
    if (typeof dir === "string") onChoose(dir, false);
  }
  return (
    <Dialog
      title="Dossier des sauvegardes"
      onClose={onClose}
      actions={
        <>
          <button className="n-btn" onClick={() => onChoose(null, true)}>
            {playing ? "Jouer sans synchroniser" : "Ne pas synchroniser"}
          </button>
          <button className="n-btn" autoFocus onClick={browse}>
            Choisir le dossier…
          </button>
        </>
      }
    >
      {playing ? (
        <p>
          FaRFa project n'a pas trouvé où <strong>{name}</strong> range ses sauvegardes. Indiquez le dossier : elles seront
          copiées sur le serveur et retrouvées sur vos autres appareils.
        </p>
      ) : (
        <p>
          Dossier actuel : <strong>{current ?? "inconnu"}</strong>
        </p>
      )}
      {suggestions.length > 0 && (
        <>
          <p className="n-label">Dossiers de données trouvés pour ce jeu :</p>
          <ul className="choice-list">
            {suggestions.map((s) => (
              <li key={s}>
                <button className="n-link" onClick={() => onChoose(s, false)}>
                  {s}
                </button>
              </li>
            ))}
          </ul>
        </>
      )}
      <p className="n-hint">
        Astuce : la documentation de la recomp indique souvent l'emplacement (un dossier « saves », des fichiers .sav…).
        Choisissez le dossier qui ne contient que les sauvegardes.
      </p>
      {!playing && current && (
        <button className="n-link" onClick={() => onChoose(null, false)}>
          Réinitialiser (rechercher à nouveau automatiquement)
        </button>
      )}
    </Dialog>
  );
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
