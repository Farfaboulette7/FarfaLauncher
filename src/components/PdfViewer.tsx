// Visualiseur PDF intégré (pdf.js, Apache-2.0), identique sur Windows, Linux et macOS.
// Deux affichages : « livre » (doubles pages, tournées avec une animation) et
// « défilement » (toutes les pages les unes sous les autres).
import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import { convertFileSrc } from "@tauri-apps/api/core";
import * as pdfjs from "pdfjs-dist";
import type { PDFDocumentProxy } from "pdfjs-dist";
import workerUrl from "pdfjs-dist/build/pdf.worker.min.mjs?url";
import "./PdfViewer.css";

pdfjs.GlobalWorkerOptions.workerSrc = workerUrl;

type Mode = "book" | "scroll";
const MODE_KEY = "farfa.pdf.mode";
/** Durée de l'animation de page tournée (ms), aussi dans PdfViewer.css. */
const FLIP_MS = 700;

export function PdfViewer({ path }: { path: string }) {
  const [doc, setDoc] = useState<PDFDocumentProxy | null>(null);
  const [error, setError] = useState(false);
  const [zoom, setZoom] = useState(1);
  const [mode, setMode] = useState<Mode>(() => {
    try {
      return localStorage.getItem(MODE_KEY) === "scroll" ? "scroll" : "book";
    } catch {
      return "book";
    }
  });
  /** Dimensions de chaque page (le livre en a besoin pour couper les pages à l'italienne). */
  const [shapes, setShapes] = useState<Shape[] | null>(null);
  const area = useRef<HTMLDivElement>(null);
  const [size, setSize] = useState({ w: 0, h: 0 });

  useEffect(() => {
    let cancelled = false;
    const task = pdfjs.getDocument({ url: convertFileSrc(path) });
    task.promise.then(
      (d) => {
        if (cancelled) return;
        Promise.all(
          Array.from({ length: d.numPages }, (_, i) =>
            d.getPage(i + 1).then((p) => {
              const v = p.getViewport({ scale: 1 });
              return { w: v.width, h: v.height };
            }),
          ),
        ).then((s) => {
          if (cancelled) return;
          setShapes(s);
          setDoc(d);
        }, () => !cancelled && setError(true));
      },
      (e) => {
        console.error(e);
        if (!cancelled) setError(true);
      },
    );
    return () => {
      cancelled = true;
      task.destroy();
    };
  }, [path]);

  useLayoutEffect(() => {
    const el = area.current;
    if (!el) return;
    const ro = new ResizeObserver(() => setSize({ w: el.clientWidth - 48, h: el.clientHeight - 48 }));
    ro.observe(el);
    return () => ro.disconnect();
  }, []);

  function changeMode(m: Mode) {
    setMode(m);
    setZoom(1);
    try {
      localStorage.setItem(MODE_KEY, m);
    } catch {
      // préférence non mémorisée
    }
  }

  if (error) return <p className="n-alert">Impossible d'afficher ce livret.</p>;

  // Livre : la double page tient entière dans la zone (100 %) ; 52 px pour la
  // navigation dessous. Une page à l'italienne (scan d'une double page) occupe
  // les deux côtés : chaque côté a alors la forme d'une demi-page.
  const first = shapes?.[0];
  const sideRatio = first ? (isWide(first) ? first.h / (first.w / 2) : first.h / first.w) : 1.414;
  const sideH = Math.max(0, Math.min(size.h - 52, (size.w / 2) * sideRatio)) * zoom;
  // Défilement : une page sur la largeur, plafonnée pour rester lisible d'un coup d'œil.
  const scrollPageW = Math.max(0, Math.min(size.w, 820)) * zoom;

  return (
    <div className="pdf">
      <div className="pdf-toolbar">
        <div className="pdf-modes" role="group" aria-label="Affichage">
          <button className="n-btn" aria-pressed={mode === "book"} onClick={() => changeMode("book")}>
            Livre
          </button>
          <button className="n-btn" aria-pressed={mode === "scroll"} onClick={() => changeMode("scroll")}>
            Défilement
          </button>
        </div>
        <button className="n-btn pdf-zoom-btn" onClick={() => setZoom((z) => Math.max(0.5, z - 0.25))} aria-label="Dézoomer">
          −
        </button>
        <span className="pdf-zoom">{Math.round(zoom * 100)} %</span>
        <button className="n-btn pdf-zoom-btn" onClick={() => setZoom((z) => Math.min(3, z + 0.25))} aria-label="Zoomer">
          +
        </button>
        {zoom !== 1 && (
          <button className="n-link" onClick={() => setZoom(1)}>
            taille d'origine
          </button>
        )}
        {doc && <span className="n-hint">{doc.numPages} pages</span>}
      </div>
      <div className={`pdf-area pdf-area--${mode}`} ref={area}>
        {!doc && (
          <p>
            <span className="n-spinner" /> Chargement du livret…
          </p>
        )}
        {doc && shapes && size.w > 0 && mode === "book" && (
          <Book doc={doc} shapes={shapes} pageW={sideH / sideRatio} pageH={sideH} />
        )}
        {doc &&
          shapes &&
          size.w > 0 &&
          mode === "scroll" &&
          shapes.map((s, i) => (
            <LazyPage key={i} doc={doc} index={i + 1} width={scrollPageW} height={(scrollPageW * s.h) / s.w} />
          ))}
      </div>
    </div>
  );
}

