import { useCallback, useEffect, useState } from "react";
import { check, type Update } from "@tauri-apps/plugin-updater";
import { logToFile } from "../components/ErrorBoundary";

/** Nouvelle vérification pendant que le launcher tourne. */
const EVERY_MS = 30 * 60 * 1000;

export interface Updater {
  /** Mise à jour à proposer tout de suite (fenêtre), null sinon. */
  available: Update | null;
  /** Mise à jour sortie pendant que le launcher tourne : petit bouton dans l'en-tête. */
  waiting: Update | null;
  /** Fenêtre ouverte depuis ce bouton : elle peut être refermée. */
  optional: boolean;
  dismiss: () => void;
  /** Ouvre la fenêtre de la mise à jour en attente. */
  open: () => void;
  /** Vérification manuelle : renvoie un message à afficher. */
  checkNow: () => Promise<string>;
}

/** Vérifie au démarrage, puis toutes les 30 minutes, s'il existe une nouvelle version du launcher. */
export function useUpdater(): Updater {
  const [available, setAvailable] = useState<Update | null>(null);
  const [waiting, setWaiting] = useState<Update | null>(null);
  const [optional, setOptional] = useState(false);

  useEffect(() => {
    check()
      .then((u) => u && setAvailable(u))
      // Au démarrage, une vérification ratée (hors ligne…) reste silencieuse.
      .catch((e) => logToFile("warn", `vérification des mises à jour: ${String(e)}`));
    // Ensuite, sans interrompre (une partie peut être en cours) : un bouton apparaît.
    const timer = window.setInterval(() => {
      check()
        .then((u) => u && setWaiting(u))
        .catch((e) => logToFile("warn", `vérification des mises à jour: ${String(e)}`));
    }, EVERY_MS);
    return () => window.clearInterval(timer);
  }, []);

  const checkNow = useCallback(async () => {
    try {
      const u = await check();
      if (u) {
        setOptional(false);
        setAvailable(u);
        return `La version ${u.version} est disponible.`;
      }
      return "FaRFa project est à jour.";
    } catch (e) {
      logToFile("warn", `vérification des mises à jour: ${String(e)}`);
      return "Impossible de vérifier les mises à jour. Vérifiez votre connexion Internet.";
    }
  }, []);

  const open = useCallback(() => {
    if (!waiting) return;
    setOptional(true);
    setAvailable(waiting);
  }, [waiting]);

  return {
    available,
    waiting: available ? null : waiting,
    optional,
    dismiss: () => {
      // Repoussée : le bouton reste pour plus tard.
      if (available) setWaiting(available);
      setAvailable(null);
    },
    open,
    checkNow,
  };
}
