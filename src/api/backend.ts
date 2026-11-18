// Appels typés vers les commandes Rust (src-tauri/src/commands).
// Le token RomM ne transite jamais par ici : seul le côté Rust le manipule.
import { invoke } from "@tauri-apps/api/core";
import { play } from "../sounds";

export interface AppError {
  code: string;
  message: string;
}

/** Codes d'erreur qui imposent un retour à l'écran de connexion. */
export const AUTH_ERROR_CODES = new Set(["auth_invalid", "forbidden"]);

export function isAppError(e: unknown): e is AppError {
  return typeof e === "object" && e !== null && "code" in e && "message" in e;
}

/** Message à afficher pour une erreur (joue aussi le son d'erreur). */
export function errorMessage(e: unknown): string {
  play("Error");
  if (isAppError(e)) return e.message;
  console.error(e);
  return "Une erreur inattendue est survenue. Les détails sont dans les logs.";
}

export interface SessionInfo {
  serverUrl: string | null;
  connected: boolean;
  offline: boolean;
  username: string | null;
  /** Image de profil du compte RomM (à charger avec useCachedImage). */
  avatarUrl?: string | null;
  notice: string | null;
}

export interface ServerInfo {
  serverUrl: string;
  version: string;
}

export interface DeviceAuthStart {
  userCode: string;
  verificationUrl: string;
  interval: number;
  expiresIn: number;
}

export type DeviceAuthPoll =
  | { status: "pending" | "slowDown" | "denied" | "expired" }
  | { status: "approved"; session: SessionInfo };

export type Target = "Windows" | "Linux" | "Linux_Arch" | "SteamOS" | "macOS_ARM" | "macOS_Intel";

export const TARGET_LABELS: Record<Target, string> = {
  Windows: "Windows",
  Linux: "Linux",
  Linux_Arch: "Linux (Arch)",
  SteamOS: "SteamOS",
  macOS_ARM: "macOS (Apple Silicon)",
  macOS_Intel: "macOS (Intel)",
};

export interface RemoteFile {
  id: number;
  fileName: string;
  relPath: string;
  size: number;
  sha1: string | null;
  md5: string | null;
}

export interface PlatformArchive {
  target: Target;
  kind: "zip" | "appImage" | "tarGz";
  score: number;
  file: RemoteFile;
}

export type Manual = { kind: "resource"; url: string } | { kind: "file"; file: RemoteFile };

/** Jeu de la plateforme « Ordinateur ». */
export type PcKind = "portable" | "archive" | "gogInstaller" | "cod";
export type CodGame = "cod4" | "waw" | "mw2" | "bo1" | "bo2" | "mw3" | "bo3";
export type CodMode = "sp" | "mp" | "zm";

export interface PcGame {
  kind: PcKind;
  /** Système pour lequel les fichiers du jeu sont faits. */
  os: "windows" | "linux";
  cod: CodGame | null;
  files: RemoteFile[];
  size: number;
  /** Jouable sur ce système (directement ou avec Proton). */
  runnable: boolean;
  /** Jeu Windows sous Linux : lancé avec Proton. */
  proton: boolean;
}

export interface PcInstall {
  kind: PcKind;
  os: "windows" | "linux";
  cod: CodGame | null;
  args: string[];
  cwd: string | null;
  proton: { version: string | null; prefix: string } | null;
  steamAppId: number | null;
  /** Touches personnalisées (jeux DOS). */
  keys?: KeyBind[];
}

/** Touche du jeu (évènement DOSBox, ex. `key_up`) et touche ou bouton du joueur (ex. `key 119`). */
export interface KeyBind {
  target: string;
  source: string;
  targetLabel?: string | null;
  sourceLabel?: string | null;
}

export type PadEvent =
  | { kind: "button"; pad: number; button: string; pressed: boolean }
  | { kind: "axis"; pad: number; axis: "lx" | "ly" | "rx" | "ry"; value: number }
  | ({ kind: "connected" } & PadInfo)
  | { kind: "disconnected"; pad: number }
  // Pendant une partie (menu en jeu) : famille d'une manette.
  | { kind: "layout"; pad: number; layout: PadLayout };

/** Dernière version de la recomp ou de l'émulateur d'un jeu, avec ses notes. */
export interface Changelog {
  kind: string;
  name: string;
  version: string;
  date: string | null;
  url: string | null;
  notes: string;
}

export type PadLayout = "xbox" | "playstation" | "nintendo" | "generic";

export interface PadInfo {
  /** Numéro pour cette session (change à chaque rebranchement). */
  pad: number;
  name: string;
  layout: PadLayout;
  /** Identifiant durable (GUID SDL et numéro de série). */
  key: string;
  guid: string;
  vendor: number;
  product: number;
  /** Emplacement XInput (Windows, manettes Xbox). */
  xinput: number | null;
}

