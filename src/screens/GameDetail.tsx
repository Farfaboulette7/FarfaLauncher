import { CSSProperties, lazy, Suspense, useEffect, useRef, useState } from "react";
import { backend, formatPlaytime, Game, Manual, Platform, Target } from "../api/backend";
import { InstallsState } from "../hooks/useInstalls";
import { InstallFacts, InstallPanel } from "./InstallPanel";
import { ModsTab } from "./ModsTab";
import { ControlsTab } from "./ControlsTab";
import { useCachedImage } from "../hooks/useCachedImage";
import { BoxArt } from "../components/BoxArt";
import { Marquee } from "../components/Marquee";
import { useReveal } from "../reveal";
import "./GameDetail.css";

// pdf.js est lourd : chargé seulement à l'ouverture d'un livret.
const PdfViewer = lazy(() => import("../components/PdfViewer").then((m) => ({ default: m.PdfViewer })));

type Tab = "infos" | "medias" | "livret" | "mods" | "controles";

interface Props {
  game: Game;
  platform: Platform | null;
  target: Target | null;
  installs: InstallsState;
  favorite: boolean;
  /** `null` : favoris non modifiables (hors ligne, droits manquants). */
  onToggleFavorite: (() => void) | null;
  favoriteError: string | null;
  onBack: () => void;
  onError: (e: unknown) => Promise<string>;
}

export function GameDetail({ game, platform, target, installs, favorite, onToggleFavorite, favoriteError, onBack, onError }: Props) {
  const [tab, setTab] = useState<Tab>("infos");
  const [coverKind, setCoverKind] = useState<"box" | "flat" | null>(null);
  const [zoom, setZoom] = useState(false);
  const root = useRef<HTMLDivElement>(null);
  const content = useRef<HTMLDivElement>(null);
  useReveal(root, [], { screen: true });
  useReveal(content, [tab], { skipFirst: true });
  // Le contenu de l'onglet arrive juste après les boutons (plus vite à un changement d'onglet).
  const contentDelay = useRef("340ms");
  useEffect(() => {
    contentDelay.current = "140ms";
  }, []);

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape" && !document.querySelector(".lightbox, .gd-boxzoom, .pdf--full, .dialog-backdrop")) onBack();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [onBack]);

  const tabs: { id: Tab; label: string; count?: number }[] = [
    { id: "infos", label: "Informations" },
    { id: "medias", label: "Médias", count: game.screenshotUrls.length },
    { id: "livret", label: "Livret", count: game.manuals.length },
    { id: "mods", label: "Mods", count: game.mods.length },
    { id: "controles", label: "Contrôles" },
  ];

  return (
    <div className="gd" ref={root}>
      <header className="gd-header">
        <button className="n-link gd-back" data-sound="none" onClick={onBack}>
          ← Retour
        </button>
        <div className="gd-title-row">
          <div className="gd-title-block">
            <Marquee as="h1" className="n-title gd-title" text={game.name} />
            {platform && <p className="n-subtitle gd-platform">{platform.name}</p>}
          </div>
          <button
            className="n-btn gd-fav"
            aria-pressed={favorite}
            disabled={!onToggleFavorite}
            title={onToggleFavorite ? undefined : (favoriteError ?? "Favoris indisponibles hors ligne")}
            onClick={() => onToggleFavorite?.()}
          >
            {favorite ? "★ Favori" : "☆ Favori"}
          </button>
        </div>
      </header>
      <div className="n-rule gd-rule" />

      <div className="gd-body">
        <aside className="gd-side">
          <div className={`gd-cover${coverKind ? ` gd-cover--${coverKind}` : ""}`}>
            <BoxArt game={game} interactive autoSpin onKind={setCoverKind} />
            {coverKind === null && !game.boxFaces && <span>{game.name}</span>}
            {coverKind !== null && (
              <button
                className="gd-zoom"
                title={coverKind === "box" ? "Agrandir la boîte" : "Agrandir la jaquette"}
                aria-label={coverKind === "box" ? "Agrandir la boîte" : "Agrandir la jaquette"}
                onClick={() => setZoom(true)}
              >
                <svg viewBox="0 0 16 16" aria-hidden="true">
                  <path d="M1 6V1h5M10 1h5v5M15 10v5h-5M6 15H1v-5" />
                </svg>
              </button>
            )}
          </div>
          <InstallPanel game={game} target={target} installs={installs} onError={onError} />
        </aside>

        <section className="gd-main n-panel">
          <nav className="gd-tabs" role="tablist">
            {tabs.map((t) => (
              <button
                key={t.id}
                role="tab"
                className="n-btn gd-tab"
                aria-pressed={tab === t.id}
                aria-selected={tab === t.id}
                onClick={() => setTab(t.id)}
              >
                {t.label}
                {t.count !== undefined && <span className="gd-tab-count">{t.count}</span>}
              </button>
            ))}
          </nav>
          <div
            className="gd-content n-appear"
            key={tab}
            ref={content}
            style={{ "--appear-delay": contentDelay.current } as CSSProperties}
          >
            {tab === "infos" && <InfoTab game={game} installs={installs} onError={onError} />}
            {tab === "medias" && <MediaTab game={game} />}
            {tab === "livret" && <ManualTab game={game} onError={onError} />}
            {tab === "mods" && <ModsTab game={game} installs={installs} onError={onError} />}
            {tab === "controles" && <ControlsTab game={game} installs={installs} onError={onError} />}
          </div>
        </section>
      </div>
      {zoom && coverKind && <BoxZoom game={game} kind={coverKind} onClose={() => setZoom(false)} />}
    </div>
  );
}

