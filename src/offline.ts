// Mode hors ligne : le serveur (NAS) ne répond pas. La bibliothèque vient du cache,
// les jeux installés restent jouables, les installations attendent son retour.
import { createContext, useContext, useEffect, useRef } from "react";
import { backend, SessionInfo } from "./api/backend";

/** Vrai quand le serveur est injoignable. */
export const OfflineContext = createContext(false);

export function useOffline(): boolean {
  return useContext(OfflineContext);
}

/** Test du serveur en ligne : assez espacé pour ne rien coûter. */
const ONLINE_EVERY_MS = 60_000;
/** Hors ligne : on retente plus souvent, pour revenir vite. */
const OFFLINE_EVERY_MS = 20_000;

/**
 * Surveille le serveur et met la session à jour : passage hors ligne quand il ne répond
 * plus, reconnexion automatique (avec le nom du compte) quand il revient.
 */
export function useConnectionWatch(session: SessionInfo | null, setSession: (s: SessionInfo) => void) {
  const latest = useRef(session);
  latest.current = session;
  const connected = !!session?.connected;
  const offline = !!session?.offline;
  useEffect(() => {
    if (!connected) return;
    let live = true;
    const timer = window.setInterval(
      async () => {
        let online: boolean;
        try {
          online = await backend.checkConnection();
        } catch {
          return;
        }
        const s = latest.current;
        if (!live || !s?.connected) return;
        if (online && s.offline) {
          // Reconnexion : la session relit le compte (nom affiché, client à jour).
          backend
            .sessionStatus()
            .then((next) => live && setSession(next))
            .catch((e) => console.warn("reconnexion", e));
        } else if (!online && !s.offline) {
          setSession({ ...s, offline: true });
        }
      },
      offline ? OFFLINE_EVERY_MS : ONLINE_EVERY_MS,
    );
    return () => {
      live = false;
      window.clearInterval(timer);
    };
  }, [connected, offline, setSession]);
}