/** Manette choisie pour un joueur. */
export interface PlayerPad {
  key: string;
  name: string;
}

export const GAMEPAD_EVENT = "gamepad";

export interface ProtonVersion {
  name: string;
  path: string | null;
  source: "steam" | "farfa" | "remote";
  size: number | null;
  recommended: boolean;
}

export interface CodModeInfo {
  mode: CodMode;
  label: string;
  /** Passe par Plutonium : en ligne ou en LAN. */
  plutonium: boolean;
}

/** Langue de Black Ops III (boiii ne parle qu'anglais au jeu : le launcher renomme ses zones). */
export type Bo3Language = "fr" | "en";

export interface Platform {
  id: number;
  slug: string;
  fsSlug: string;
  name: string;
  romCount: number;
  /** Icônes servies par RomM, à essayer dans l'ordre. */
  iconUrls: string[];
  /** Un émulateur tourne sur ce système pour cette console. */
  hasEmulator: boolean;
}

export interface GameRuntime {
  romId: number;
  name: string;
  repo: string;
  game: string;
}

/** Lien entre un jeu installé et le runtime qui le fait tourner. */
export interface SharedGame {
  runtimeRomId: number;
  game: string;
  runtimeName: string;
  /** `emulator` : le programme vient de FarfaRomManager. */
  kind?: string | null;
}

/** Ce que la page d'un jeu sait de l'émulateur de sa console. */
export interface EmulatorOffer {
  id: string;
  name: string;
  version: string;
  versionKey: string;
  runtimeRomId: number;
  /** Build pour ce système. */
  available: boolean;
  size: number;
  systems: Target[];
  repo: string | null;
  /** Éléments de configuration que le serveur n'a pas encore. */
  configMissing: string[];
  /** Version à installer pour ce jeu : l'émulateur plus ses archives (mods, mise à jour…). */
  gameVersionKey?: string;
  /** Taille des archives de ce jeu. */
  packsSize?: number;
  /** Émulateur conseillé pour cette console : le petit message de sa fiche. */
  advice?: string | null;
}

/** Une recomp proposée pour un jeu qui en a plusieurs. */
export interface RecompOffer {
  id: string;
  name: string;
  version: string;
  /** Build pour ce système. */
  available: boolean;
  /** Systèmes qui ont une build (vide : recomp partagée). */
  systems: Target[];
  /** Dépôt du projet. */
  repo?: string | null;
  /** Taille de sa build pour ce système (0 : aucune). */
  size?: number;
  /** « Conseillé » : le petit message de sa fiche. */
  advice: string | null;
}

export interface GamesPage {
  games: Game[];
  hasMore: boolean;
}

export interface ManagerInfo {
  url: string | null;
  custom: boolean;
}

export interface Game {
  id: number;
  platformId: number;
  name: string;
  folderName: string;
  summary: string | null;
  coverUrl: string | null;
  /** Boîte en 3D détourée (ScreenScraper / gamelist), si RomM en a une. */
  box3dUrl: string | null;
  /** Faces de la boîte (avant, tranche, dos) pour la boîte 3D interactive. */
  boxFaces: { front: string; side: string; back: string | null } | null;
  screenshotUrls: string[];
  videoUrl: string | null;
  genres: string[];
  developers: string[];
  releaseYear: number | null;
  manuals: Manual[];
  archives: PlatformArchive[];
  myArchive: PlatformArchive | null;
  romFiles: RemoteFile[];
  /** Ce que l'émulateur télécharge (une seule version de cartouche, ou tout le disque). */
  emulatorRomBytes: number;
  mods: RemoteFile[];
  /** Jeu PC (plateforme « Ordinateur »). */
  pc: PcGame | null;
  /** Jeu lancé par un runtime partagé : archives, mods et version sont les siens. */
  runtime: GameRuntime | null;
  /** Ce qui distingue cette ROM de ses homonymes (`USA, Europe · Rev 1`). */
  variant?: string | null;
  /** Nombre maximal de joueurs sur une même machine (RomM). */
  players?: number | null;
  /** Console d'origine, commune à `gba` et `portage-gba`. */
  console: string;
  /** Ce jeu a une recomp. */
  recomp: boolean;
  emulator: EmulatorOffer | null;
  /** Tous les émulateurs de sa console, au choix (dans l'ordre où ils sont proposés). */
  emulators: EmulatorOffer[];
  /** Ses recomps (même s'il n'en a qu'une), disponibles ou non sur ce système. */
  recomps?: RecompOffer[];
  /** Recomp proposée : celle choisie, sinon la conseillée, sinon la première. */
  recompId?: string | null;
  versionFile: RemoteFile | null;
  serverVersion: string | null;
  /** Contenu complet de VERSION.txt (comparé pour les mises à jour). */
  serverVersionRaw: string | null;
  /** false tant que les fichiers du jeu n'ont pas été relus sur le serveur. */
  detailsLoaded: boolean;
  /** Date d'ajout sur le serveur (RFC 3339). */
  createdAt: string;
  updatedAt: string;
}