/** Boîte (ou jaquette) en plein écran ; la boîte reste orientable à la souris. */
function BoxZoom({ game, kind, onClose }: { game: Game; kind: "box" | "flat"; onClose: () => void }) {
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => e.key === "Escape" && onClose();
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [onClose]);
  return (
    <div className={`gd-boxzoom gd-boxzoom--${kind}`} onClick={onClose} role="dialog" aria-modal="true" aria-label={game.name}>
      <div className="gd-boxzoom-art" onClick={kind === "box" ? (e) => e.stopPropagation() : undefined}>
        <BoxArt game={game} interactive autoSpin />
      </div>
      <span className="lightbox-count">{kind === "box" ? "Glisser pour tourner · " : ""}Échap ou clic pour fermer</span>
    </div>
  );
}

function InfoTab({ game, installs, onError }: { game: Game; installs: InstallsState; onError: (e: unknown) => Promise<string> }) {
  const refreshKey = installs.lastSession.get(game.id);
  const [playtime, setPlaytime] = useState<number | null | undefined>(undefined);

  useEffect(() => {
    let alive = true;
    backend
      .gamePlaytime(game.id)
      .then((ms) => alive && setPlaytime(ms))
      .catch(async (e) => {
        await onError(e);
        if (alive) setPlaytime(null);
      });
    return () => {
      alive = false;
    };
  }, [game.id, onError, refreshKey]);

  return (
    <div className="gd-info">
      {game.summary ? <p className="gd-summary">{game.summary}</p> : <p className="n-hint">Aucun résumé.</p>}
      <div className="gd-facts-cols">
        <section>
          <h3 className="n-section-title">Jeu</h3>
          <dl className="gd-facts gd-facts--wide">
            {game.releaseYear && (
              <>
                <dt>Sortie</dt>
                <dd>{game.releaseYear}</dd>
              </>
            )}
            {game.developers.length > 0 && (
              <>
                <dt>Développement</dt>
                <dd>{game.developers.join(", ")}</dd>
              </>
            )}
            {game.genres.length > 0 && (
              <>
                <dt>Genres</dt>
                <dd>{game.genres.join(", ")}</dd>
              </>
            )}
            <dt>Temps de jeu</dt>
            <dd>
              {playtime === undefined ? <span className="n-spinner" /> : playtime === null ? "indisponible" : playtime === 0 ? "jamais joué" : formatPlaytime(playtime)}
            </dd>
          </dl>
        </section>
        <section>
          <h3 className="n-section-title">Installation</h3>
          <dl className="gd-facts gd-facts--wide">
            <InstallFacts game={game} installs={installs} />
          </dl>
        </section>
      </div>
    </div>
  );
}

function MediaTab({ game }: { game: Game }) {
  const [open, setOpen] = useState<number | null>(null);
  if (game.screenshotUrls.length === 0) return <p className="n-hint">Aucune capture d'écran.</p>;
  return (
    <>
      <ul className="gd-shots">
        {game.screenshotUrls.map((url, i) => (
          <li key={url}>
            <Shot url={url} onClick={() => setOpen(i)} />
          </li>
        ))}
      </ul>
      {open !== null && <Lightbox urls={game.screenshotUrls} index={open} onIndex={setOpen} onClose={() => setOpen(null)} />}
    </>
  );
}

function Shot({ url, onClick }: { url: string; onClick: () => void }) {
  const { src } = useCachedImage(url);
  return (
    <button className="gd-shot" onClick={onClick}>
      {src ? <img src={src} alt="" /> : <span className="n-spinner" />}
    </button>
  );
}

function Lightbox({ urls, index, onIndex, onClose }: { urls: string[]; index: number; onIndex: (i: number) => void; onClose: () => void }) {
  const { src } = useCachedImage(urls[index]);
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") onClose();
      if (e.key === "ArrowRight") onIndex((index + 1) % urls.length);
      if (e.key === "ArrowLeft") onIndex((index - 1 + urls.length) % urls.length);
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [index, urls.length, onIndex, onClose]);

  return (
    <div className="lightbox" onClick={onClose} role="dialog" aria-modal="true">
      {src && <img src={src} alt="" onClick={(e) => e.stopPropagation()} />}
      <span className="lightbox-count">
        {index + 1} / {urls.length} · ← → pour naviguer · Échap pour fermer
      </span>
    </div>
  );
}

function ManualTab({ game, onError }: { game: Game; onError: (e: unknown) => Promise<string> }) {
  const [choice, setChoice] = useState(0);
  const [path, setPath] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const manual: Manual | undefined = game.manuals[choice];

  useEffect(() => {
    if (!manual) return;
    let alive = true;
    setPath(null);
    setError(null);
    backend
      .manualPath(game.id, manual)
      .then((p) => alive && setPath(p))
      .catch(async (e) => {
        const msg = await onError(e);
        if (alive) setError(msg);
      });
    return () => {
      alive = false;
    };
  }, [game.id, manual, onError]);

  if (!manual) return <p className="n-hint">Aucun livret pour ce jeu.</p>;
  return (
    <div className="gd-manual">
      {game.manuals.length > 1 && (
        <div className="gd-manual-choice">
          {game.manuals.map((m, i) => (
            <button key={i} className="n-btn" aria-pressed={i === choice} onClick={() => setChoice(i)}>
              {m.kind === "file" ? m.file.fileName : "Livret"}
            </button>
          ))}
        </div>
      )}
      {error && <p className="n-alert">{error}</p>}
      {!error && !path && (
        <p>
          <span className="n-spinner" /> Téléchargement du livret…
        </p>
      )}
      {path && (
        <Suspense fallback={<span className="n-spinner" />}>
          <PdfViewer path={path} />
        </Suspense>
      )}
    </div>
  );
}
