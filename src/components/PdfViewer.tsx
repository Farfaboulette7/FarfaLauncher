// Visualiseur PDF intégré (pdf.js, Apache-2.0), identique sur Windows, Linux et macOS.
import { useEffect, useRef, useState } from "react";
import { convertFileSrc } from "@tauri-apps/api/core";
import * as pdfjs from "pdfjs-dist";
import type { PDFDocumentProxy } from "pdfjs-dist";
import workerUrl from "pdfjs-dist/build/pdf.worker.min.mjs?url";
import "./PdfViewer.css";

pdfjs.GlobalWorkerOptions.workerSrc = workerUrl;

export function PdfViewer({ path }: { path: string }) {
  const [doc, setDoc] = useState<PDFDocumentProxy | null>(null);
  const [error, setError] = useState(false);
  const [zoom, setZoom] = useState(1);
  const [width, setWidth] = useState(0);
  const scroller = useRef<HTMLDivElement>(null);

  useEffect(() => {
    let cancelled = false;
    const task = pdfjs.getDocument({ url: convertFileSrc(path) });
    task.promise.then(
      (d) => {
        if (!cancelled) setDoc(d);
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

  useEffect(() => {
    const el = scroller.current;
    if (!el) return;
    const ro = new ResizeObserver(() => setWidth(el.clientWidth - 32));
    ro.observe(el);
    return () => ro.disconnect();
  }, []);

  if (error) return <p className="n-alert">Impossible d'afficher ce livret.</p>;

  return (
    <div className="pdf">
      <div className="pdf-toolbar">
        <button className="n-btn" onClick={() => setZoom((z) => Math.max(0.5, z - 0.25))} aria-label="Dézoomer">
          −
        </button>
        <span className="pdf-zoom">{Math.round(zoom * 100)} %</span>
        <button className="n-btn" onClick={() => setZoom((z) => Math.min(3, z + 0.25))} aria-label="Zoomer">
          +
        </button>
        <button className="n-link" onClick={() => setZoom(1)}>
          ajuster à la largeur
        </button>
        {doc && <span className="n-hint">{doc.numPages} pages</span>}
      </div>
      <div className="pdf-pages" ref={scroller}>
        {!doc && (
          <p>
            <span className="n-spinner" /> Chargement du livret…
          </p>
        )}
        {doc &&
          width > 0 &&
          Array.from({ length: doc.numPages }, (_, i) => <PdfPage key={i} doc={doc} index={i + 1} width={width * zoom} />)}
      </div>
    </div>
  );
}

/** Une page, rendue seulement quand elle approche de la zone visible. */
function PdfPage({ doc, index, width }: { doc: PDFDocumentProxy; index: number; width: number }) {
  const holder = useRef<HTMLDivElement>(null);
  const canvas = useRef<HTMLCanvasElement>(null);
  const [visible, setVisible] = useState(false);
  const [ratio, setRatio] = useState(1.414);

  useEffect(() => {
    const el = holder.current;
    if (!el) return;
    const io = new IntersectionObserver(([entry]) => entry.isIntersecting && setVisible(true), { rootMargin: "600px" });
    io.observe(el);
    return () => io.disconnect();
  }, []);

  useEffect(() => {
    if (!visible) return;
    let cancelled = false;
    let renderTask: { cancel: () => void } | null = null;
    doc.getPage(index).then((page) => {
      if (cancelled || !canvas.current) return;
      const base = page.getViewport({ scale: 1 });
      setRatio(base.height / base.width);
      const dpr = window.devicePixelRatio || 1;
      const viewport = page.getViewport({ scale: (width / base.width) * dpr });
      const c = canvas.current;
      c.width = Math.floor(viewport.width);
      c.height = Math.floor(viewport.height);
      const task = page.render({ canvas: c, viewport });
      renderTask = task;
      task.promise.catch(() => undefined);
    });
    return () => {
      cancelled = true;
      renderTask?.cancel();
    };
  }, [visible, doc, index, width]);

  return (
    <div className="pdf-page" ref={holder} style={{ width, height: width * ratio }}>
      <canvas ref={canvas} style={{ width: "100%", height: "100%" }} />
    </div>
  );
}