/** Événements poussés par le Rust pendant le chargement des détails. */
export const LIBRARY_GAME_EVENT = "library-game";
export const LIBRARY_DONE_EVENT = "library-done";
export interface LibraryDone {
  failed: string[];
}

export interface LibrarySnapshot {
  serverUrl: string;
  fetchedAt: number;
  platforms: Platform[];
  games: Game[];
}

export interface FavoritesInfo {
  romIds: number[];
  /** Liste du cache : serveur injoignable. */
  offline: boolean;
}

export interface InstalledGame {
  romId: number;
  shared?: SharedGame | null;
  name: string;
  version: string | null;
  versionRaw: string | null;
  target: Target;
  archiveName: string;
  executable: string | null;
  pendingCandidates: string[];
  roms: string[];
  modsDownloaded: string[];
  saves: SaveSync;
  /** Vrai jeu démarré par le launcher de l'exécutable : « Jouer » le lance directement. */
  directLaunch: DirectLaunch | null;
  /** Passer quand même par le launcher. */
  preferLauncher: boolean;
  /** Manette de chaque joueur ; vide = ordre de branchement. */
  players?: (PlayerPad | null)[];
  installedAt: number;
  updatedAt: number;
  gameDir: string;
  /** Jeu PC (plateforme « Ordinateur »). */
  pc: PcInstall | null;
  /** Façons dont ce jeu est installé (celle en service comprise) : on bascule sans retélécharger. */
  ways?: InstallWay[];
}

/** Façon de faire tourner un jeu : sa recomp (portage PC) ou un émulateur de sa console. */
export type InstallMode = "recomp" | "emulator";

/** Une recomp ou un émulateur installé pour un jeu. */
export interface InstallWay {
  kind: InstallMode;
  id: string;
  version: string | null;
  /** C'est elle que « Jouer » lance. */
  active: boolean;
  gameDir: string;
}

export interface DirectLaunch {
  exe: string;
  args: string[];
  cwd: string | null;
  env: [string, string][];
}

export interface InstallInfo {
  installRoot: string;
  games: InstalledGame[];
  active: number[];
  /** File de téléchargements, dans l'ordre (absente avec un ancien moteur). */
  queue?: { romId: number; paused: boolean }[];
  /** Limite de débit des téléchargements en Mo/s (0 : illimité). */
  downloadLimitMb?: number;
}

export type InstallPhase =
  | "prepare"
  | "proton"
  | "download"
  | "verify"
  | "backup"
  | "extract"
  | "data"
  | "place"
  | "installer"
  | "client"
  | "build"
  | "check"
  | "copy"
  | "generate"
  | "compile"
  | "warm"
  | "cleanup"
  | "shaders"
  | "cache"
  | "finalize"
  // File de téléchargements : `done` = place dans la file, `total` = sa longueur.
  | "queued"
  | "paused";

/** Le jeu attend dans la file (son tour ou une reprise), sans rien installer. */
export function isWaiting(phase: InstallPhase): boolean {
  return phase === "queued" || phase === "paused";
}

export interface InstallProgress {
  romId: number;
  phase: InstallPhase;
  done: number;
  total: number;
  /** Téléchargement : fichier reçu en cours de vérification. */
  verifying?: boolean;
}

export interface DiskInfo {
  name: string;
  mount: string;
  total: number;
  available: number;
  /** Disque où s'installent les nouveaux jeux. */
  holdsGames: boolean;
}

export interface SystemInfo {
  os: string;
  kernel: string | null;
  hostName: string | null;
  cpu: string | null;
  cpuCores: number | null;
  cpuThreads: number;
  memoryTotal: number;
  memoryAvailable: number;
  /** Secondes depuis le démarrage de l'ordinateur. */
  uptime: number;
  disks: DiskInfo[];
  gamesCount: number;
  gamesSize: number;
  cacheSize: number;
  logsSize: number;
  /** Carte graphique lue par le système (null : inconnue, l'interface lit alors WebGL). */
  gpu: GpuInfo | null;
  /** Niveau pour l'émulation, qui choisit les réglages optimisés. */
  level: "faible" | "moyen" | "fort";
}

/** Réglages optimisés d'un jeu installé, pour ce PC. */
export interface Optimizations {
  enabled: boolean;
  level: "faible" | "moyen" | "fort";
  applied: { label: string; key: string; value: string }[];
}

/** Message du fil d'un rapport : le joueur, ou Claude qui lui répond. */
export interface ReportMessage {
  de: "joueur" | "claude";
  texte: string;
  at: number | null;
}