// --- Rendu des pages ----------------------------------------------------------

/** Pages déjà dessinées, par document et par taille : tourner une page est immédiat. */
const rendered = new WeakMap<PDFDocumentProxy, Map<string, Promise<HTMLCanvasElement>>>();

function renderPage(doc: PDFDocumentProxy, index: number, width: number): Promise<HTMLCanvasElement> {
  let cache = rendered.get(doc);
  if (!cache) rendered.set(doc, (cache = new Map()));
  const key = `${index}@${Math.round(width)}`;
  let p = cache.get(key);
  if (!p) {
    p = doc.getPage(index).then(async (page) => {
      const base = page.getViewport({ scale: 1 });
      const dpr = window.devicePixelRatio || 1;
      const viewport = page.getViewport({ scale: (width / base.width) * dpr });
      const c = document.createElement("canvas");
      c.width = Math.floor(viewport.width);
      c.height = Math.floor(viewport.height);
      await page.render({ canvas: c, viewport }).promise;
      return c;
    });
    p.catch(() => cache!.delete(key));
    cache.set(key, p);
  }
  return p;
}

interface Shape {
  w: number;
  h: number;
}

/** Page à l'italienne : souvent le scan d'une double page du livret papier. */
function isWide(s: Shape) {
  return s.w > s.h * 1.15;
}

/** Ce qui occupe un côté du livre : une page entière, ou une moitié de page large. */
interface Slot {
  page: number;
  half: "full" | "left" | "right";
}

/** Copie une page rendue (ou sa moitié) dans son propre canvas. */
function PageCanvas({ doc, slot, width, height }: { doc: PDFDocumentProxy; slot: Slot; width: number; height: number }) {
  const canvas = useRef<HTMLCanvasElement>(null);
  const { page, half } = slot;
  useEffect(() => {
    let alive = true;
    renderPage(doc, page, half === "full" ? width : width * 2).then((src) => {
      const c = canvas.current;
      if (!alive || !c) return;
      const sw = half === "full" ? src.width : Math.floor(src.width / 2);
      const sx = half === "right" ? src.width - sw : 0;
      c.width = sw;
      c.height = src.height;
      c.getContext("2d")?.drawImage(src, sx, 0, sw, src.height, 0, 0, sw, src.height);
    }, () => undefined);
    return () => {
      alive = false;
    };
  }, [doc, page, half, width]);
  return <canvas ref={canvas} className="pdf-canvas" style={{ width, height }} />;
}

/** Mode défilement : une page n'est dessinée qu'à l'approche de la zone visible. */
function LazyPage({ doc, index, width, height }: { doc: PDFDocumentProxy; index: number; width: number; height: number }) {
  const holder = useRef<HTMLDivElement>(null);
  const [visible, setVisible] = useState(false);
  useEffect(() => {
    const el = holder.current;
    if (!el) return;
    const io = new IntersectionObserver(([entry]) => entry.isIntersecting && setVisible(true), { rootMargin: "600px" });
    io.observe(el);
    return () => io.disconnect();
  }, []);
  return (
    <div className="pdf-page" ref={holder} style={{ width, height }}>
      {visible && <PageCanvas doc={doc} slot={{ page: index, half: "full" }} width={width} height={height} />}
    </div>
  );
}

// --- Mode livre -----------------------------------------------------------------

type View = [Slot | null, Slot | null];

/**
 * Doubles pages comme un livret papier : la couverture seule à droite, puis
 * (2, 3), (4, 5)… Une page large est coupée en deux côtés ; une page restée
 * seule avant elle garde un côté vide.
 */
