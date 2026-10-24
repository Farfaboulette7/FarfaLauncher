import { useEffect, useState } from "react";
import { convertFileSrc } from "@tauri-apps/api/core";
import { backend } from "../api/backend";

// Mémoïsation pour la session : une image n'est demandée qu'une fois au Rust.
const memo = new Map<string, Promise<string>>();

function load(url: string): Promise<string> {
  let p = memo.get(url);
  if (!p) {
    p = backend.imagePath(url).then((path) => convertFileSrc(path));
    p.catch(() => memo.delete(url));
    memo.set(url, p);
  }
  return p;
}

/** URL affichable d'une image du serveur, servie depuis le cache local. */
export function useCachedImage(url: string | null | undefined): { src: string | null; failed: boolean } {
  const [state, setState] = useState<{ src: string | null; failed: boolean }>({ src: null, failed: false });

  useEffect(() => {
    let alive = true;
    setState({ src: null, failed: false });
    if (!url) return;
    load(url)
      .then((src) => alive && setState({ src, failed: false }))
      .catch(() => alive && setState({ src: null, failed: true }));
    return () => {
      alive = false;
    };
  }, [url]);

  return state;
}

/** Première image disponible parmi `urls` (les suivantes servent de repli). */
export function useFirstImage(urls: (string | null | undefined)[]): { src: string | null; index: number } {
  const list = urls.filter((u): u is string => !!u);
  const key = list.join("\n");
  const [state, setState] = useState<{ src: string | null; index: number }>({ src: null, index: -1 });

  useEffect(() => {
    let alive = true;
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

/** Visuel d'un jeu : la boîte en 3D si elle existe, sinon la jaquette. */
export function useBoxArt(game: { box3dUrl: string | null; coverUrl: string | null }): { src: string | null; is3d: boolean } {
  const { src, index } = useFirstImage([game.box3dUrl, game.coverUrl]);
  return { src, is3d: !!game.box3dUrl && index === 0 };
}

export function clearImageMemo() {
  memo.clear();
}