/** Rapport de bug envoyé par ce joueur, avec la réponse après analyse. */
export interface MyReport {
  id: string;
  rom: number | null;
  game: string | null;
  /** Envoi (secondes Unix). */
  at: number | null;
  status: string;
  description: string | null;
  /** Réponse écrite après analyse, pour le joueur. */
  reponse: string | null;
  /** Ce qui a été changé pour corriger. */
  modifications: string[];
  /** Version du launcher qui apporte la correction. */
  corrigeDans: string | null;
  /** Échanges après la première réponse (absent sur un gestionnaire plus ancien). */
  messages?: ReportMessage[];
  /** Version de test de la correction : `tag` de sa release, `prete` une fois compilée. */
  versionTest?: { tag: string; prete: boolean } | null;
  /** Dernière nouvelle (secondes Unix). */
  updatedAt: number;
}

export interface GpuInfo {
  name: string;
  vendor: string;
  /** Mémoire vidéo dédiée (unifiée sur Mac), octets ; 0 si inconnue. */
  vram: number;
  driver: string | null;
}

export type InstallOutcome = { status: "installed"; game: InstalledGame } | { status: "needsChoice"; game: InstalledGame };

export interface InstallFinished {
  romId: number;
  outcome: InstallOutcome | null;
  error: AppError | null;
}

export const GAME_STARTED_EVENT = "game-started";
export const GAME_STOPPED_EVENT = "game-stopped";
export interface GameStopped {
  romId: number;
  durationMs: number;
  recorded: boolean;
  pending: number;
  /** Envoi de la sauvegarde après la partie. */
  /** `pending` : serveur injoignable, sauvegarde gardée sur ce PC et envoyée à son retour. */
  save: "off" | "unchanged" | "uploaded" | "failed" | "pending";
  saveError: AppError | null;
  /** État du jeu, quand le jeu démarré par son launcher vient d'être relevé. */
  game: InstalledGame | null;
  /** Le jeu semble avoir planté : on propose d'envoyer un rapport. */
  crashed: boolean;
  /** Détails du plantage, pour préremplir le rapport (absent avec un ancien moteur). */
  crash?: Crash | null;
  /** Ouvrir tout de suite la proposition de rapport (« Ne plus demander » pas coché). */
  askReport?: boolean;
}

export interface Crash {
  /** Durée de la partie, en secondes. */
  seconds: number;
  /** Erreur qui a arrêté le jeu, ou son code de sortie ; `null` : fermé trop vite. */
  error: string | null;
}

/** Version précédente d'une sauvegarde, gardée par FarfaRomManager. */
export interface SaveVersion {
  version: string;
  /** Date de son envoi (RFC 3339). */
  savedAt: string;
  /** Date à laquelle une autre l'a remplacée. */
  replacedAt: string;
  size: number;
  /** Appareil qui l'a envoyée (inconnu pour les plus anciennes). */
  device: string | null;
}

export interface SaveSync {
  dir: string | null;
  patterns: string[];
  off: boolean;
  syncedHash: string | null;
  syncedServer: string | null;
  /** Introuvables même après une partie : on demande à l'utilisateur. */
  searchedAfterPlay: boolean;
}

/** Réponse de la vérification des sauvegardes avant de jouer. */
export type SaveCheck =
  | { status: "off" | "ready" }
  | { status: "unknown"; suggestions: string[] }
  | { status: "conflict"; localModified: number | null; serverModified: string };

/** Mode choisi pour un Call of Duty (Plutonium : en ligne ou en LAN). */
export interface CodChoice {
  mode: CodMode;
  online: boolean;
}

/** Sauvegarde à utiliser pour la partie. */
export type SaveChoice = "local" | "server" | "unchanged";

export const MOD_PROGRESS_EVENT = "mod-progress";
export const MOD_FINISHED_EVENT = "mod-finished";
export interface ModProgress {
  romId: number;
  fileId: number;
  done: number;
  total: number;
}
export interface ModFinished {
  romId: number;
  fileId: number;
  fileName: string;
  error: AppError | null;
}

/** Serveur des musiques et bruitages. */
export interface AssetsInfo {
  /** Adresse effective ; null si elle ne peut pas être déduite de celle de RomM. */
  url: string | null;
  /** Saisie dans les paramètres (sinon déduite : romm.x → assets.x). */
  custom: boolean;
}

/** Une version d'un morceau ; boucle en secondes. */
export interface MusicVariant {
  file: string;
  loopStart: number;
  loopEnd: number;
}

export interface MusicTrack {
  id: string;
  title: string;
  normal: MusicVariant;
  eightBit: MusicVariant;
}