function viewsOf(shapes: Shape[]): View[] {
  const views: View[] = [];
  let pending: Slot | null = null;
  shapes.forEach((s, i) => {
    const page = i + 1;
    if (isWide(s)) {
      if (pending) views.push([pending, null]);
      pending = null;
      views.push([{ page, half: "left" }, { page, half: "right" }]);
    } else if (i === 0) {
      views.push([null, { page, half: "full" }]);
    } else if (pending) {
      views.push([pending, { page, half: "full" }]);
      pending = null;
    } else {
      pending = { page, half: "full" };
    }
  });
  if (pending) views.push([pending, null]);
  return views;
}

interface Flip {
  dir: "next" | "prev";
  to: number;
}

function Book({ doc, shapes, pageW, pageH }: { doc: PDFDocumentProxy; shapes: Shape[]; pageW: number; pageH: number }) {
  const spreads = useMemo(() => viewsOf(shapes), [shapes]);
  const [at, setAt] = useState(0);
  const [flip, setFlip] = useState<Flip | null>(null);
  const flipTimer = useRef<number | null>(null);

  const finish = useCallback((to: number) => {
    if (flipTimer.current) window.clearTimeout(flipTimer.current);
    flipTimer.current = null;
    setAt(to);
    setFlip(null);
  }, []);

  const go = useCallback(
    (dir: "next" | "prev") => {
      if (flip) return;
      const to = at + (dir === "next" ? 1 : -1);
      if (to < 0 || to >= spreads.length) return;
      setFlip({ dir, to });
      // Filet de sécurité si la fin d'animation n'arrive pas.
      flipTimer.current = window.setTimeout(() => finish(to), FLIP_MS + 300);
    },
    [at, flip, spreads.length, finish],
  );

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.target instanceof HTMLInputElement || document.querySelector(".lightbox, .dialog-backdrop")) return;
      if (e.key === "ArrowRight" || e.key === "PageDown") go("next");
      if (e.key === "ArrowLeft" || e.key === "PageUp") go("prev");
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [go]);

  // Pages voisines préparées à l'avance.
  useEffect(() => {
    for (const v of [spreads[at - 1], spreads[at + 1]])
      for (const s of v ?? []) if (s) void renderPage(doc, s.page, s.half === "full" ? pageW : pageW * 2);
  }, [doc, at, spreads, pageW]);

  useEffect(() => () => {
    if (flipTimer.current) window.clearTimeout(flipTimer.current);
  }, []);

  const [left, right] = spreads[at];
  const target = flip ? spreads[flip.to] : null;
  // Pendant l'animation, le dessous montre déjà la page découverte.
  const baseLeft = flip?.dir === "prev" ? target![0] : left;
  const baseRight = flip?.dir === "next" ? target![1] : right;
  const page = (s: Slot | null) => (s ? <PageCanvas doc={doc} slot={s} width={pageW} height={pageH} /> : null);
  const label = [...new Set([left?.page, right?.page].filter(Boolean))].join("–");

  return (
    <div className="pdf-book-wrap">
      <div className="pdf-book" style={{ width: pageW * 2, height: pageH }}>
        <div
          className={`pdf-side pdf-side--left${baseLeft ? "" : " is-empty"}`}
          onClick={() => go("prev")}
          title={at > 0 ? "Page précédente" : undefined}
        >
          {page(baseLeft)}
        </div>
        <div
          className={`pdf-side pdf-side--right${baseRight ? "" : " is-empty"}`}
          onClick={() => go("next")}
          title={at < spreads.length - 1 ? "Page suivante" : undefined}
        >
          {page(baseRight)}
        </div>
        {flip && (
          <div
            key={`${at}-${flip.dir}`}
            className={`pdf-leaf pdf-leaf--${flip.dir}`}
            style={{ width: pageW, height: pageH }}
            onAnimationEnd={(e) => e.animationName.startsWith("pdf-flip") && finish(flip.to)}
          >
            <div className={`pdf-leaf-face${(flip.dir === "next" ? right : left) ? "" : " is-empty"}`}>
              {page(flip.dir === "next" ? right : left)}
            </div>
            <div className={`pdf-leaf-face pdf-leaf-back${(flip.dir === "next" ? target![0] : target![1]) ? "" : " is-empty"}`}>
              {page(flip.dir === "next" ? target![0] : target![1])}
            </div>
          </div>
        )}
      </div>
      <div className="pdf-book-nav">
        <button className="n-btn" onClick={() => go("prev")} disabled={at === 0 || !!flip} aria-label="Page précédente">
          ←
        </button>
        <span className="pdf-book-pages">
          {at === 0 ? "Couverture" : label.includes("–") ? `Pages ${label}` : `Page ${label}`} / {doc.numPages}
        </span>
        <button
          className="n-btn"
          onClick={() => go("next")}
          disabled={at === spreads.length - 1 || !!flip}
          aria-label="Page suivante"
        >
          →
        </button>
      </div>
    </div>
  );
}
