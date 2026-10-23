// Appels typés vers les commandes Rust (src-tauri/src/commands).
// Le token RomM ne transite jamais par ici : seul le côté Rust le manipule.
import { invoke } from "@tauri-apps/api/core";

export interface AppError {
  code: string;
  message: string;
}

/** Codes d'erreur qui imposent un retour à l'écran de connexion. */
export const AUTH_ERROR_CODES = new Set(["auth_invalid", "forbidden"]);

export function isAppError(e: unknown): e is AppError {
  return typeof e === "object" && e !== null && "code" in e && "message" in e;
}

export function errorMessage(e: unknown): string {
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

export type Target = "Windows" | "Linux" | "SteamOS" | "macOS_ARM" | "macOS_Intel";

export const TARGET_LABELS: Record<Target, string> = {
  Windows: "Windows",
  Linux: "Linux",
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

export interface Game {
  id: number;
  name: string;
  folderName: string;
  summary: string | null;
  coverUrl: string | null;
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
  versionFile: RemoteFile | null;
  serverVersion: string | null;
  /** false tant que les fichiers du jeu n'ont pas été relus sur le serveur. */
  detailsLoaded: boolean;
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
  games: Game[];
}

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
  libraryRefresh: () => invoke<{ snapshot: LibrarySnapshot; started: boolean }>("library_refresh"),
  imagePath: (url: string) => invoke<string>("image_path", { url }),
  manualPath: (romId: number, manual: Manual) => invoke<string>("manual_path", { romId, manual }),
  gamePlaytime: (romId: number) => invoke<number | null>("game_playtime", { romId }),
  clearCache: () => invoke<void>("clear_cache"),
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