/** Présence d'un ami (FarfaRomManager, `/api/presence/amis`). */
export interface Friend {
  id: number;
  username: string;
  avatar: boolean;
  status: "en jeu" | "en ligne" | "hors ligne";
  game: { romId: number; name: string } | null;
  /** Début de la partie ou de la présence (Unix, secondes). */
  since: number | null;
  lastSeen: number | null;
}
export interface Friends {
  hidden: boolean;
  amis: Friend[];
}

/** Temps de jeu d'un joueur sur un jeu (statistiques entre amis). */
export interface StatsGame {
  romId: number;
  totalMs: number;
  sessions: number;
  /** Dernière partie (Unix, secondes) ; absent si le joueur apparaît hors ligne. */
  last?: number;
}
export interface PlayerStats {
  id: number;
  username: string;
  avatar: boolean;
  totalMs: number;
  sessions: number;
  games: number;
  /** 7 derniers jours ; `null` si le joueur apparaît hors ligne. */
  weekMs: number | null;
  top: StatsGame[];
  recent: StatsGame[];
  /** « Masquer mes statistiques » (son propre profil seulement). */
  hidden: boolean;
  me: boolean;
}
export interface RankingRow {
  id: number;
  username: string;
  avatar: boolean;
  totalMs: number;
  games?: number;
  sessions?: number;
}

export type GameRequestStatus = "nouveau" | "accepté" | "refusé" | "ajouté";
/** Demande de jeu envoyée au gestionnaire. */
export interface GameRequest {
  id: string;
  name: string;
  console: string;
  link: string;
  message: string;
  status: GameRequestStatus;
  /** Réponse de l'administrateur au joueur. */
  response: string;
  /** Secondes Unix. */
  createdAt: number;
  updatedAt: number;
  /** Compte qui l'a envoyée (liste de l'administrateur). */
  username?: string | null;
}

/** Nouvelle version du launcher, trouvée sur le canal choisi (stable ou bêta). */
export interface UpdateInfo {
  version: string;
  currentVersion: string;
  body: string | null;
  /** Version de test (pré-release). */
  beta: boolean;
}
export const UPDATE_PROGRESS_EVENT = "update-progress";
export type UpdateProgress = { event: "progress"; done: number; total: number | null } | { event: "finished" };

export const INSTALL_PROGRESS_EVENT = "install-progress";
export const INSTALL_FINISHED_EVENT = "install-finished";

