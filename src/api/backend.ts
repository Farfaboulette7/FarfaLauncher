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
};
