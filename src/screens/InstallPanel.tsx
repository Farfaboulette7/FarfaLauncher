import { Fragment, useEffect, useRef, useState } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import {
  backend,
  Bo3Language,
  CodChoice,
  errorMessage,
  CodModeInfo,
  downloadSize,
  emulatorDownloadSize,
  emulatorReady,
  formatPlaytime,
  hasUpdate,
  isEmulatorInstall,
  formatSize,
  EmulatorOffer,
  Game,
  isAvailable,
  withEffectiveEmulator,
  PcKind,
  ProtonVersion,
  GameStopped,
  isWaiting,
  Crash,
  InstallPhase,
  isAppError,
  SaveChoice,
  SaveVersion,
  SharedShaders,
  Optimizations,
  RecompOffer,
  InstallWay,
  Target,
  TARGET_LABELS,
  InstalledGame,
} from "../api/backend";
import { InstallsState } from "../hooks/useInstalls";
import { InstallChoice, InstallMode, useInstallChoice } from "../hooks/useInstallChoice";
import { Dialog } from "../components/Dialog";
import { REPORT_SENT_EVENT } from "../components/ReportsInbox";
import { LaunchOverlay } from "../components/LaunchOverlay";
import { ChangelogFact } from "../components/ChangelogButton";
import { useOffline } from "../offline";

export const PHASE_LABELS: Record<InstallPhase, string> = {
  prepare: "Préparation…",
  proton: "Téléchargement de Proton",
  download: "Téléchargement",
  verify: "Vérification…",
  backup: "Sauvegarde des saves…",
  extract: "Extraction du jeu",
  decrypt: "Déchiffrement du jeu",
  data: "Extraction des données du jeu",
  place: "Rangement des fichiers…",
  installer: "Installation",
  client: "Téléchargement du client",
  build: "Construction du jeu",
  check: "Vérification de votre copie du jeu",
  copy: "Copie du jeu",
  generate: "Traduction du code du jeu",
  compile: "Compilation du jeu",
  warm: "Premier démarrage du jeu",
  cleanup: "Nettoyage",
  shaders: "Préparation des shaders",
  cache: "Préparation des données du jeu",
  finalize: "Finalisation…",
  queued: "En attente",
  paused: "En pause",
};

/** Infobulle des boutons qui demandent le serveur. */
const OFFLINE_TITLE = "Indisponible hors ligne";

/** Plantages déjà proposés au joueur : la proposition ne s'ouvre qu'une fois par partie. */
const crashesProposed = new WeakSet<GameStopped>();

interface Props {
  game: Game;
  target: Target | null;
  installs: InstallsState;
  onError: (e: unknown) => Promise<string>;
  /** Lancer le jeu dès qu'il est prêt (raccourci Steam). */
  autoPlay?: boolean;
  onAutoPlayed?: () => void;
}

/** « Ajouter à Steam » : raccourci qui lance le jeu par le launcher, avec ses images. */
function SteamLink({ romId, onError }: { romId: number; onError: (e: unknown) => Promise<string> }) {
  const [info, setInfo] = useState<{ available: boolean; added: boolean } | null>(null);
  const [state, setState] = useState<"idle" | "adding" | "done">("idle");
  const [notice, setNotice] = useState<string | null>(null);
  useEffect(() => {
    let alive = true;
    backend
      .steamInfo(romId)
      .then((i) => alive && setInfo(i))
      .catch(() => undefined);
    return () => {
      alive = false;
    };
  }, [romId]);
  if (!info?.available) return null;
  async function add() {
    setState("adding");
    setNotice(null);
    try {
      const r = await backend.addToSteam(romId);
      setState("done");
      setInfo({ available: true, added: true });
      setNotice(
        `${r.updated ? "Raccourci Steam mis à jour" : "Ajouté à Steam"} ✓ ${
          r.steamRunning ? "Redémarrez Steam (Steam → Quitter, puis rouvrez-le) pour le voir." : "Il apparaîtra à la prochaine ouverture de Steam."
        }`,
      );
    } catch (e) {
      setState("idle");
      setNotice(await onError(e));
    }
  }
  return (
    <>
      <button className="n-link" disabled={state === "adding"} onClick={() => void add()} title="Raccourci non-Steam qui lance le jeu par FaRFa project, avec sa jaquette">
        {state === "adding" ? "Ajout à Steam…" : info.added ? "Mettre à jour le raccourci Steam" : "Ajouter à Steam"}
      </button>
      {notice && <p className="n-notice gd-steam-notice">{notice}</p>}
    </>
  );
}


