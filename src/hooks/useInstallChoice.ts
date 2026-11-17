import { useCallback, useSyncExternalStore } from "react";

import type { InstallMode } from "../api/backend";

export type { InstallMode };

/**
 * Choix fait dans l'onglet « Informations » (section Installation) : le panneau d'installation
 * le suit. `null` : rien de choisi, le launcher propose le meilleur pour ce système.
 */
export interface InstallChoice {
  mode: InstallMode | null;
  recomp: string | null;
  emulator: string | null;
}

const EMPTY: InstallChoice = { mode: null, recomp: null, emulator: null };
const choices = new Map<number, InstallChoice>();
const listeners = new Set<() => void>();

function subscribe(listener: () => void) {
  listeners.add(listener);
  return () => listeners.delete(listener);
}

/** Choix d'installation d'un jeu, partagé entre ses composants (gardé pendant la session). */
export function useInstallChoice(romId: number): [InstallChoice, (patch: Partial<InstallChoice>) => void] {
  const choice = useSyncExternalStore(subscribe, () => choices.get(romId) ?? EMPTY);
  const update = useCallback(
    (patch: Partial<InstallChoice>) => {
      choices.set(romId, { ...(choices.get(romId) ?? EMPTY), ...patch });
      listeners.forEach((l) => l());
    },
    [romId],
  );
  return [choice, update];
}
