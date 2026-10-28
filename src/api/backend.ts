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
export type CodGame = "cod4" | "waw" | "mw2" | "bo1" | "bo2" | "mw3";
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
}

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

export interface Platform {
  id: number;
  slug: string;
  fsSlug: string;
  name: string;
  romCount: number;
  /** Icônes servies par RomM, à essayer dans l'ordre. */
  iconUrls: string[];
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
  mods: RemoteFile[];
  /** Jeu PC (plateforme « Ordinateur »). */
  pc: PcGame | null;
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
  installedAt: number;
  updatedAt: number;
  gameDir: string;
  /** Jeu PC (plateforme « Ordinateur »). */
  pc: PcInstall | null;
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
}

export type InstallPhase =
  | "prepare"
  | "proton"
  | "download"
  | "verify"
  | "backup"
  | "extract"
  | "installer"
  | "client"
  | "finalize";

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
  save: "off" | "unchanged" | "uploaded" | "failed";
  saveError: AppError | null;
  /** État du jeu, quand le jeu démarré par son launcher vient d'être relevé. */
  game: InstalledGame | null;
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

export const INSTALL_PROGRESS_EVENT = "install-progress";
export const INSTALL_FINISHED_EVENT = "install-finished";

export const backend = {
  sessionStatus: () => invoke<SessionInfo>("session_status"),
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
  clearCache: () => invoke<void>("clear_cache"),
  favoritesList: () => invoke<FavoritesInfo>("favorites_list"),
  soundPaths: () => invoke<Record<string, string>>("sound_paths"),
  assetsInfo: () => invoke<AssetsInfo>("assets_info"),
  setAssetsUrl: (url: string | null) => invoke<AssetsInfo>("set_assets_url", { url }),
  musicTracks: () => invoke<MusicTrack[]>("music_tracks"),
  musicFiles: (track: MusicTrack) => invoke<{ normal: string; eightBit: string }>("music_files", { track }),
  favoriteSet: (romId: number, favorite: boolean) => invoke<number[]>("favorite_set", { romId, favorite }),
  installInfo: () => invoke<InstallInfo>("install_info"),
  /** `dest` : dossier où créer celui du jeu (sinon le dossier des jeux des paramètres). */
  installGame: (romId: number, dest?: string, proton?: string | null) =>
    invoke<void>("install_game", { romId, dest: dest ?? null, proton: proton ?? null }),
  cancelInstall: (romId: number) => invoke<void>("cancel_install", { romId }),
  executableCandidates: (romId: number) => invoke<string[]>("executable_candidates", { romId }),
  chooseExecutable: (romId: number, executable: string) => invoke<InstalledGame>("choose_executable", { romId, executable }),
  uninstallGame: (romId: number, keepSaves: boolean) => invoke<void>("uninstall_game", { romId, keepSaves }),
  openGameFolder: (romId: number, sub?: "mods_downloads") => invoke<void>("open_game_folder", { romId, sub: sub ?? null }),
  setInstallDir: (path: string | null) => invoke<string>("set_install_dir", { path }),
  /** `dest` : dossier qui contiendra celui du jeu. */
  moveGame: (romId: number, dest: string) => invoke<InstalledGame>("move_game", { romId, dest }),
  setPreferLauncher: (romId: number, prefer: boolean) => invoke<InstalledGame>("set_prefer_launcher", { romId, prefer }),
  systemInfo: () => invoke<SystemInfo>("system_info"),
  serverPing: () => invoke<number>("server_ping"),
  playGame: (romId: number, saveChoice: SaveChoice, cod?: CodChoice | null) =>
    invoke<void>("play_game", { romId, saveChoice, cod: cod ?? null }),
  protonVersions: () => invoke<ProtonVersion[]>("proton_versions"),
  setProtonVersion: (romId: number, version: string | null) => invoke<InstalledGame>("set_proton_version", { romId, version }),
  codModes: (romId: number) => invoke<CodModeInfo[]>("cod_modes", { romId }),
  saveCheck: (romId: number) => invoke<SaveCheck>("save_check", { romId }),
  setSaveLocation: (romId: number, dir: string | null, off: boolean) =>
    invoke<InstalledGame>("set_save_location", { romId, dir, off }),
  openSaveFolder: (romId: number) => invoke<void>("open_save_folder", { romId }),
  runningGames: () => invoke<number[]>("running_games"),
  flushPlaySessions: () => invoke<number>("flush_play_sessions"),
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
export function isAvailable(game: Game): boolean {
  return !!game.myArchive || !!game.pc?.runnable;
}

/** Taille à télécharger pour installer le jeu. */
export function downloadSize(game: Game): number {
  if (game.pc) return game.pc.size;
  return game.myArchive ? game.myArchive.file.size + game.romFiles.reduce((s, f) => s + f.size, 0) : 0;
}

export function hasUpdate(game: Game, inst: InstalledGame | undefined): boolean {
  return !!inst?.executable && !!game.serverVersionRaw && game.serverVersionRaw !== inst.versionRaw;
}
