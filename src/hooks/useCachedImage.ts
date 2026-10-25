import { useEffect, useState } from "react";
import { convertFileSrc } from "@tauri-apps/api/core";
import { backend } from "../api/backend";

// Mémoïsation pour la session : une image n'est demandée qu'une fois au Rust.
// Une image absente (ou un échec réseau) n'est redemandée qu'après quelques
// minutes, pas à chaque affichage d'un écran.
const memo = new Map<string, Promise<string>>();
/** Images déjà prêtes : affichées dès le premier rendu, sans case vide. */
const ready = new Map<string, string>();
/** Images introuvables, avec l'heure de l'échec. */
const missing = new Map<string, number>();
const RETRY_MISSING_MS = 5 * 60 * 1000;

function isMissing(url: string): boolean {
  const at = missing.get(url);
  if (at === undefined) return false;
  if (Date.now() - at < RETRY_MISSING_MS) return true;
  missing.delete(url);
  memo.delete(url);
  return false;
}
/** Images décodées gardées en mémoire : revenir sur un écran est instantané. */
const decoded = new Map<string, HTMLImageElement>();
/** Ratio largeur / hauteur des images déjà chargées. */
const ratios = new Map<string, number>();

function load(url: string): Promise<string> {
  isMissing(url); // oublie un ancien échec expiré
  let p = memo.get(url);
  if (!p) {
    p = backend
      .imagePath(url)
      .then((path) => convertFileSrc(path))
      .then(async (src) => {
        const img = new Image();
        img.src = src;
        await img.decode().catch(() => undefined);
        if (img.naturalHeight > 0) ratios.set(src, img.naturalWidth / img.naturalHeight);
        decoded.set(src, img);
        ready.set(url, src);
        return src;
      });
    p.catch(() => missing.set(url, Date.now()));
    memo.set(url, p);
  }
  return p;
}

/** URL affichable d'une image du serveur, servie depuis le cache local. */
export function useCachedImage(url: string | null | undefined): { src: string | null; failed: boolean } {
  const initial = () => ({ src: (url && ready.get(url)) || null, failed: !!url && isMissing(url) });
  const [state, setState] = useState<{ src: string | null; failed: boolean }>(initial);

  useEffect(() => {
    let alive = true;
    const now = initial();
    setState((s) => (s.src === now.src && s.failed === now.failed ? s : now));
    if (!url || now.src || now.failed) return;
    load(url)
      .then((src) => alive && setState({ src, failed: false }))
      .catch(() => alive && setState({ src: null, failed: true }));
    return () => {
      alive = false;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [url]);

  return state;
}

/** Première image disponible parmi `urls` (les suivantes servent de repli). */
export function useFirstImage(urls: (string | null | undefined)[]): { src: string | null; index: number } {
  const list = urls.filter((u): u is string => !!u);
  const key = list.join("\n");
  /** Réponse immédiate si tout ce qu'il faut savoir est déjà connu. */
  const known = (): { src: string | null; index: number } | null => {
    for (let i = 0; i < list.length; i++) {
      const src = ready.get(list[i]);
      if (src) return { src, index: i };
      if (!isMissing(list[i])) return null;
    }
    return { src: null, index: -1 };
  };
  const [state, setState] = useState<{ src: string | null; index: number }>(() => known() ?? { src: null, index: -1 });

  useEffect(() => {
    let alive = true;
    const now = known();
    if (now) {
      setState((s) => (s.src === now.src && s.index === now.index ? s : now));
      return;
    }
    setState({ src: null, index: -1 });
    (async () => {
      for (let i = 0; i < list.length; i++) {
        try {
          const src = await load(list[i]);
          if (alive) setState({ src, index: i });
          return;
        } catch {
          // image absente : on essaie la suivante
        }
      }
    })();
    return () => {
      alive = false;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [key]);

  return state;
}

/** Ratio largeur / hauteur d'une image chargée par ce module, s'il est connu. */
export function knownRatio(src: string | null): number | null {
  return (src && ratios.get(src)) || null;
}

/** Visuel d'un jeu : la boîte en 3D si elle existe, sinon la jaquette. */
export function useBoxArt(game: { box3dUrl: string | null; coverUrl: string | null }): { src: string | null; is3d: boolean } {
  const { src, index } = useFirstImage([game.box3dUrl, game.coverUrl]);
  return { src, is3d: !!game.box3dUrl && index === 0 };
}

export function clearImageMemo() {
  memo.clear();
  ready.clear();
  missing.clear();
  decoded.clear();
  ratios.clear();
}
