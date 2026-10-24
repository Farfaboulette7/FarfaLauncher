import { useCallback, useEffect, useState } from "react";
import { check, type Update } from "@tauri-apps/plugin-updater";
import { logToFile } from "../components/ErrorBoundary";

export interface Updater {
  /** Mise à jour trouvée, à proposer (null sinon). */
  available: Update | null;
  dismiss: () => void;
  /** Vérification manuelle : renvoie un message à afficher. */
  checkNow: () => Promise<string>;
}

/** Vérifie au démarrage s'il existe une nouvelle version du launcher. */
export function useUpdater(): Updater {
  const [available, setAvailable] = useState<Update | null>(null);

  useEffect(() => {
    check()
      .then((u) => u && setAvailable(u))
      // Au démarrage, une vérification ratée (hors ligne…) reste silencieuse.
      .catch((e) => logToFile("warn", `vérification des mises à jour: ${String(e)}`));
  }, []);

  const checkNow = useCallback(async () => {
    try {
      const u = await check();
      if (u) {
        setAvailable(u);
        return `La version ${u.version} est disponible.`;
      }
      return "FarfaLauncher est à jour.";
    } catch (e) {
      logToFile("warn", `vérification des mises à jour: ${String(e)}`);
      return "Impossible de vérifier les mises à jour. Vérifiez votre connexion Internet.";
    }
  }, []);

  return { available, dismiss: () => setAvailable(null), checkNow };
}