export function InstallPanel({ game: base, target, installs, onError, autoPlay, onAutoPlayed }: Props) {
  const inst = installs.installed.get(base.id);
  /** Serveur injoignable : jouer oui, installer ou mettre à jour non. */
  const offline = useOffline();
  /** Recomp ou émulation, et laquelle : choisi dans l'onglet « Informations » (section Installation). */
  const [choice, setChoice] = useInstallChoice(base.id);
  const mode = installMode(base, inst, choice);
  /** Mode choisi sur la page : le panneau entier parle de lui, installé ou non. */
  const sel = selectionOf(base, inst, choice);
  /** Le mode choisi n'est pas celui que le jeu installé lance aujourd'hui (installé à côté, ou pas encore). */
  const switching = !!inst?.executable && !!sel?.id && !sel.way?.active;
  const recompChoice = choice.recomp;
  const recompId = recompChoice ?? base.recompId ?? null;
  const recompOffer = recompList(base).find((r) => r.id === recompId) ?? null;
  const game = withEffectiveEmulator(base, inst, choice.emulator);
  /** Il y a un autre choix possible dans l'onglet « Informations ». */
  const hasAlternative = recompList(base).length + (base.emulators?.length ?? 0) > 1;
  const progress = installs.progress.get(game.id);
  const error = installs.errors.get(game.id);
  const [message, setMessage] = useState<string | null>(null);
  const [dialog, setDialog] = useState<"exe" | "uninstall" | "proton" | "cod" | "report" | "history" | null>(null);
  /** Rapport proposé juste après un plantage (le jeu s'est fermé tout seul). */
  const [reportAfterCrash, setReportAfterCrash] = useState(false);
  /** Proposition ouverte d'elle-même après le plantage (avec « Ne plus demander »). */
  const [reportProposed, setReportProposed] = useState(false);
  /** Question remise en place pour ce jeu (lien « Me reproposer… »). */
  const [crashPromptBack, setCrashPromptBack] = useState(false);
  const [reportSent, setReportSent] = useState<string | null>(null);
  /** Version de Proton choisie avant l'installation (`null` : la recommandée). */
  const [protonChoice, setProtonChoice] = useState<string | null>(null);
  /** Mode du Call of Duty pour la partie en cours de lancement. */
  const codChoice = useRef<CodChoice | null>(null);
  /** Cette partie ouvre le menu de la recomp plutôt que le jeu. */
  const openMenu = useRef(false);
  /** Écran « Lancement en cours » ; `started` : le jeu est lancé, on attend sa fenêtre. */
  const [overlay, setOverlay] = useState<{ started: boolean } | null>(null);
  /** Question sur les sauvegardes avant de jouer. */
  const [saveAsk, setSaveAsk] = useState<
    | { kind: "where"; suggestions: string[]; thenPlay: boolean }
    | { kind: "conflict"; localModified: number | null; serverModified: string }
    | null
  >(null);
  /** Étape en cours entre le clic sur « Jouer » et le lancement (libellé du bouton). */
  const [launching, setLaunching] = useState<string | null>(null);
  const [saveNotice, setSaveNotice] = useState<string | null>(null);
  const [candidates, setCandidates] = useState<string[]>([]);
  /** Désinstallation ou déplacement en cours (gardé même si on quitte la page). */
  const busy = installs.busy.get(game.id);
  /** Emplacement choisi pour cette installation (sinon le dossier des jeux). */
  const [installDir, setInstallDir] = useState<string | null>(null);

  const update = hasUpdate(game, inst);
  /** Runtime partagé déjà sur cet ordinateur et à jour : seule la ROM reste à télécharger. */
  const runtimeInst = game.runtime ? installs.installed.get(game.runtime.romId) : undefined;
  const runtimeReady = !!runtimeInst?.executable && !hasUpdate(game, runtimeInst);
  const emulatorInst = game.emulator ? installs.installed.get(game.emulator.runtimeRomId) : undefined;
  const emulatorUpToDate = !!emulatorInst?.executable && emulatorInst.versionRaw === game.emulator?.versionKey;
  const installedWithEmulator = isEmulatorInstall(inst);
  const pendingChoice = !!inst && !inst.executable && !progress;
  const runningSince = installs.running.get(game.id);
  // Dossier des mods du jeu installé : bouton pour l'ouvrir s'il existe.
  const [modsFolder, setModsFolder] = useState(false);
  const installedExe = inst?.executable ?? null;
  useEffect(() => {
    if (!installedExe) {
      setModsFolder(false);
      return;
    }
    let live = true;
    backend
      .hasModsFolder(game.id)
      .then((found) => live && setModsFolder(found))
      .catch(() => live && setModsFolder(false));
    return () => {
      live = false;
    };
  }, [game.id, installedExe, runningSince]);
  // La recomp installée a son propre launcher : bouton « Ouvrir la recomp ».
  const [recompMenu, setRecompMenu] = useState(false);
  const menuKey = installedExe && !installedWithEmulator ? `${installedExe}|${inst?.archiveName}|${inst?.directLaunch?.exe ?? ""}` : null;
  useEffect(() => {
    if (!menuKey) {
      setRecompMenu(false);
      return;
    }
    let live = true;
    backend
      .recompMenu(game.id)
      .then((m) => live && setRecompMenu(m))
      .catch(() => live && setRecompMenu(false));
    return () => {
      live = false;
    };
  }, [game.id, menuKey]);
  const last = installs.lastSession.get(game.id);
  // Le jeu vient de planter : proposer tout de suite un rapport prérempli.
  useEffect(() => {
    // Hors ligne, le rapport ne partirait pas : proposé au retour du serveur.
    if (!last?.askReport || !inst || offline || runningSince !== undefined || dialog !== null || crashesProposed.has(last)) return;
    crashesProposed.add(last);
    setReportSent(null);
    setReportAfterCrash(true);
    setReportProposed(true);
    setDialog("report");
  }, [last, inst, offline, runningSince, dialog]);

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
    setLaunching(
      choice === "server" ? "Récupération de la sauvegarde du serveur…" : inst?.pc?.proton || inst?.pc?.cod ? "Préparation du jeu…" : "Lancement…",
    );
    setOverlay({ started: false });
    const err = await installs.play(game.id, choice, codChoice.current, openMenu.current);
    setLaunching(null);
    setOverlay((o) => (err ? null : o && { started: true }));
    if (err) setMessage(err);
    else if (choice === "server") setSaveNotice("Sauvegarde du serveur récupérée (l'ancienne est gardée dans .farfalauncher/backups).");
  }

  // Raccourci Steam : le jeu se lance dès qu'il est prêt (installé, ni en cours ni occupé).
  useEffect(() => {
    if (!autoPlay || !inst?.executable || progress || runningSince !== undefined || launching || dialog !== null) return;
    onAutoPlayed?.();
    onPlay();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [autoPlay, inst, progress, runningSince, launching, dialog]);

  /** « Jouer » : un Call of Duty demande d'abord son mode. */
  function onPlay() {
    openMenu.current = false;
    if (inst?.pc?.cod) setDialog("cod");
    else {
      codChoice.current = null;
      startPlay();
    }
  }

  /** Ouvre le launcher de la recomp partagée (réglages, mods, autres jeux). */
  function onOpenMenu() {
    openMenu.current = true;
    codChoice.current = null;
    startPlay();
  }

  /** « Changer pour … » : le mode déjà installé à côté passe en service (« Jouer » revient ensuite). */
  async function switchWay() {
    if (!sel?.way || !sel.id) return;
    setMessage(null);
    setLaunching(`Passage à ${sel.name}…`);
    try {
      await backend.useInstall(base.id, sel.kind, sel.id);
      setChoice({ mode: null, recomp: null, emulator: null });
      await installs.reload();
    } catch (e) {
      setLaunching(null);
      setMessage(await onError(e));
      await installs.reload();
      return;
    }
    setLaunching(null);
  }

  /** « Installer » un autre mode à côté de celui installé (rien n'est supprimé). */
  function installOtherWay() {
    if (!sel) return;
    if (sel.mode === "emulator" && sel.emulator) void installs.start(base.id, undefined, null, true, sel.emulator.id);
    else if (sel.mode === "recomp" && sel.recomp) void installs.start(base.id, undefined, null, false, null, sel.recomp.id);
  }

  /** « Jouer » : compare d'abord la sauvegarde locale à celle du serveur. */
  async function startPlay() {
    setMessage(null);
    setSaveNotice(null);
    setLaunching("Vérification des sauvegardes…");
    try {
      const check = await backend.saveCheck(game.id);
      setLaunching(null);
      if (check.status === "unknown") setSaveAsk({ kind: "where", suggestions: check.suggestions, thenPlay: true });
      else if (check.status === "conflict") setSaveAsk({ kind: "conflict", ...check });
      else await launch("unchanged");
    } catch (e) {
      // Sauvegardes indisponibles (droits, serveur…) : on joue quand même.
      setLaunching(null);
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

  /** Déplace le dossier du jeu ailleurs (autre disque, autre dossier). */
  async function moveGame() {
    if (!inst) return;
    const dest = await open({
      directory: true,
      multiple: false,
      title: `Nouvel emplacement de ${game.name}`,
      defaultPath: parentDir(inst.gameDir),
    });
    if (typeof dest !== "string") return;
    installs.setBusy(game.id, "move");
    const updated = await run(() => backend.moveGame(game.id, dest));
    installs.setBusy(game.id, null);
    if (updated) installs.setInstalled(updated);
  }

  /** Le jeu démarré par son launcher a été relevé : le lancer directement ou non. */
  async function togglePreferLauncher() {
    if (!inst) return;
    const updated = await run(() => backend.setPreferLauncher(game.id, !inst.preferLauncher));
    if (updated) installs.setInstalled(updated);
  }

  async function chooseInstallDir() {
    const dir = await open({
      directory: true,
      multiple: false,
      title: `Où installer ${game.name} ?`,
      defaultPath: installDir ?? installs.installRoot ?? undefined,
    });
    if (typeof dir === "string") setInstallDir(dir);
  }

  async function openExeChoice() {
    const list = inst?.executable ? await run(() => backend.executableCandidates(game.id)) : inst?.pendingCandidates;
    if (list) {
      setCandidates(list);
      setDialog("exe");
    }
  }

  // Mode en service aujourd'hui (il reste installé quand on en choisit un autre).
  const current = currentModeOf(base, inst, sel);
  const panelTitle = current ? modeTitle(current) : inst?.executable ? "Jeu installé" : "Installation";
  const activeName = current && inst?.executable ? current.name : "L'installation actuelle";
  const system = target ? ` (${TARGET_LABELS[target]})` : "";

  return (
    <div className="n-panel gd-actions">
      <div className="n-panel-header gd-actions-title" title={panelTitle}>
        {panelTitle}
      </div>
      <div className="n-panel-body gd-actions-body">
        {progress ? (
          <ProgressView romId={game.id} phase={progress.phase} done={progress.done} total={progress.total} verifying={!!progress.verifying} onCancel={() => backend.cancelInstall(game.id)} />
        ) : switching && sel && runningSince === undefined && !busy ? (
          sel.way ? (
            <>
              <button className="n-btn n-btn--primary n-btn--block" disabled={!!launching} onClick={() => void switchWay()}>
                {launching ? (
                  <>
                    <span className="n-spinner" /> {launching}
                  </>
                ) : (
                  `Changer pour ${sel.name}`
                )}
              </button>
              <p className="n-hint">Déjà installé : rien n'est retéléchargé. {activeName} reste installé.</p>
            </>
          ) : sel.mode === "emulator" && sel.emulator && emulatorReady({ ...base, emulator: sel.emulator }) ? (
            <>
              <button className="n-btn n-btn--primary n-btn--block" disabled={offline} title={offline ? OFFLINE_TITLE : undefined} onClick={installOtherWay}>
                Installer {sel.name}
              </button>
              <p className="n-hint">
                {activeName} reste installé : vous pourrez y revenir. La ROM déjà sur le disque n'est pas retéléchargée. Les émulateurs partagent la même sauvegarde.
              </p>
            </>
          ) : sel.mode === "recomp" && sel.recomp?.available && !base.runtime ? (
            <>
              <button className="n-btn n-btn--primary n-btn--block" disabled={offline} title={offline ? OFFLINE_TITLE : undefined} onClick={installOtherWay}>
                Installer {sel.name}
              </button>
              <p className="n-hint">
                {activeName} reste installé : vous pourrez y revenir. La ROM déjà sur le disque n'est pas retéléchargée. Chaque recomp a ses propres sauvegardes.
              </p>
            </>
          ) : sel.mode === "recomp" && sel.recomp && !sel.recomp.available ? (
            <p className="n-alert" data-sound="none">
              {sel.name} n'est pas disponible pour votre système{system}. {sel.recomp.missingReason ?? availableOn(sel.recomp.systems)}
            </p>
          ) : sel.mode === "emulator" && sel.emulator && !sel.emulator.available ? (
            <p className="n-alert" data-sound="none">
              {sel.name} n'est pas disponible pour votre système{system}. {availableOn(sel.emulator.systems)}
            </p>
          ) : sel.mode === "emulator" && sel.emulator ? (
            <p className="n-notice">
              {sel.name} a besoin d'éléments de configuration absents du serveur : {sel.emulator.configMissing.join(", ")}.
            </p>
          ) : (
            <p className="n-notice">
              Pour jouer avec {sel.name}, désinstallez d'abord {activeName}.
            </p>
          )
        ) : offline && !inst?.executable && !pendingChoice ? (
          <>
            <button className="n-btn n-btn--primary n-btn--block" disabled>
              Installer
            </button>
            <p className="n-hint">Hors ligne : l'installation sera possible au retour du serveur.</p>
          </>
        ) : !isAvailable(game) && !game.detailsLoaded ? (
          <p>
            <span className="n-spinner" /> Lecture des fichiers sur le serveur…
          </p>
        ) : inst?.executable ? (
          <>
            {busy ? (
              <div className="gd-running">
                <span className="n-spinner" /> {busy === "uninstall" ? "Désinstallation en cours…" : "Déplacement en cours…"}
              </div>
            ) : runningSince !== undefined ? (
              <>
                <div className="gd-running">
                  <span className="n-spinner" /> En cours de jeu · <Elapsed since={runningSince} />
                </div>
                <p className="n-hint">Pour quitter ou voir le temps de jeu : maintenez Select + Start sur la manette.</p>
              </>
            ) : (
              <button
                className="n-btn n-btn--primary n-btn--block"
                data-sound="none"
                disabled={!!launching}
                onClick={onPlay}
              >
                {launching ? (
                  <>
                    <span className="n-spinner" /> {launching}
                  </>
                ) : (
                  "Jouer"
                )}
              </button>
            )}
            {(installedWithEmulator ? !!inst.shared : recompMenu) && !busy && runningSince === undefined && !launching && (
              <button
                className="n-btn n-btn--block gd-launcher"
                title={
                  installedWithEmulator
                    ? `Ouvre ${inst.shared?.runtimeName} et ses paramètres`
                    : `Ouvre le launcher de ${inst.shared?.runtimeName ?? sel?.name ?? "la recomp"} (réglages, mods${inst.shared ? ", autres jeux" : ""})`
                }
                onClick={onOpenMenu}
              >
                {installedWithEmulator ? "Ouvrir l'émulateur" : "Ouvrir la recomp"}
              </button>
            )}
            {last && <p className="n-hint">{sessionSummary(last)}</p>}
            {update && runningSince === undefined && !busy && !launching && (
              <button className="n-btn n-btn--block gd-update" disabled={offline} title={offline ? OFFLINE_TITLE : undefined} onClick={() => installs.start(game.id, undefined, null, installedWithEmulator)}>
                Mettre à jour ({inst.versionRaw ? inst.version : "?"} → {installedWithEmulator ? game.emulator?.version : game.serverVersion})
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
        ) : mode === "recomp" && isAvailable(game) && recompOffer?.available !== false ? (
          <>
            <button
              className="n-btn n-btn--primary n-btn--block"
              disabled={recompOffer ? !recompOffer.available : false}
              onClick={() => installs.start(game.id, installDir ?? undefined, game.pc?.proton ? protonChoice : null, false, null, recompId)}
            >
              {recompOffer && !game.pc ? `Installer ${recompOffer.name}` : "Installer"}
            </button>
            {game.pc?.proton && (
              <p className="n-hint">
                Jeu Windows lancé avec Proton : {protonChoice ?? "version recommandée"}{" "}
                <button className="n-link" onClick={() => setDialog("proton")}>
                  Changer…
                </button>
              </p>
            )}
            {game.pc?.kind === "gogInstaller" && !game.pc.proton && (
              <p className="n-hint">L'installateur GOG demandera l'autorisation de Windows.</p>
            )}
            <p className="n-hint gd-install-dest" title={installDir ?? installs.installRoot ?? undefined}>
              {/* Taille connue pour la recomp proposée seulement : une autre a ses propres fichiers. */}
              {recompChoice && recompChoice !== base.recompId ? recompOffer?.version : formatSize(downloadSize(game, runtimeReady))}
              {runtimeInst?.executable
                ? ` · ${game.runtime?.name} est déjà installé`
                : (installDir ?? installs.installRoot) && <Breakable text={` · dans ${installDir ?? installs.installRoot}`} />}
            </p>
            {!runtimeInst?.executable && (
              <div className="gd-inst-actions">
                <button className="n-link" onClick={chooseInstallDir}>
                  Changer l'emplacement…
                </button>
                {installDir && (
                  <button className="n-link" onClick={() => setInstallDir(null)}>
                    Emplacement par défaut
                  </button>
                )}
              </div>
            )}
            {hasAlternative && <p className="n-hint">{CHOICE_HINT}</p>}
          </>
        ) : mode === "emulator" && emulatorReady(game) ? (
          <>
            <button className="n-btn n-btn--primary n-btn--block" onClick={() => installs.start(game.id, installDir ?? undefined, null, true, game.emulator?.id)}>
              {game.emulator ? `Installer ${game.emulator.name}` : "Installer"}
            </button>
            <p className="n-hint gd-install-dest" title={installDir ?? installs.installRoot ?? undefined}>
              {formatSize(emulatorDownloadSize(game, emulatorUpToDate))} · avec {game.emulator?.name}
              {emulatorUpToDate && " (déjà installé)"}
            </p>
            {hasAlternative && <p className="n-hint">{CHOICE_HINT}</p>}
            {!emulatorInst?.executable && (
              <div className="gd-inst-actions">
                <button className="n-link" onClick={chooseInstallDir}>
                  Changer l'emplacement…
                </button>
                {installDir && (
                  <button className="n-link" onClick={() => setInstallDir(null)}>
                    Emplacement par défaut
                  </button>
                )}
              </div>
            )}
          </>
        ) : mode === "recomp" && recompOffer && !recompOffer.available ? (
          <>
            <p className="n-alert" data-sound="none">
              {recompOffer.name} n'est pas disponible pour votre système{target ? ` (${TARGET_LABELS[target]})` : ""}. {recompOffer.missingReason ?? availableOn(recompOffer.systems)}
            </p>
            {hasAlternative && <p className="n-hint">{CHOICE_HINT}</p>}
          </>
        ) : mode === "emulator" && game.emulator?.available ? (
          <p className="n-notice">
            {game.emulator.name} a besoin d'éléments de configuration absents du serveur : {game.emulator.configMissing.join(", ")}.
          </p>
        ) : game.pc || (mode === "recomp" && game.recomp) ? (
          <>
            <p className="n-alert" data-sound="none">
              Non disponible pour votre système{target ? ` (${TARGET_LABELS[target]})` : ""}.{" "}
              {game.pc ? "" : availableOn(recompOffer?.systems ?? game.archives.map((a) => a.target))}
            </p>
            {hasAlternative && <p className="n-hint">{CHOICE_HINT}</p>}
          </>
        ) : (
          <p className="n-notice" data-sound="none">
            {game.emulator ? `Pas d'émulateur disponible pour votre système${target ? ` (${TARGET_LABELS[target]})` : ""}.` : "Pas d'émulateur disponible."}
          </p>
        )}

        {error && <p className="n-alert">{error.message}</p>}
        {message && <p className="n-alert">{message}</p>}
        {saveNotice && <p className="n-notice">{saveNotice}</p>}
        {last?.crashed && !reportSent && runningSince === undefined && inst && (
          <p className="n-notice">
            Le jeu s'est arrêté brusquement.{" "}
            <button
              className="n-link"
              disabled={offline}
              title={offline ? OFFLINE_TITLE : undefined}
              onClick={() => {
                setReportAfterCrash(true);
                setReportProposed(false);
                setDialog("report");
              }}
            >
              Signaler le problème
            </button>
            {last.askReport === false && !crashPromptBack && (
              <>
                {" · "}
                <button
                  className="n-link"
                  onClick={() =>
                    backend
                      .setCrashPrompt(game.id, true)
                      .then(() => setCrashPromptBack(true))
                      .catch(async (e) => setMessage(await onError(e)))
                  }
                >
                  Me reproposer un rapport après un plantage
                </button>
              </>
            )}
          </p>
        )}
        {reportSent && <p className="n-notice">{reportSent}</p>}

        {inst && !switching && !progress && runningSince === undefined && !busy && !launching && (
          <div className="gd-inst-actions">
            <button className="n-link" onClick={() => run(() => backend.openGameFolder(game.id))}>
              Ouvrir le dossier du jeu
            </button>
            {modsFolder && (
              <button className="n-link" onClick={() => run(() => backend.openModsFolder(game.id))}>
                Ouvrir le dossier des mods
              </button>
            )}
            {inst.executable && !inst.pc?.cod && !inst.shared && (
              <button className="n-link" onClick={openExeChoice}>
                Changer l'exécutable
              </button>
            )}
            {inst.pc?.proton && (
              <button className="n-link" onClick={() => setDialog("proton")}>
                Version de Proton…
              </button>
            )}
            {inst.directLaunch && !inst.shared && (
              <button className="n-link" onClick={togglePreferLauncher}>
                {inst.preferLauncher ? "Lancer le jeu sans son launcher" : "Passer par le launcher du jeu"}
              </button>
            )}
            {inst.executable && inst.saves.dir && !inst.saves.off && (
              <button className="n-link" onClick={() => run(() => backend.openSaveFolder(game.id))}>
                Ouvrir les sauvegardes
              </button>
            )}
            {inst.executable && !inst.saves.off && (
              <button className="n-link" disabled={offline} title={offline ? OFFLINE_TITLE : undefined} onClick={() => setDialog("history")}>
                Historique des sauvegardes…
              </button>
            )}
            {inst.executable && !inst.shared && (
              <button className="n-link" onClick={() => setSaveAsk({ kind: "where", suggestions: [], thenPlay: false })}>
                Dossier des sauvegardes…
              </button>
            )}
            <SteamLink romId={game.id} onError={onError} />
            {!installedWithEmulator && (
              <button className="n-link" onClick={moveGame}>
                {inst.shared ? "Déplacer la recomp…" : "Déplacer le jeu…"}
              </button>
            )}
            <button
              className="n-link"
              disabled={offline}
              title={offline ? OFFLINE_TITLE : undefined}
              onClick={() => {
                setReportAfterCrash(false);
                setReportProposed(false);
                setDialog("report");
              }}
            >
              Signaler un problème
            </button>
            <button className="n-link" onClick={() => setDialog("uninstall")}>
              Désinstaller
            </button>
          </div>
        )}
      </div>

      {overlay && (
        <LaunchOverlay
          romId={game.id}
          name={game.name}
          step={launching && launching !== "Lancement…" ? launching : null}
          started={overlay.started}
          onDone={() => setOverlay(null)}
        />
      )}
      {dialog === "proton" && (
        <ProtonDialog
          current={inst?.pc?.proton ? inst.pc.proton.version : protonChoice}
          onClose={() => setDialog(null)}
          onChoose={async (version) => {
            if (inst?.pc?.proton) {
              const updated = await run(() => backend.setProtonVersion(game.id, version));
              if (updated) installs.setInstalled(updated);
            } else setProtonChoice(version);
            setDialog(null);
          }}
        />
      )}
      {dialog === "cod" && inst && (
        <CodModeDialog
          romId={game.id}
          name={game.name}
          onClose={() => setDialog(null)}
          onChoose={(choice) => {
            setDialog(null);
            codChoice.current = choice;
            startPlay();
          }}
        />
      )}
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
          installDir={inst ? exeDir(inst.gameDir, inst.executable) : null}
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
      {dialog === "report" && inst && (
        <ReportDialog
          romId={game.id}
          name={game.name}
          afterCrash={reportAfterCrash}
          crash={reportAfterCrash ? (last?.crash ?? null) : null}
          proposed={reportProposed}
          onMute={() => {
            setCrashPromptBack(false);
            backend.setCrashPrompt(game.id, false).catch(async (e) => setMessage(await onError(e)));
          }}
          onClose={() => setDialog(null)}
          onSent={() => {
            setDialog(null);
            setReportSent("Rapport envoyé, merci ! Suivez sa réponse avec l'icône des messages, en haut de la bibliothèque.");
          }}
        />
      )}
      {dialog === "history" && inst && (
        <SaveHistoryDialog
          romId={game.id}
          name={game.name}
          onClose={() => setDialog(null)}
          onRestored={(v) => {
            setDialog(null);
            setSaveNotice(`Sauvegarde du ${formatIso(v.savedAt)} restaurée. L'ancienne est gardée dans l'historique.`);
          }}
        />
      )}
      {dialog === "uninstall" && inst && (
        <UninstallDialog
          name={game.name}
          runtimeName={inst.shared?.runtimeName ?? null}
          emulator={installedWithEmulator}
          others={(inst.ways ?? [])
            .filter((w) => !w.active)
            .map((w) => (w.kind === "emulator" ? base.emulators?.find((e) => e.id === w.id)?.name : recompList(base).find((r) => r.id === w.id)?.name) ?? w.id)}
          onClose={() => setDialog(null)}
          onConfirm={async (keepSaves) => {
            setDialog(null);
            installs.setBusy(game.id, "uninstall");
            const ok = await run(() => backend.uninstallGame(game.id, keepSaves).then(() => true));
            installs.setBusy(game.id, null);
            if (ok) {
              installs.remove(game.id);
              // Le runtime a pu partir avec son dernier jeu.
              if (inst.shared) await installs.reload();
            }
          }}
        />
      )}
    </div>
  );
}

/** « Conseillé » : une petite étiquette et le message de la fiche, rien de plus. */
function Advice({ text }: { text: string }) {
  return (
    <p className="n-hint gd-advice-line">
      <span className="gd-advice">Conseillé</span> {text}
    </p>
  );
}

/** Remarque de la fiche (`remarque:`) : même place et même style que le conseil, sans l'étiquette. */
function Note({ text }: { text: string }) {
  return <p className="n-hint gd-advice-line">{text}</p>;
}

/** Rappel sous le bouton d'installation : le choix se fait dans l'onglet « Informations ». */
const CHOICE_HINT = "Recomp ou émulation : à choisir dans Informations › Installation.";

/** « Disponible pour : Windows. » */
/** Sans build pour ce système : « Disponible pour : … », sauf si la fiche donne la raison (`missingReason`, à sa place). */
function availableOn(systems: Target[]): string {
  return systems.length ? `Disponible pour : ${systems.map((s) => TARGET_LABELS[s]).join(", ")}.` : "Pas encore de version publiée.";
}

/** Recomps du jeu, disponibles ou non sur ce système (la seule, quand il n'en a qu'une). */
function recompList(game: Game): RecompOffer[] {
  if (!game.recomp || game.pc) return [];
  if (game.recomps?.length) return game.recomps;
  return [
    {
      id: game.recompId ?? "recomp",
      name: game.runtime?.name ?? "Recomp",
      version: game.serverVersion ?? "",
      available: isAvailable(game),
      systems: game.archives.map((a) => a.target),
      advice: null,
    },
  ];
}

/** Façon dont le jeu est installé (`null` : pas installé, ou jeu PC). */
function installedMode(game: Game, inst: InstalledGame | undefined): InstallMode | null {
  if (!inst || game.pc) return null;
  return isEmulatorInstall(inst) ? "emulator" : "recomp";
}

/**
 * Recomp ou émulation : la façon dont le jeu est installé, sinon le choix du joueur, sinon ce
 * qui marche sur ce système (la recomp d'abord).
 */
function installMode(game: Game, inst: InstalledGame | undefined, choice: InstallChoice): InstallMode {
  const recomps = recompList(game).length > 0;
  const emulators = (game.emulators?.length ?? 0) > 0;
  const installed = installedMode(game, inst);
  if (installed) return installed;
  if (choice.mode === "recomp" && recomps) return "recomp";
  if (choice.mode === "emulator" && emulators) return "emulator";
  if (game.pc || isAvailable(game)) return "recomp";
  if (emulatorReady(game)) return "emulator";
  return recomps || !emulators ? "recomp" : "emulator";
}

/** Émulateur effectif (celui installé, sinon celui choisi, sinon le premier proposé). */
function effectiveEmulatorOf(game: Game, inst: InstalledGame | undefined, chosen: string | null): EmulatorOffer | null {
  return withEffectiveEmulator(game, inst, chosen).emulator;
}

/** Ce qui est choisi dans Informations › Installation, et s'il est déjà installé. */
export interface Selection {
  mode: InstallMode;
  recomp: RecompOffer | null;
  emulator: EmulatorOffer | null;
  /** Nom et identifiant de ce qui est choisi (recomp ou émulateur). */
  kind: InstallMode;
  id: string | null;
  name: string;
  /** Installation déjà sur le disque pour ce choix. */
  way: InstallWay | undefined;
}

/** « Jeu en mode recomp : Hookshot », « Jeu en mode émulation : Citron ». */
function modeTitle(m: { mode: InstallMode; name: string }): string {
  return `Jeu en mode ${m.mode === "recomp" ? "recomp" : "émulation"} : ${m.name}`;
}

/**
 * Mode dans lequel on est : celui en service pour un jeu installé (il ne change qu'en changeant
 * vraiment de mode), sinon ce qui est choisi pour l'installation.
 */
function currentModeOf(base: Game, inst: InstalledGame | undefined, sel: Selection | null): { mode: InstallMode; name: string } | null {
  if (!sel) return null;
  const active = inst?.executable ? inst.ways?.find((w) => w.active) : undefined;
  if (!active) return inst?.executable ? null : { mode: sel.mode, name: sel.name };
  const name = active.kind === "recomp" ? recompList(base).find((r) => r.id === active.id)?.name : base.emulators?.find((e) => e.id === active.id)?.name;
  return { mode: active.kind, name: name ?? active.id };
}

/** Mode dans lequel on est (en-tête de la page du jeu). */
export function useCurrentMode(base: Game, installs: InstallsState): { mode: InstallMode; name: string } | null {
  const [choice] = useInstallChoice(base.id);
  const inst = installs.installed.get(base.id);
  return currentModeOf(base, inst, selectionOf(base, inst, choice));
}

/** Recomp ou émulateur choisi sur la page du jeu (en-tête, panneau, Nouveautés), installé ou non. */
export function useSelection(base: Game, installs: InstallsState): Selection | null {
  const [choice] = useInstallChoice(base.id);
  return selectionOf(base, installs.installed.get(base.id), choice);
}

/** Recomp ou émulateur choisi : le choix du joueur, sinon ce qui est installé, sinon le meilleur. */
function selectionOf(base: Game, inst: InstalledGame | undefined, choice: InstallChoice): Selection | null {
  const recomps = recompList(base);
  const emulators = base.emulators ?? [];
  if (base.pc || recomps.length + emulators.length === 0) return null;
  // Ce qui est choisi ici, pas encore appliqué, passe avant ce qui est installé.
  const chosenOk = choice.mode && (choice.mode === "recomp" ? recomps.length > 0 : emulators.length > 0);
  const mode: InstallMode = chosenOk ? choice.mode! : installMode(base, inst, choice);
  const recompId = choice.recomp ?? base.recompId ?? recomps[0]?.id ?? null;
  const recomp = recomps.find((r) => r.id === recompId) ?? recomps[0] ?? null;
  const emulator =
    isEmulatorInstall(inst) && choice.emulator ? (emulators.find((e) => e.id === choice.emulator) ?? null) : effectiveEmulatorOf(base, inst, choice.emulator);
  const picked = mode === "recomp" ? recomp : emulator;
  const ways = inst?.executable ? (inst.ways ?? []) : [];
  return {
    mode,
    recomp,
    emulator,
    kind: mode,
    id: picked?.id ?? null,
    name: picked?.name ?? "",
    way: ways.find((w) => w.kind === mode && w.id === picked?.id),
  };
}

/**
 * Onglet « Informations », section Installation : recomp ou émulation, puis toutes les recomps
 * ou tous les émulateurs du launcher pour ce jeu, y compris ceux sans version pour ce système.
 * Jeu installé : ce qui est déjà sur le disque se met en service d'un clic, le reste s'installe
 * à côté.
 */
function InstallChoiceFacts({ game: base, target, installs }: { game: Game; target: Target | null; installs: InstallsState }) {
  const inst = installs.installed.get(base.id);
  const [choice, setChoice] = useInstallChoice(base.id);
  const sel = selectionOf(base, inst, choice);
  if (!sel) return null;
  const recomps = recompList(base);
  const emulators = base.emulators ?? [];
  const { mode, recomp, emulator: current } = sel;
  const system = target ? TARGET_LABELS[target] : "ce système";
  const ways = inst?.executable ? (inst.ways ?? []) : [];
  const owned = (kind: InstallMode, id: string | undefined) => ways.some((w) => w.kind === kind && w.id === id);
  // Installer ou jouer avec ce qui est choisi : le bouton du panneau d'installation s'en charge.

  return (
    <>
      {recomps.length > 0 && emulators.length > 0 && (
        <>
          <dt>Mode</dt>
          <dd>
            <span className="gd-lang" role="group" aria-label="Recomp ou émulation">
              {(
                [
                  ["recomp", "Recomp"],
                  ["emulator", "Émulation"],
                ] as const
              ).map(([m, label]) => (
                <button key={m} className="n-btn" aria-pressed={mode === m} onClick={() => mode !== m && setChoice({ mode: m })}>
                  {label}
                </button>
              ))}
            </span>
          </dd>
        </>
      )}
      <dt>{mode === "recomp" ? (recomps.length > 1 ? "Recomps" : "Recomp") : emulators.length > 1 ? "Émulateurs" : "Émulateur"}</dt>
      <dd>
        <div className="gd-emulators-list" role="radiogroup" aria-label={mode === "recomp" ? "Recomp" : "Émulateur"}>
          {mode === "recomp"
            ? recomps.map((r) => {
                const on = r.id === recomp?.id;
                return (
                  <button
                    key={r.id}
                    role="radio"
                    aria-checked={on}
                    className={`n-btn gd-emulator${on ? " is-on" : ""}`}
                    title={r.available ? `${r.name} ${r.version}` : (r.missingReason ?? `Pas de version pour ${system}`)}
                    onClick={() => !on && setChoice({ mode: "recomp", recomp: r.id })}
                  >
                    {r.name}
                    {r.advice && <span className="gd-advice">Conseillé</span>}
                    {!r.available && <span className="gd-emulator-off"> (indisponible)</span>}
                    {owned("recomp", r.id) && <span className="gd-emulator-off"> (installée)</span>}
                  </button>
                );
              })
            : emulators.map((e) => {
                const on = e.id === current?.id;
                const usable = e.available && e.configMissing.length === 0;
                return (
                  <button
                    key={e.id}
                    role="radio"
                    aria-checked={on}
                    className={`n-btn gd-emulator${on ? " is-on" : ""}`}
                    title={usable ? `${e.name} ${e.version}` : `Pas de version pour ${system}`}
                    onClick={() => !on && setChoice({ mode: "emulator", emulator: e.id })}
                  >
                    {e.name}
                    {e.advice && <span className="gd-advice">Conseillé</span>}
                    {!usable && <span className="gd-emulator-off"> (indisponible)</span>}
                    {owned("emulator", e.id) && <span className="gd-emulator-off"> (installé)</span>}
                  </button>
                );
              })}
        </div>
        {mode === "recomp" && recomp && !recomp.available && (
          <p className="n-alert" data-sound="none">
            {recomp.name} n'est pas disponible pour {system}. {recomp.missingReason ?? availableOn(recomp.systems)}
          </p>
        )}
        {mode === "emulator" && current && !current.available && (
          <p className="n-alert" data-sound="none">
            {current.name} n'est pas disponible pour {system}. {availableOn(current.systems)}
          </p>
        )}
        {mode === "emulator" && current?.available && current.configMissing.length > 0 && (
          <p className="n-notice">
            {current.name} a besoin d'éléments de configuration absents du serveur : {current.configMissing.join(", ")}.
          </p>
        )}
        {mode === "recomp" && recomp?.advice && <Advice text={recomp.advice} />}
        {mode === "recomp" && recomp?.note && <Note text={recomp.note} />}
        {mode === "emulator" && current?.advice && <Advice text={current.advice} />}
        {mode === "emulator" && current?.note && <Note text={current.note} />}
      </dd>
      <ChangelogFact romId={base.id} console={base.console} kind={mode} id={sel.id} />
    </>
  );
}

/** Ligne « Mods » : le dossier où mettre ses mods, d'après la documentation de la recomp ou de l'émulateur. */
function ModsFact({ romId, installKey }: { romId: number; installKey: string }) {
  const [dir, setDir] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  useEffect(() => {
    let live = true;
    backend
      .modsFolder(romId)
      .then((d) => live && setDir(d))
      .catch(() => live && setDir(null));
    return () => {
      live = false;
    };
  }, [romId, installKey]);
  if (!dir) return null;
  return (
    <>
      <dt>Mods</dt>
      <dd className="gd-savedir" title={dir}>
        <button className="n-link" onClick={() => backend.openModsFolder(romId).then(() => setError(null), (e) => setError(errorMessage(e)))}>
          Ouvrir le dossier des mods
        </button>{" "}
        <span className="n-hint">
          <Breakable text={shortPath(dir)} />
        </span>
        {error && <p className="n-alert">{error}</p>}
      </dd>
    </>
  );
}

const PC_KIND_LABELS: Record<PcKind, string> = {
  portable: "jeu prêt à lancer",
  archive: "archive",
  gogInstaller: "installateur GOG",
  cod: "jeu + client communautaire",
};

/** Black Ops III : langue du jeu (boiii ne parle qu'anglais au jeu, le launcher renomme ses zones). */
function Bo3LanguageFact({ romId, playing }: { romId: number; playing: boolean }) {
  return <LanguageFact romId={romId} playing={playing} load={backend.bo3Language} save={backend.setBo3Language} />;
}

/** Recomp qui propose l'anglais (`langue:` avec une valeur en anglais, OpenSmash) : posé au prochain lancement. */
function GameLanguageFact({ romId, playing }: { romId: number; playing: boolean }) {
  const [offered, setOffered] = useState(false);
  useEffect(() => {
    let live = true;
    backend
      .gameLanguage(romId)
      .then((l) => live && setOffered(l !== null))
      .catch(() => {});
    return () => {
      live = false;
    };
  }, [romId]);
  if (!offered) return null;
  return <LanguageFact romId={romId} playing={playing} load={loadGameLanguage} save={backend.setGameLanguage} />;
}

const loadGameLanguage = (romId: number) => backend.gameLanguage(romId).then((l): Bo3Language => l ?? "fr");

/** Choix Français / English d'un jeu installé. */
function LanguageFact({
  romId,
  playing,
  load,
  save,
}: {
  romId: number;
  playing: boolean;
  load: (romId: number) => Promise<Bo3Language>;
  save: (romId: number, lang: Bo3Language) => Promise<Bo3Language>;
}) {
  const [lang, setLang] = useState<Bo3Language | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  useEffect(() => {
    let live = true;
    load(romId)
      .then((l) => live && setLang(l))
      .catch(() => {});
    return () => {
      live = false;
    };
  }, [romId, load]);
  const choose = (l: Bo3Language) => {
    if (l === lang || busy) return;
    setBusy(true);
    setError(null);
    save(romId, l)
      .then(setLang)
      .catch((e) => setError(errorMessage(e)))
      .finally(() => setBusy(false));
  };
  return (
    <>
      <dt>Langue</dt>
      <dd>
        <span className="gd-lang" role="group" aria-label="Langue du jeu">
          {(
            [
              ["fr", "Français"],
              ["en", "English"],
            ] as const
          ).map(([code, label]) => (
            <button key={code} className="n-btn" aria-pressed={lang === code} disabled={busy || playing || lang === null} onClick={() => choose(code)}>
              {label}
            </button>
          ))}
        </span>
        {error && <p className="n-alert">{error}</p>}
      </dd>
    </>
  );
}

/** Détails de l'installation, affichés dans l'onglet « Informations ». */
export function InstallFacts({ game: base, target, installs }: { game: Game; target: Target | null; installs: InstallsState }) {
  const inst = installs.installed.get(base.id);
  const [choice] = useInstallChoice(base.id);
  // Recomp ou émulateur choisi : les lignes ci-dessous parlent de lui, installé ou non (version,
  // téléchargement, sources, plateformes, tout vient du catalogue de FarfaRomManager).
  const sel = selectionOf(base, inst, choice);
  const chosen = withEffectiveEmulator(base, inst, choice.emulator);
  const game = sel?.emulator && sel.mode === "emulator" ? { ...chosen, emulator: sel.emulator } : chosen;
  const runtimeInstalled = !!game.runtime && !!installs.installed.get(game.runtime.romId)?.executable;
  const withEmulator = sel ? sel.mode === "emulator" : false;
  const choosing = !!sel;
  // Ce qui est choisi n'est pas ce que « Jouer » lance : on montre ce qu'on sait de lui.
  const preview = !!sel && !(inst && (!inst.executable || sel.way?.active || !(inst.ways?.length)));
  const emulatorSystems = withEmulator ? (game.emulator?.systems ?? []) : [];
  const recompSystems = !withEmulator && sel?.recomp && sel.recomp.id !== base.recompId ? sel.recomp.systems : null;
  const [shaders, setShaders] = useState<Record<string, SharedShaders>>({});
  const playing = installs.running.has(base.id);
  useEffect(() => {
    if (!withEmulator) {
      setShaders({});
      return;
    }
    if (playing) return;
    let live = true;
    const load = () =>
      backend
        .shaderInfo(base.id)
        .then((s) => live && setShaders(s))
        .catch(() => {});
    load();
    // Partie qui vient de finir : les shaders partent sur le serveur juste après.
    const again = [15000, 60000].map((ms) => window.setTimeout(load, ms));
    return () => {
      live = false;
      again.forEach((t) => window.clearTimeout(t));
    };
  }, [base.id, withEmulator, playing]);
  const shaderLines = Object.entries(shaders)
    .filter(([emu]) => !game.emulator || emu === game.emulator.id)
    .map(([emu, s]) => {
    const name = game.emulators?.find((e) => e.id === emu)?.name ?? emu;
    const last = s.sessions[s.sessions.length - 1]?.added ?? 0;
    const recent = s.sessions.slice(-3);
    // Plus rien de neuf aux dernières parties : il en manque sans doute peu.
    const almost = recent.length >= 3 && recent.reduce((n, x) => n + x.added, 0) <= Math.max(1, s.count / 100);
    const players = s.players.length > 1 ? ` · ${s.players.length} joueurs` : "";
    return `${name} : ${s.count.toLocaleString("fr-FR")}${players} · +${last} à la dernière partie${almost ? " (presque complet)" : ""}`;
  });
  // Sources : le dépôt de ce qui fait (ou fera) tourner le jeu, et sa page sur RomM.
  const [rommUrl, setRommUrl] = useState<string | null>(null);
  useEffect(() => {
    let live = true;
    backend
      .sessionStatus()
      .then((s) => live && setRommUrl(s.serverUrl))
      .catch(() => {});
    return () => {
      live = false;
    };
  }, []);
  const recompRepo = sel?.recomp?.repo ?? game.runtime?.repo;
  const source = withEmulator
    ? game.emulator?.repo
      ? { name: game.emulator.name, url: game.emulator.repo }
      : null
    : recompRepo
      ? { name: sel?.recomp?.name ?? game.runtime?.name ?? "la recomp", url: recompRepo }
      : null;
  const emulatorHere = installs.installed.get(game.emulator?.runtimeRomId ?? -1)?.versionRaw === game.emulator?.versionKey;
  const romBytes = game.romFiles.reduce((n, f) => n + f.size, 0);
  const rommPage = rommUrl ? `${rommUrl.replace(/\/+$/, "")}/rom/${base.id}` : null;
  return (
    <>
      <InstallChoiceFacts game={base} target={target} installs={installs} />
      {!choosing && game.runtime && !withEmulator && (
        <>
          <dt>Recomp</dt>
          <dd>
            <button className="n-link" title={game.runtime.repo} onClick={() => backend.openUrl(game.runtime!.repo)}>
              {game.runtime.name}
            </button>
          </dd>
        </>
      )}
      {!choosing && withEmulator && game.emulator && (
        <>
          <dt>Émulateur</dt>
          <dd>
            {game.emulator.repo ? (
              <button className="n-link" title={game.emulator.repo} onClick={() => backend.openUrl(game.emulator!.repo!)}>
                {game.emulator.name}
              </button>
            ) : (
              game.emulator.name
            )}
          </dd>
        </>
      )}
      <dt>Version serveur</dt>
      <dd>{(withEmulator ? game.emulator?.version : (preview && sel?.recomp?.version) || game.serverVersion) ?? "inconnue"}</dd>
      <dt>Version installée</dt>
      <dd>{preview ? (sel?.way?.version ?? (sel?.way ? "inconnue" : "non installé")) : inst?.executable ? (inst.version ?? "inconnue") : "non installé"}</dd>
      {preview && sel?.way && (
        <>
          <dt>Emplacement</dt>
          <dd className="gd-savedir" title={sel.way.gameDir}>
            <Breakable text={shortPath(sel.way.gameDir)} />
          </dd>
        </>
      )}
      {preview && !sel?.way && game.detailsLoaded && (withEmulator ? emulatorReady(game) : !!sel?.recomp?.available) && (
        <>
          <dt>Téléchargement</dt>
          <dd>
            {formatSize(
              withEmulator
                ? emulatorDownloadSize(game, emulatorHere)
                : sel?.recomp?.id === base.recompId
                  ? downloadSize(game, runtimeInstalled && !hasUpdate(game, installs.installed.get(game.runtime?.romId ?? -1)))
                  : (sel?.recomp?.size ?? 0) + romBytes,
            )}
            {inst?.executable && " (la ROM déjà sur le disque n'est pas retéléchargée)"}
          </dd>
        </>
      )}
      {!preview && inst && (
        <>
          <dt>Emplacement</dt>
          <dd className="gd-savedir" title={inst.gameDir}>
            <Breakable text={shortPath(inst.gameDir)} />
          </dd>
        </>
      )}
      {!preview && inst?.executable && (
        <>
          <dt>Exécutable</dt>
          <dd>
            <Breakable text={inst.executable} />
          </dd>
          {inst.directLaunch && (
            <>
              <dt>Lancement</dt>
              <dd title={inst.directLaunch.exe}>
                {inst.preferLauncher ? "par le launcher" : <Breakable text={`direct (${inst.directLaunch.exe.split("/").pop()})`} />}
              </dd>
            </>
          )}
          <dt>Sauvegardes</dt>
          <dd className="gd-savedir" title={inst.saves.dir ?? undefined}>
            {inst.saves.off ? "non synchronisées" : inst.saves.dir ? <Breakable text={shortPath(inst.saves.dir)} /> : "dossier inconnu (demandé au lancement)"}
          </dd>
        </>
      )}
      {game.pc && (
        <>
          <dt>Type</dt>
          <dd>{PC_KIND_LABELS[game.pc.kind]}</dd>
        </>
      )}
      {inst?.pc?.proton && (
        <>
          <dt>Proton</dt>
          <dd>{inst.pc.proton.version ?? "version recommandée"}</dd>
        </>
      )}
      {inst?.executable && inst.pc?.cod === "bo3" && <Bo3LanguageFact romId={base.id} playing={playing} />}
      {inst?.executable && !inst.pc && <GameLanguageFact romId={base.id} playing={playing} />}
      {!choosing && !inst && isAvailable(game) && (
        <>
          <dt>Téléchargement</dt>
          <dd>{formatSize(downloadSize(game, runtimeInstalled && !hasUpdate(game, installs.installed.get(game.runtime?.romId ?? -1))))}</dd>
        </>
      )}
      {!choosing && !inst && !isAvailable(game) && emulatorReady(game) && game.detailsLoaded && (
        <>
          <dt>Téléchargement</dt>
          <dd>{formatSize(emulatorDownloadSize(game, installs.installed.get(game.emulator?.runtimeRomId ?? -1)?.versionRaw === game.emulator?.versionKey))}</dd>
        </>
      )}
      {shaderLines.length > 0 && (
        <>
          <dt>Shaders partagés</dt>
          <dd title="Shaders déjà vus par les joueurs, réunis sur le serveur : reçus avant la partie, ils évitent les saccades.">
            {shaderLines.map((l) => (
              <div key={l}>{l}</div>
            ))}
          </dd>
        </>
      )}
      {!preview && inst?.executable && <ModsFact romId={base.id} installKey={`${inst.executable}|${inst.archiveName}|${inst.gameDir}`} />}
      {!preview && inst?.executable && <OptimizedFacts romId={base.id} running={installs.running.has(base.id)} />}
      {(source || rommPage) && (
        <>
          <dt>Sources</dt>
          <dd className="gd-sources">
            {source && (
              <button className="n-link" title={source.url} onClick={() => backend.openUrl(source.url)}>
                {repoLabel(source.url)} de {source.name}
              </button>
            )}
            {source && rommPage && " · "}
            {rommPage && (
              <button className="n-link" title={rommPage} onClick={() => backend.openUrl(rommPage)}>
                Page RomM
              </button>
            )}
          </dd>
        </>
      )}
      <dt>Plateformes</dt>
      <dd>
        <ul className="gd-platforms">
          {game.pc && (
            <li className={game.pc.runnable ? "is-mine" : undefined}>
              {game.pc.os === "linux" ? "Linux" : "Windows"}
              {game.pc.proton && " (via Proton)"}
            </li>
          )}
          {!game.pc && (recompSystems ?? (withEmulator ? [] : game.archives)).length === 0 && emulatorSystems.length === 0 && <li className="n-hint">aucune</li>}
          {recompSystems?.map((s) => (
            <li key={s} className={sel?.recomp?.available && s === target ? "is-mine" : undefined}>
              {TARGET_LABELS[s]}
            </li>
          ))}
          {emulatorSystems.map((s) => (
            <li key={s} className={game.emulator?.available && s === target ? "is-mine" : undefined}>
              {TARGET_LABELS[s]}
            </li>
          ))}
          {!withEmulator && !recompSystems && game.archives.map((a) => (
            <li key={a.target} className={a.target === game.myArchive?.target ? "is-mine" : undefined}>
              {TARGET_LABELS[a.target]}
            </li>
          ))}
        </ul>
      </dd>
    </>
  );
}

/** Taille toujours avec une décimale (« 1,0 Go », « 12,3 Mo ») : sa largeur ne varie pas d'un instant à l'autre. */
function fixedSize(bytes: number): string {
  const units = ["o", "Ko", "Mo", "Go", "To"];
  let v = Math.max(0, bytes);
  let i = 0;
  while (v >= 1000 && i < units.length - 1) {
    v /= 1024;
    i++;
  }
  return `${v.toLocaleString("fr-FR", { minimumFractionDigits: i === 0 ? 0 : 1, maximumFractionDigits: i === 0 ? 0 : 1 })} ${units[i]}`;
}

/** « GitHub » pour un dépôt GitHub, sinon « Code source » (forge Forgejo de Longshot…). */
function repoLabel(url: string): string {
  try {
    return new URL(url).hostname.replace(/^www\./, "") === "github.com" ? "GitHub" : "Code source";
  } catch {
    return "Code source";
  }
}

/** Phases dont `done`/`total` sont des octets. */
const BYTE_PHASES: InstallPhase[] = ["download", "proton", "client", "extract", "decrypt", "data"];

/** Explication affichée sous la barre : ce qui se passe, et combien de temps ça prend. */
const PHASE_HINTS: Partial<Record<InstallPhase, string>> = {
  extract: "Les fichiers du jeu sont décompressés dans son dossier.",
  decrypt: "Le jeu est déchiffré pour l'émulateur : une minute environ par Go.",
  data: "Les données tirées de votre copie du jeu sont posées à côté de lui : des dizaines de milliers de fichiers, plusieurs minutes pour un gros jeu.",
  build: "Le jeu est construit sur ce PC à partir de votre copie : 10 à 45 minutes selon le processeur.",
  check: "Le launcher vérifie que c'est bien la version du jeu attendue par la recomp.",
  copy: "Les fichiers du jeu sont mis en place pour la recomp.",
  generate: "Le code du jeu est traduit en code pour PC : quelques minutes.",
  compile: "Le code traduit est compilé sur votre PC : le processeur tourne à fond, c'est normal. Comptez de 10 à 30 minutes selon le processeur.",
  shaders: "Les shaders du jeu sont préparés pour votre carte graphique, pour qu'il ne saccade pas en jouant. La barre peut rester immobile quelques minutes au début.",
  warm: "Le jeu se lance une minute tout seul pour préparer votre carte graphique. Une fenêtre peut s'ouvrir : ne la fermez pas, elle se fermera d'elle-même.",
  cleanup: "Les fichiers temporaires sont supprimés, presque fini.",
  cache: "Les données copiées depuis votre disque sont converties une fois pour le jeu : quelques minutes.",
  place: "Les fichiers extraits sont rangés dans le dossier du jeu. Ça peut prendre une minute pour un gros jeu.",
  finalize: "Derniers réglages, presque fini.",
};

/** « 2 min 30 s », « 45 s », « 1 h 05 ». */
function formatRemaining(s: number): string {
  if (s < 60) return `${Math.max(1, Math.round(s))} s`;
  if (s < 3600) {
    const m = Math.floor(s / 60);
    const sec = Math.round(s % 60);
    return m < 10 && sec ? `${m} min ${sec} s` : `${Math.round(s / 60)} min`;
  }
  const h = Math.floor(s / 3600);
  return `${h} h ${String(Math.round((s % 3600) / 60)).padStart(2, "0")}`;
}

/** Débit lissé (unités de `done` par seconde) de la phase en cours, et depuis quand elle dure (ms). */
function useRate(phase: InstallPhase, done: number): { rate: number | null; elapsed: number } {
  const ref = useRef<{ phase: InstallPhase; start: number; t: number; done: number; rate: number | null }>({ phase, start: 0, t: 0, done, rate: null });
  const s = ref.current;
  const now = performance.now();
  if (s.phase !== phase || done < s.done) {
    ref.current = { phase, start: now, t: now, done, rate: null };
  } else if (now - s.t >= 1000) {
    const inst = ((done - s.done) * 1000) / (now - s.t);
    s.rate = s.rate === null ? inst : s.rate * 0.7 + inst * 0.3;
    s.t = now;
    s.done = done;
  }
  return { rate: ref.current.rate, elapsed: now - ref.current.start };
}

/** Réglages optimisés pour ce PC : ce que le launcher pose au lancement, et l'interrupteur. */
function OptimizedFacts({ romId, running }: { romId: number; running: boolean }) {
  const [info, setInfo] = useState<Optimizations | null>(null);
  useEffect(() => {
    let live = true;
    backend
      .optimizations(romId)
      .then((o) => live && setInfo(o))
      .catch(() => {});
    return () => {
      live = false;
    };
  }, [romId]);
  if (!info || info.applied.length === 0) return null;
  async function toggle(enabled: boolean) {
    setInfo((i) => (i ? { ...i, enabled } : i));
    await backend.setOptimized(romId, enabled).catch(() => setInfo((i) => (i ? { ...i, enabled: !enabled } : i)));
  }
  return (
    <>
      <dt>Réglages optimisés</dt>
      <dd>
        <label className="n-check">
          <input type="checkbox" checked={info.enabled} disabled={running} onChange={(e) => toggle(e.target.checked)} />
          Adaptés à ce PC (niveau {LEVEL_NAMES[info.level]})
        </label>
        {info.enabled ? (
          <ul className="gd-optimized">
            {info.applied.map((a) => (
              <li key={a.key}>{a.label}</li>
            ))}
          </ul>
        ) : (
          <p className="n-hint">Coupés : vos propres réglages de l'émulateur sont gardés.</p>
        )}
      </dd>
    </>
  );
}

const LEVEL_NAMES: Record<Optimizations["level"], string> = { faible: "modeste", moyen: "moyen", fort: "puissant" };

/** Place dans la file, en clair : « 1er », « 2e »… */
export function queuePlace(n: number): string {
  return n === 1 ? "1er" : `${n}e`;
}

type ProgressProps = {
  romId: number;
  phase: InstallPhase;
  done: number;
  total: number;
  verifying: boolean;
  onCancel: () => void;
};

/** Progression d'une installation, ou sa place dans la file de téléchargements. */
function ProgressView(props: ProgressProps) {
  const { romId, phase, done, total, onCancel } = props;
  if (!isWaiting(phase)) return <ActiveProgress {...props} />;
  return (
    <div className="gd-progress">
      <div className="gd-progress-label">{PHASE_LABELS[phase]}</div>
      <p className="n-hint">
        {phase === "paused"
          ? "Ce qui est déjà téléchargé est gardé : l'installation reprendra là où elle s'est arrêtée."
          : `${queuePlace(done)} sur ${total} dans la file : l'installation commencera après les précédentes.`}
      </p>
      <div className="gd-inst-actions">
        {phase === "paused" ? (
          <button className="n-link" onClick={() => backend.resumeInstall(romId)}>
            Reprendre
          </button>
        ) : (
          <>
            {done > 1 && (
              <button className="n-link" onClick={() => backend.moveInstall(romId, true)}>
                Passer devant
              </button>
            )}
            <button className="n-link" onClick={() => backend.pauseInstall(romId)}>
              Mettre en pause
            </button>
          </>
        )}
        <button className="n-link" onClick={onCancel}>
          Annuler
        </button>
      </div>
    </div>
  );
}

function ActiveProgress({ romId, phase, done, total, verifying, onCancel }: ProgressProps) {
  const installing = phase === "installer";
  // Installateur : estimation d'après la place prise, jamais 100 % avant la fin.
  const pct = total > 0 ? Math.min(installing ? 99 : 100, (done / total) * 100) : null;
  const bytes = BYTE_PHASES.includes(phase);
  const { rate, elapsed } = useRate(phase, done);
  // Pas d'estimation sur trop peu de mesures (début d'étape : « 9 h » qui retombe à 20 min).
  const settled = elapsed >= 20000 && total > 0 && done / total >= 0.01;
  const remaining = settled && rate && rate > 0 && total > done && !verifying ? (total - done) / rate : null;
  // Chiffres sur leur propre ligne, en colonnes de largeur fixe : ils changent sans faire sauter le texte.
  const showRate = bytes && !installing && pct !== null && !verifying && done < total;
  const hint = PHASE_HINTS[phase];
  return (
    <div className="gd-progress">
      <div className="gd-progress-label">
        {PHASE_LABELS[phase]}
        {verifying && <span className="n-hint"> · vérification du fichier reçu</span>}
      </div>
      {(installing ? done > 0 : pct !== null) && (
        <div className="gd-progress-stats">
          {installing ? (
            <span className="gd-stat-size">{fixedSize(done)} installés</span>
          ) : (
            <>
              <span className="gd-stat-pct">{(pct ?? 0).toLocaleString("fr-FR", { minimumFractionDigits: 1, maximumFractionDigits: 1 })} %</span>
              {bytes && (
                <span className="gd-stat-size">
                  {fixedSize(done)} / {fixedSize(total)}
                </span>
              )}
              {showRate && <span className="gd-stat-rate">{rate && rate > 0 ? `${fixedSize(rate)}/s` : "…"}</span>}
            </>
          )}
        </div>
      )}
      <div className="gd-progress-bar" role="progressbar" aria-valuenow={pct ?? undefined} aria-valuemin={0} aria-valuemax={100}>
        <div className={pct === null ? "is-indeterminate" : "gd-progress-fill"} style={pct === null ? undefined : { transform: `scaleX(${Math.min(100, Math.max(0, pct)) / 100})` }} />
      </div>
      {remaining !== null && remaining >= 5 && <p className="n-hint">Encore environ {formatRemaining(remaining)}.</p>}
      {installing && <p className="n-hint">Installation en cours, elle ne peut plus être annulée.</p>}
      {hint && <p className="n-hint">{hint}</p>}
      {phase !== "finalize" && phase !== "data" && phase !== "place" && !installing && (
        <div className="gd-inst-actions">
          {PAUSABLE.includes(phase) && (
            <button className="n-link" onClick={() => backend.pauseInstall(romId)}>
              Mettre en pause
            </button>
          )}
          <button className="n-link" onClick={onCancel}>
            Annuler
          </button>
        </div>
      )}
    </div>
  );
}

/** Étapes où une pause ne perd rien : les téléchargements (repris grâce aux fichiers `.part`). */
export const PAUSABLE: InstallPhase[] = ["prepare", "download", "proton", "client"];

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

function ProtonDialog({
  current,
  onClose,
  onChoose,
}: {
  current: string | null;
  onClose: () => void;
  onChoose: (version: string | null) => Promise<void>;
}) {
  const [versions, setVersions] = useState<ProtonVersion[] | null>(null);
  const [choice, setChoice] = useState<string | null>(current);
  const [busy, setBusy] = useState(false);
  useEffect(() => {
    backend
      .protonVersions()
      .then(setVersions)
      .catch(() => setVersions([]));
  }, []);
  const where = (v: ProtonVersion) =>
    v.source === "remote" ? `à télécharger${v.size ? ` (${formatSize(v.size)})` : ""}` : v.source === "steam" ? "installée par Steam" : "installée";
  return (
    <Dialog
      title="Version de Proton"
      onClose={onClose}
      actions={
        <>
          <button className="n-btn" onClick={onClose}>
            Annuler
          </button>
          <button
            className="n-btn"
            disabled={busy}
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
      <p>Proton fait tourner les jeux Windows sous Linux. Changez de version si le jeu ne démarre pas ou fonctionne mal.</p>
      <ul className="choice-list">
        <li>
          <label>
            <input type="radio" name="proton" checked={choice === null} onChange={() => setChoice(null)} />
            Recommandée (la plus récente de GE-Proton)
          </label>
        </li>
        {versions === null && (
          <li className="n-hint">
            <span className="n-spinner" /> Recherche des versions…
          </li>
        )}
        {versions?.map((v) => (
          <li key={v.name}>
            <label>
              <input type="radio" name="proton" checked={choice === v.name} onChange={() => setChoice(v.name)} />
              {v.name} · {where(v)}
              {v.recommended && " · recommandée"}
            </label>
          </li>
        ))}
      </ul>
    </Dialog>
  );
}

function CodModeDialog({
  romId,
  name,
  onClose,
  onChoose,
}: {
  romId: number;
  name: string;
  onClose: () => void;
  onChoose: (choice: CodChoice) => void;
}) {
  const [modes, setModes] = useState<CodModeInfo[] | null>(null);
  useEffect(() => {
    backend
      .codModes(romId)
      .then((list) => {
        // Un seul lancement possible (Black Ops III : le mode se choisit dans le jeu) : pas de question.
        if (list.length === 1 && !list[0].plutonium) onChoose({ mode: list[0].mode, online: false });
        else setModes(list);
      })
      .catch(() => setModes([]));
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [romId]);
  return (
    <Dialog
      title={name}
      onClose={onClose}
      actions={
        <button className="n-btn" onClick={onClose}>
          Annuler
        </button>
      }
    >
      <p>Quel mode lancer ?</p>
      {modes === null ? (
        <p className="n-hint">
          <span className="n-spinner" />
        </p>
      ) : (
        <ul className="cod-modes">
          {modes.map((m) => (
            <li key={m.mode}>
              <span className="cod-mode-name">{m.label}</span>
              {m.plutonium ? (
                <>
                  <button className="n-btn" onClick={() => onChoose({ mode: m.mode, online: true })}>
                    En ligne
                  </button>
                  <button className="n-btn" onClick={() => onChoose({ mode: m.mode, online: false })}>
                    Hors ligne (LAN)
                  </button>
                </>
              ) : (
                <button className="n-btn" autoFocus={m.mode === "sp"} onClick={() => onChoose({ mode: m.mode, online: false })}>
                  Jouer
                </button>
              )}
            </li>
          ))}
        </ul>
      )}
      {modes?.some((m) => m.plutonium) && (
        <p className="n-hint">
          En ligne : Plutonium demande de se connecter à son compte la première fois (sa fenêtre s'ouvre). Hors ligne : sans
          compte, en solo ou en réseau local, avec des bots.
        </p>
      )}
    </Dialog>
  );
}

/** Rapport de bug : description écrite par le joueur, et liste des fichiers joints. */
function ReportDialog({
  romId,
  name,
  afterCrash,
  crash,
  proposed,
  onMute,
  onClose,
  onSent,
}: {
  romId: number;
  name: string;
  afterCrash: boolean;
  /** Plantage qui vient d'arriver : la description est préremplie. */
  crash: Crash | null;
  /** Ouverte d'elle-même après le plantage : case « Ne plus demander pour ce jeu ». */
  proposed: boolean;
  /** Le joueur a coché « Ne plus demander pour ce jeu ». */
  onMute: () => void;
  onClose: () => void;
  onSent: () => void;
}) {
  const [text, setText] = useState(() => (crash ? crashDescription(crash) : ""));
  const [mute, setMute] = useState(false);
  const [files, setFiles] = useState<string[] | null>(null);
  const [sending, setSending] = useState(false);
  const [error, setError] = useState<string | null>(null);
  /** Ferme la fenêtre, en retenant « Ne plus demander » s'il est coché. */
  function close() {
    if (mute) onMute();
    onClose();
  }
  useEffect(() => {
    backend
      .reportFiles(romId)
      .then(setFiles)
      .catch(() => setFiles([]));
  }, [romId]);
  async function send() {
    setSending(true);
    setError(null);
    try {
      await backend.sendReport(romId, text, afterCrash);
      window.dispatchEvent(new Event(REPORT_SENT_EVENT));
      if (mute) onMute();
      onSent();
    } catch (e) {
      setError(errorMessage(e));
      setSending(false);
    }
  }
  return (
    <Dialog
      title={proposed ? "Le jeu s'est arrêté" : "Signaler un problème"}
      onClose={sending ? () => {} : close}
      actions={
        <>
          <button className="n-btn" onClick={close} disabled={sending}>
            {proposed ? "Ne pas envoyer" : "Annuler"}
          </button>
          <button className="n-btn" onClick={send} disabled={sending || text.trim().length < 5}>
            {sending ? "Envoi…" : "Envoyer"}
          </button>
        </>
      }
    >
      {proposed ? (
        <p>
          <strong>{name}</strong> s'est fermé brusquement. Envoyer un rapport aide à corriger le problème : ajoutez ce que vous faisiez à ce moment-là.
        </p>
      ) : (
        <p>
          Qu'est-ce qui ne va pas avec <strong>{name}</strong> ?
        </p>
      )}
      <textarea
        className="n-input report-text"
        rows={5}
        autoFocus
        onFocus={(e) => {
          // Curseur après le texte prérempli.
          const end = e.currentTarget.value.length;
          e.currentTarget.setSelectionRange(end, end);
        }}
        value={text}
        maxLength={4000}
        placeholder="Par exemple : le décor ne s'affiche plus, seul Yoshi est visible, depuis le niveau 2. Ça marchait hier."
        onChange={(e) => setText(e.target.value)}
      />
      <p className="n-hint">Décrivez ce que vous voyez, quand ça arrive, et si ça marchait avant. Les fichiers ci-dessous sont joints automatiquement (votre nom de compte et votre dossier personnel en sont retirés).</p>
      {files === null ? (
        <p className="n-hint">
          <span className="n-spinner" /> Recherche des journaux…
        </p>
      ) : (
        <ul className="report-files">
          {files.map((f) => (
            <li key={f}>{f}</li>
          ))}
        </ul>
      )}
      {proposed && (
        <label className="n-check">
          <input type="checkbox" checked={mute} onChange={(e) => setMute(e.target.checked)} disabled={sending} />
          Ne plus demander pour ce jeu
        </label>
      )}
      {error && <p className="n-alert">{error}</p>}
    </Dialog>
  );
}

/** Début de la description d'un rapport envoyé après un plantage. */
function crashDescription(c: Crash): string {
  const after = c.seconds < 60 ? `${c.seconds} s` : formatPlaytime(c.seconds * 1000);
  const error = c.error ? ` (erreur : ${c.error})` : "";
  return `Le jeu s'est arrêté tout seul au bout de ${after}${error}.\nCe que je faisais : `;
}

/** Date RFC 3339 du serveur, en clair. */
function formatIso(iso: string): string {
  const d = new Date(iso);
  return Number.isNaN(d.getTime()) ? iso : formatDate(d);
}

/** Historique des sauvegardes d'un jeu sur le serveur, avec restauration après confirmation. */
function SaveHistoryDialog({
  romId,
  name,
  onClose,
  onRestored,
}: {
  romId: number;
  name: string;
  onClose: () => void;
  onRestored: (v: SaveVersion) => void;
}) {
  /** `undefined` : chargement ; `null` : pas d'historique pour ce jeu. */
  const [versions, setVersions] = useState<SaveVersion[] | null | undefined>(undefined);
  const [chosen, setChosen] = useState<SaveVersion | null>(null);
  const [restoring, setRestoring] = useState(false);
  const [error, setError] = useState<string | null>(null);
  useEffect(() => {
    backend
      .saveHistory(romId)
      .then(setVersions)
      .catch((e) => {
        setError(errorMessage(e));
        setVersions([]);
      });
  }, [romId]);
  async function restore(v: SaveVersion) {
    setRestoring(true);
    setError(null);
    try {
      await backend.restoreSaveVersion(romId, v.version);
      onRestored(v);
    } catch (e) {
      setError(errorMessage(e));
      setRestoring(false);
    }
  }
  if (chosen) {
    return (
      <Dialog
        title="Restaurer la sauvegarde"
        onClose={restoring ? () => {} : () => setChosen(null)}
        actions={
          <>
            <button className="n-btn" onClick={() => setChosen(null)} disabled={restoring}>
              Annuler
            </button>
            <button className="n-btn" onClick={() => restore(chosen)} disabled={restoring}>
              {restoring ? "Restauration…" : "Restaurer"}
            </button>
          </>
        }
      >
        <p>
          Remplacer la sauvegarde actuelle de <strong>{name}</strong> par celle du <strong>{formatIso(chosen.savedAt)}</strong>
          {chosen.device ? ` (${chosen.device})` : ""} ?
        </p>
        <p className="n-hint">
          Rien n'est perdu : la sauvegarde actuelle du serveur passe d'abord dans l'historique, et celle de cet ordinateur est copiée dans les copies de
          secours du jeu.
        </p>
        {error && <p className="n-alert">{error}</p>}
      </Dialog>
    );
  }
  return (
    <Dialog
      title="Historique des sauvegardes"
      onClose={onClose}
      actions={
        <button className="n-btn" onClick={onClose}>
          Fermer
        </button>
      }
    >
      {versions === undefined ? (
        <p className="n-hint">
          <span className="n-spinner" /> Lecture de l'historique…
        </p>
      ) : versions === null ? (
        <p>Pas d'historique pour {name} : le serveur ne garde pas encore les versions précédentes de cette sauvegarde.</p>
      ) : versions.length === 0 ? (
        !error && <p>Pas encore de version précédente : elles apparaissent quand une nouvelle sauvegarde remplace l'ancienne.</p>
      ) : (
        <>
          <p className="n-hint">Les {versions.length} dernières versions remplacées sur le serveur, de la plus récente à la plus ancienne.</p>
          <ul className="save-history">
            {versions.map((v) => (
              <li key={v.version}>
                <span className="save-history-when">{formatIso(v.savedAt)}</span>
                <span className="n-hint">
                  {v.device ?? "appareil inconnu"} · {formatSize(v.size)}
                </span>
                <button className="n-link" onClick={() => setChosen(v)}>
                  Restaurer…
                </button>
              </li>
            ))}
          </ul>
        </>
      )}
      {error && <p className="n-alert">{error}</p>}
    </Dialog>
  );
}

function UninstallDialog({
  name,
  runtimeName,
  emulator,
  others,
  onClose,
  onConfirm,
}: {
  name: string;
  runtimeName: string | null;
  emulator: boolean;
  /** Autres façons de jouer installées (recomp, émulateurs) : elles partent aussi. */
  others: string[];
  onClose: () => void;
  onConfirm: (keepSaves: boolean) => void;
}) {
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
      {others.length > 0 && <p className="n-hint">Également installé avec {others.join(", ")} : tout est supprimé.</p>}
      {runtimeName && (
        <p className="n-hint">{runtimeName} n'est supprimé qu'avec le dernier jeu qui l'utilise. Les sauvegardes des autres jeux ne sont pas touchées.{emulator && " La sauvegarde conservée reste dans le dossier de la ROM."}</p>
      )}
      <label className="n-check">
        <input type="checkbox" checked={keepSaves} onChange={(e) => setKeepSaves(e.target.checked)} />
        {runtimeName ? "Conserver les sauvegardes de ce jeu" : "Conserver les sauvegardes (dossiers save / saves)"}
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
      : s.save === "pending"
        ? " Serveur injoignable : sauvegarde gardée sur ce PC, envoyée à son retour."
        : s.save === "failed"
          ? ` Sauvegarde non envoyée : ${s.saveError?.message ?? "erreur inconnue"}`
          : "";
  return time + save;
}

function parentDir(p: string): string {
  return p.replace(/[\\/][^\\/]+[\\/]?$/, "") || p;
}

/** Dossier de l'exécutable du jeu (à défaut, le dossier du jeu). */
function exeDir(gameDir: string, executable: string | null): string {
  if (!executable) return gameDir;
  const sep = gameDir.includes("\\") ? "\\" : "/";
  const rel = executable.split("/").slice(0, -1);
  return rel.length ? [gameDir.replace(/[\\/]+$/, ""), ...rel].join(sep) : gameDir;
}

/**
 * Chemin ou nom de fichier qui peut aller à la ligne après un séparateur (/ \ _ - .),
 * jamais au milieu d'un mot tant qu'un morceau tient sur la ligne.
 */
function Breakable({ text }: { text: string }) {
  // Sans « lookbehind » : absent des anciens WebKitGTK.
  const parts = text.replace(/([\\/_.-])/g, "$1\u0000").split("\u0000");
  return (
    <>
      {parts.map((part, i) => (
        <Fragment key={i}>
          {i > 0 && <wbr />}
          {part}
        </Fragment>
      ))}
    </>
  );
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
  installDir,
  playing,
  onClose,
  onChoose,
}: {
  name: string;
  suggestions: string[];
  current: string | null;
  /** Dossier où la recomp est installée : ouvert par défaut. */
  installDir: string | null;
  playing: boolean;
  onClose: () => void;
  onChoose: (dir: string | null, off: boolean) => void;
}) {
  async function browse() {
    const dir = await open({
      directory: true,
      multiple: false,
      title: `Dossier des sauvegardes de ${name}`,
      defaultPath: current ?? installDir ?? undefined,
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
