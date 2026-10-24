import { useCallback, useEffect, useState } from "react";
import { backend, isAppError } from "../api/backend";

export interface FavoritesState {
  ids: Set<number>;
  loading: boolean;
  /** Liste du cache (serveur injoignable) : modification impossible. */
  offline: boolean;
  /** Message d'erreur (ex. droits manquants sur un ancien token). */
  error: string | null;
  /** Le token n'a pas les droits `collections.*` : il faut se reconnecter. */
  needsRelogin: boolean;
  reload: () => Promise<void>;
  toggle: (romId: number) => Promise<void>;
}

/** Favoris = la collection « favoris » de RomM, partagée entre appareils. */
export function useFavorites(onError: (e: unknown) => Promise<string>, enabled: boolean): FavoritesState {
  const [ids, setIds] = useState<Set<number>>(new Set());
  const [loading, setLoading] = useState(true);
  const [offline, setOffline] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [needsRelogin, setNeedsRelogin] = useState(false);

  const fail = useCallback(
    async (e: unknown) => {
      setNeedsRelogin(isAppError(e) && e.code === "scope_missing");
      setError(await onError(e));
    },
    [onError],
  );

  const reload = useCallback(async () => {
    setLoading(true);
    try {
      const info = await backend.favoritesList();
      setIds(new Set(info.romIds));
      setOffline(info.offline);
      setError(null);
      setNeedsRelogin(false);
    } catch (e) {
      await fail(e);
    } finally {
      setLoading(false);
    }
  }, [fail]);

  useEffect(() => {
    if (enabled) reload();
    else setLoading(false);
  }, [enabled, reload]);

  const toggle = useCallback(
    async (romId: number) => {
      const favorite = !ids.has(romId);
      // Affichage immédiat, corrigé par la réponse du serveur.
      setIds((s) => {
        const n = new Set(s);
        if (favorite) n.add(romId);
        else n.delete(romId);
        return n;
      });
      try {
        setIds(new Set(await backend.favoriteSet(romId, favorite)));
        setError(null);
      } catch (e) {
        setIds((s) => {
          const n = new Set(s);
          if (favorite) n.delete(romId);
          else n.add(romId);
          return n;
        });
        await fail(e);
      }
    },
    [ids, fail],
  );

  return { ids, loading, offline, error, needsRelogin, reload, toggle };
}