export const backend = {
  sessionStatus: () => invoke<SessionInfo>("session_status"),
  /** Le serveur répond-il ? (Le Rust passe en mode hors ligne ou en sort.) */
  checkConnection: () => invoke<boolean>("check_connection"),
  setServer: (url: string) => invoke<ServerInfo>("set_server", { url }),
  startDeviceAuth: () => invoke<DeviceAuthStart>("start_device_auth"),
  pollDeviceAuth: () => invoke<DeviceAuthPoll>("poll_device_auth"),
  cancelDeviceAuth: () => invoke<void>("cancel_device_auth"),
  loginWithPairCode: (code: string) => invoke<SessionInfo>("login_with_pair_code", { code }),
  loginWithToken: (token: string) => invoke<SessionInfo>("login_with_token", { token }),
  logout: () => invoke<void>("logout"),
  openLogs: () => invoke<void>("open_logs"),
  openUrl: (url: string) => invoke<void>("open_url", { url }),
  systemTarget: () => invoke<Target>("system_target"),
  libraryCached: () => invoke<LibrarySnapshot | null>("library_cached"),
  libraryRefresh: () => invoke<{ snapshot: LibrarySnapshot | null; started: boolean }>("library_refresh"),
  imagePath: (url: string) => invoke<string>("image_path", { url }),
  manualPath: (romId: number, manual: Manual) => invoke<string>("manual_path", { romId, manual }),
  gamePlaytime: (romId: number) => invoke<number | null>("game_playtime", { romId }),
  gameChangelog: (romId: number, console: string) => invoke<Changelog | null>("game_changelog", { romId, console }),
  clearCache: () => invoke<void>("clear_cache"),
  favoritesList: () => invoke<FavoritesInfo>("favorites_list"),
  soundPaths: () => invoke<Record<string, string>>("sound_paths"),
  assetsInfo: () => invoke<AssetsInfo>("assets_info"),
  setAssetsUrl: (url: string | null) => invoke<AssetsInfo>("set_assets_url", { url }),
  musicTracks: () => invoke<MusicTrack[]>("music_tracks"),
  musicFiles: (track: MusicTrack) => invoke<{ normal: string; eightBit: string }>("music_files", { track }),
  favoriteSet: (romId: number, favorite: boolean) => invoke<number[]>("favorite_set", { romId, favorite }),
  installInfo: () => invoke<InstallInfo>("install_info"),
  /** Nouvelle version du launcher sur le canal choisi, `null` s'il est à jour. */
  checkUpdate: () => invoke<UpdateInfo | null>("check_update"),
  /** Télécharge et installe la version trouvée par `checkUpdate` (progression : UPDATE_PROGRESS_EVENT). */
  installUpdate: () => invoke<void>("install_update"),
  /** Installe la version de test d'un signalement (progression : UPDATE_PROGRESS_EVENT). */
  installTestVersion: (tag: string) => invoke<void>("install_test_version", { tag }),
  betaChannel: () => invoke<boolean>("beta_channel"),
  setBetaChannel: (beta: boolean) => invoke<void>("set_beta_channel", { beta }),
  /** `dest` : dossier où créer celui du jeu (sinon le dossier des jeux des paramètres). */
  installGame: (romId: number, dest?: string, proton?: string | null, withEmulator = false, emulator?: string | null, recomp?: string | null) =>
    invoke<void>("install_game", { romId, dest: dest ?? null, proton: proton ?? null, withEmulator, emulator: emulator ?? null, recomp: recomp ?? null }),
  cancelInstall: (romId: number) => invoke<void>("cancel_install", { romId }),
  /** Pause : ce qui est téléchargé est gardé, la reprise repart de là. */
  pauseInstall: (romId: number) => invoke<void>("pause_install", { romId }),
  resumeInstall: (romId: number) => invoke<void>("resume_install", { romId }),
  /** Monte (`up`) ou descend le jeu d'une place dans la file. */
  moveInstall: (romId: number, up: boolean) => invoke<void>("move_install", { romId, up }),
  /** Limite de débit en Mo/s (0 : illimité). */
  setDownloadLimit: (mb: number) => invoke<void>("set_download_limit", { mb }),
  executableCandidates: (romId: number) => invoke<string[]>("executable_candidates", { romId }),
  chooseExecutable: (romId: number, executable: string) => invoke<InstalledGame>("choose_executable", { romId, executable }),
  uninstallGame: (romId: number, keepSaves: boolean) => invoke<void>("uninstall_game", { romId, keepSaves }),
  /** Jouer avec une autre façon déjà installée (recomp ou émulateur), sans retélécharger. */
  useInstall: (romId: number, kind: InstallMode, id: string) => invoke<InstalledGame>("use_install", { romId, kind, id }),
  openGameFolder: (romId: number, sub?: "mods_downloads") => invoke<void>("open_game_folder", { romId, sub: sub ?? null }),
  ingameResume: () => invoke<void>("ingame_resume"),
  quitGame: (romId: number) => invoke<void>("quit_game", { romId }),
  hasModsFolder: (romId: number) => invoke<boolean>("has_mods_folder", { romId }),
  openModsFolder: (romId: number) => invoke<void>("open_mods_folder", { romId }),
  setInstallDir: (path: string | null) => invoke<string>("set_install_dir", { path }),
  /** `dest` : dossier qui contiendra celui du jeu. */
  moveGame: (romId: number, dest: string) => invoke<InstalledGame>("move_game", { romId, dest }),
  setPreferLauncher: (romId: number, prefer: boolean) => invoke<InstalledGame>("set_prefer_launcher", { romId, prefer }),
  /** `ask = false` : « Ne plus demander pour ce jeu » après un plantage. */
  setCrashPrompt: (romId: number, ask: boolean) => invoke<void>("set_crash_prompt", { romId, ask }),
  systemInfo: () => invoke<SystemInfo>("system_info"),
  serverPing: () => invoke<number>("server_ping"),
  optimizations: (romId: number) => invoke<Optimizations>("optimizations", { romId }),
  setOptimized: (romId: number, enabled: boolean) => invoke<void>("set_optimized", { romId, enabled }),
  reportFiles: (romId: number) => invoke<string[]>("report_files", { romId }),
  sendReport: (romId: number, description: string, afterCrash: boolean) => invoke<string>("send_report", { romId, description, afterCrash }),
  myReports: () => invoke<MyReport[]>("my_reports"),
  myGameRequests: () => invoke<GameRequest[]>("my_game_requests"),
  sendGameRequest: (name: string, console: string, link: string, message: string) =>
    invoke<GameRequest>("send_game_request", { name, console, link, message }),
  /** Toutes les demandes (administrateur) ; `null` pour un autre compte. */
  allGameRequests: () => invoke<GameRequest[] | null>("all_game_requests"),
  updateGameRequest: (id: string, status: GameRequestStatus, response: string) =>
    invoke<GameRequest>("update_game_request", { id, status, response }),
  /** Amis et leur présence ; `null` : gestionnaire sans la présence. */
  friends: () => invoke<Friends | null>("friends"),
  presenceHidden: () => invoke<boolean>("presence_hidden"),
  setPresenceHidden: (hidden: boolean) => invoke<boolean>("set_presence_hidden", { hidden }),
  friendAvatar: (id: number) => invoke<string>("friend_avatar", { id }),
  /** Profil d'un joueur (`null` : le sien) ; `null` en retour : masqué ou gestionnaire trop ancien. */
  playerStats: (id: number | null) => invoke<PlayerStats | null>("player_stats", { id }),
  statsRanking: () => invoke<RankingRow[] | null>("stats_ranking"),
  gameRanking: (romId: number) => invoke<RankingRow[] | null>("game_ranking", { romId }),
  setStatsHidden: (hidden: boolean) => invoke<boolean>("set_stats_hidden", { hidden }),
  /** Jeu demandé par un raccourci Steam (`--jouer <id>`), une seule fois. */
  takePlayRequest: () => invoke<number | null>("take_play_request"),
  steamInfo: (romId: number) => invoke<{ available: boolean; added: boolean }>("steam_info", { romId }),
  addToSteam: (romId: number) => invoke<{ appid: number; steamRunning: boolean; updated: boolean }>("add_to_steam", { romId }),
  reportsSeen: () => invoke<Record<string, number>>("reports_seen"),
  markReportsSeen: (seen: Record<string, number>) => invoke<void>("mark_reports_seen", { seen }),
  replyReport: (id: string, text: string) => invoke<{ messages: ReportMessage[]; status: string }>("reply_report", { id, text }),
  supportStatus: () => invoke<{ enLigne: boolean; vuA: number | null }>("support_status"),
  consoleGames: (platformIds: number[], offset: number) => invoke<GamesPage>("console_games", { platformIds, offset }),
  consoleGamesRange: (platformIds: number[], offset: number, limit: number) =>
    invoke<GamesPage>("console_games_range", { platformIds, offset, limit }),
  /** Index alphabétique de RomM : premier caractère (minuscule) → position dans la liste triée par nom. */
  consoleCharIndex: (platformIds: number[]) => invoke<{ index: Record<string, number>; total: number }>("console_char_index", { platformIds }),
  searchGames: (term: string) => invoke<Game[]>("search_games", { term }),
  recentGames: () => invoke<Game[]>("recent_games"),
  gamesByIds: (ids: number[]) => invoke<Game[]>("games_by_ids", { ids }),
  gameDetails: (romId: number) => invoke<Game>("game_details", { romId }),
  managerInfo: () => invoke<ManagerInfo>("manager_info"),
  setManagerUrl: (url: string | null) => invoke<ManagerInfo>("set_manager_url", { url }),
  playGame: (romId: number, saveChoice: SaveChoice, cod?: CodChoice | null, menu = false) =>
    invoke<void>("play_game", { romId, saveChoice, cod: cod ?? null, menu }),
  protonVersions: () => invoke<ProtonVersion[]>("proton_versions"),
  setProtonVersion: (romId: number, version: string | null) => invoke<InstalledGame>("set_proton_version", { romId, version }),
  codModes: (romId: number) => invoke<CodModeInfo[]>("cod_modes", { romId }),
  bo3Language: (romId: number) => invoke<Bo3Language>("bo3_language", { romId }),
  setBo3Language: (romId: number, lang: Bo3Language) => invoke<Bo3Language>("set_bo3_language", { romId, lang }),
  /** Langue choisie d'un jeu dont la recomp propose l'anglais (OpenSmash) ; null : pas de choix. */
  gameLanguage: (romId: number) => invoke<Bo3Language | null>("game_language", { romId }),
  /** Posée au prochain lancement. */
  setGameLanguage: (romId: number, lang: Bo3Language) => invoke<Bo3Language>("set_game_language", { romId, lang }),
  setKeyBinds: (romId: number, keys: KeyBind[]) => invoke<InstalledGame>("set_key_binds", { romId, keys }),
  controllerPorts: (romId: number) => invoke<number>("controller_ports", { romId }),
  setPlayers: (romId: number, players: (PlayerPad | null)[]) => invoke<InstalledGame>("set_players", { romId, players }),
  isFullscreen: () => invoke<boolean>("is_fullscreen"),
  setFullscreen: (on: boolean) => invoke<void>("set_fullscreen", { on }),
  gamepads: () => invoke<PadInfo[]>("gamepads"),
  saveCheck: (romId: number) => invoke<SaveCheck>("save_check", { romId }),
  shaderInfo: (romId: number) => invoke<Record<string, SharedShaders>>("shader_info", { romId }),
  setSaveLocation: (romId: number, dir: string | null, off: boolean) =>
    invoke<InstalledGame>("set_save_location", { romId, dir, off }),
  openSaveFolder: (romId: number) => invoke<void>("open_save_folder", { romId }),
  /** `null` : pas d'historique pour ce jeu (sauvegarde gardée par RomM). */
  saveHistory: (romId: number) => invoke<SaveVersion[] | null>("save_history", { romId }),
  restoreSaveVersion: (romId: number, version: string) => invoke<void>("restore_save_version", { romId, version }),
  runningGames: () => invoke<number[]>("running_games"),
  flushPlaySessions: () => invoke<number>("flush_play_sessions"),
  /** Envoie les sauvegardes gardées hors ligne ; rend le nombre encore en attente. */
  flushPendingSaves: () => invoke<number>("flush_pending_saves"),
  downloadedMods: (romId: number) => invoke<string[]>("downloaded_mods", { romId }),
  downloadMod: (romId: number, fileId: number) => invoke<void>("download_mod", { romId, fileId }),
};

