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

export function clearImageMemo() {
  memo.clear();
}
