import { lazy, Suspense, useEffect, useState } from "react";
import { backend, formatPlaytime, formatSize, Game, Manual, Target, TARGET_LABELS } from "../api/backend";
import { useCachedImage } from "../hooks/useCachedImage";
import "./GameDetail.css";

// pdf.js est lourd : chargé seulement à l'ouverture d'un livret.
const PdfViewer = lazy(() => import("../components/PdfViewer").then((m) => ({ default: m.PdfViewer })));

type Tab = "infos" | "medias" | "livret" | "mods";

interface Props {
  game: Game;
  target: Target | null;
  onBack: () => void;
  onError: (e: unknown) => Promise<string>;
}

export function GameDetail({ game, target, onBack, onError }: Props) {
  const [tab, setTab] = useState<Tab>("infos");
  const cover = useCachedImage(game.coverUrl);

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape" && !document.querySelector(".lightbox")) onBack();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [onBack]);

  const tabs: { id: Tab; label: string; count?: number }[] = [
    { id: "infos", label: "Informations" },
    { id: "medias", label: "Médias", count: game.screenshotUrls.length },
    { id: "livret", label: "Livret", count: game.manuals.length },
    { id: "mods", label: "Mods", count: game.mods.length },
  ];

  return (
    <div className="gd">
      <header className="gd-header">
        <button className="n-link gd-back" onClick={onBack}>
          ← Bibliothèque
        </button>
        <h1 className="n-title gd-title" title={game.name}>
          {game.name}
        </h1>
      </header>
      <div className="n-rule gd-rule" />

      <div className="gd-body">
        <aside className="gd-side">
          <div className="gd-cover">{cover.src ? <img src={cover.src} alt="" /> : <span>{game.name}</span>}</div>
          <ActionPanel game={game} target={target} />
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
          <div className="gd-content n-appear" key={tab}>
            {tab === "infos" && <InfoTab game={game} onError={onError} />}
            {tab === "medias" && <MediaTab game={game} />}
            {tab === "livret" && <ManualTab game={game} onError={onError} />}
            {tab === "mods" && <ModsTab game={game} />}
          </div>
        </section>
      </div>
    </div>
  );
}

function ActionPanel({ game, target }: { game: Game; target: Target | null }) {
  return (
    <div className="n-panel gd-actions">
      <div className="n-panel-header">Installation</div>
      <div className="n-panel-body gd-actions-body">
        {!game.myArchive && !game.detailsLoaded ? (
          <p>
            <span className="n-spinner" /> Lecture des fichiers sur le serveur…
          </p>
        ) : game.myArchive ? (
          <button className="n-btn n-btn--primary n-btn--block" disabled title="Disponible à la prochaine version">
            Installer
          </button>
        ) : (
          <p className="n-alert">Non disponible pour votre système{target ? ` (${TARGET_LABELS[target]})` : ""}.</p>
        )}
        <dl className="gd-facts">
          <dt>Version serveur</dt>
          <dd>{game.serverVersion ?? "inconnue"}</dd>
          <dt>Version installée</dt>
          <dd>Non installé</dd>
          {game.myArchive && (
            <>
              <dt>Téléchargement</dt>
              <dd>{formatSize(game.myArchive.file.size + game.romFiles.reduce((s, f) => s + f.size, 0))}</dd>
            </>
          )}
        </dl>
        <div>
          <p className="n-label">Plateformes disponibles</p>
          <ul className="gd-platforms">
            {game.archives.length === 0 && <li className="n-hint">aucune</li>}
            {game.archives.map((a) => (
              <li key={a.target} className={a.target === game.myArchive?.target ? "is-mine" : undefined}>
                {TARGET_LABELS[a.target]}
              </li>
            ))}
          </ul>
        </div>
      </div>
    </div>
  );
}

function InfoTab({ game, onError }: { game: Game; onError: (e: unknown) => Promise<string> }) {
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
  }, [game.id, onError]);

  return (
    <div className="gd-info">
      {game.summary ? <p className="gd-summary">{game.summary}</p> : <p className="n-hint">Aucun résumé.</p>}
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
          {playtime === undefined ? <span className="n-spinner" /> : playtime === null ? "indisponible" : formatPlaytime(playtime)}
        </dd>
      </dl>
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

function ModsTab({ game }: { game: Game }) {
  if (game.mods.length === 0) {
    return game.detailsLoaded ? (
      <p className="n-hint">Aucun mod disponible.</p>
    ) : (
      <p>
        <span className="n-spinner" /> Lecture des fichiers sur le serveur…
      </p>
    );
  }
  return (
    <ul className="gd-mods">
      {game.mods.map((m) => (
        <li key={m.id}>
          <span className="gd-mod-name">{m.fileName}</span>
          <span className="n-hint">{formatSize(m.size)}</span>
        </li>
      ))}
    </ul>
  );
}