export function formatSize(bytes: number): string {
  const units = ["o", "Ko", "Mo", "Go", "To"];
  let v = bytes;
  let i = 0;
  while (v >= 1024 && i < units.length - 1) {
    v /= 1024;
    i++;
  }
  return `${v.toLocaleString("fr-FR", { maximumFractionDigits: i >= 2 ? 1 : 0 })} ${units[i]}`;
}

export function formatPlaytime(ms: number): string {
  const totalMin = Math.round(ms / 60000);
  if (totalMin < 1) return "moins d'une minute";
  const h = Math.floor(totalMin / 60);
  const m = totalMin % 60;
  if (h === 0) return `${m} min`;
  return m === 0 ? `${h} h` : `${h} h ${String(m).padStart(2, "0")}`;
}

/** Mise à jour disponible : le VERSION.txt du serveur diffère de celui installé
 *  (ou la version installée est inconnue). */
/** Le jeu peut être installé sur ce système. */
/** Shaders d'un jeu Switch réunis sur FarfaRomManager pour un émulateur. */
export interface SharedShaders {
  count: number;
  players: number[];
  sessions: { at: string; added: number }[];
  updatedAt: string;
}

export function isAvailable(game: Game): boolean {
  return !!game.myArchive || !!game.pc?.runnable;
}

/** Taille à télécharger pour installer le jeu. */
export function downloadSize(game: Game, runtimeInstalled = false): number {
  if (game.pc) return game.pc.size;
  const roms = game.romFiles.reduce((s, f) => s + f.size, 0);
  if (runtimeInstalled) return roms;
  return game.myArchive ? game.myArchive.file.size + roms : 0;
}

export function isEmulatorInstall(inst: InstalledGame | undefined): boolean {
  return inst?.shared?.kind === "emulator";
}

/**
 * Émulateur de ce jeu : celui avec lequel il est installé, sinon celui choisi sur sa page,
 * sinon le premier proposé pour sa console.
 */
export function effectiveEmulator(game: Game, inst: InstalledGame | undefined, chosen?: string | null): EmulatorOffer | null {
  const all = game.emulators ?? [];
  if (isEmulatorInstall(inst)) return all.find((e) => e.runtimeRomId === inst!.shared!.runtimeRomId) ?? game.emulator;
  return all.find((e) => e.id === chosen) ?? game.emulator;
}

/** Le jeu, avec l'émulateur effectif à la place de celui proposé par défaut. */
export function withEffectiveEmulator(game: Game, inst: InstalledGame | undefined, chosen?: string | null): Game {
  const emulator = effectiveEmulator(game, inst, chosen);
  return emulator === game.emulator ? game : { ...game, emulator };
}

export function hasUpdate(game: Game, inst: InstalledGame | undefined): boolean {
  if (!inst?.executable) return false;
  // Installé avec l'émulateur : c'est lui qui est suivi, pas la recomp du jeu.
  const offer = effectiveEmulator(game, inst);
  const remote = isEmulatorInstall(inst) ? offer?.gameVersionKey || offer?.versionKey : game.serverVersionRaw;
  return !!remote && remote !== inst.versionRaw;
}

/** Émulateur installable pour ce jeu sur ce système. */
export function emulatorReady(game: Game): boolean {
  // Jeu fourni en archive de données à décompresser pour sa recomp : rien qu'un émulateur puisse lancer.
  // (La ROM de RomM reste installable par un émulateur : `emulatorRomBytes` la compte, données à part.)
  const onlyData =
    game.detailsLoaded && !game.emulatorRomBytes && game.romFiles.length > 0 && game.romFiles.every((f) => f.fileName.toLowerCase().endsWith(".extract.zip"));
  return !onlyData && !!game.emulator?.available && game.emulator.configMissing.length === 0;
}

/** Taille à télécharger pour installer le jeu avec son émulateur. */
export function emulatorDownloadSize(game: Game, emulatorInstalled: boolean): number {
  const roms = game.emulatorRomBytes || game.romFiles.reduce((s, f) => s + f.size, 0);
  return roms + (game.emulator?.packsSize ?? 0) + (emulatorInstalled ? 0 : (game.emulator?.size ?? 0));
}
