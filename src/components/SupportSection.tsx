/**
 * Section « Soutenir le projet », tout en bas des paramètres.
 *
 * Les infos (message, TWINT, PayPal…) ne sont pas dans le code : elles viennent du fichier
 * `soutien.json` de FarfaRomManager, lu par le Rust (`src-tauri/src/commands/support.rs`) à
 * chaque ouverture des paramètres. Pour les modifier : `docs/runbook.md`, « Modifier les infos
 * de don ».
 *
 * Discrète par principe : aucun popup, aucune notification, aucun rappel ailleurs. Tant que la
 * lecture n'est pas finie, ou si elle échoue (fichier absent, gestionnaire injoignable, JSON
 * invalide, `enabled: false`), le composant n'affiche **rien** : pas même un cadre vide.
 */
import { useEffect, useMemo, useRef, useState } from "react";
import qrcode from "qrcode-generator";
import { backend, SupportInfo, SupportLink } from "../api/backend";

export function SupportSection() {
  /** `null` : rien à afficher (lecture en cours, ou section cachée). */
  const [info, setInfo] = useState<SupportInfo | null>(null);

  useEffect(() => {
    // `alive` : si l'utilisateur quitte les paramètres avant la réponse, on n'y touche plus.
    let alive = true;
    backend
      .supportInfo()
      .then((i) => alive && setInfo(i))
      // La commande ne rejette jamais, mais par prudence : en cas d'erreur, section cachée.
      .catch(() => undefined);
    return () => {
      alive = false;
    };
  }, []);

  if (!info) return null;

  return (
    <section className="n-panel">
      <div className="n-panel-header">Soutenir le projet</div>
      <div className="n-panel-body settings-section">
        {info.message && <p>{info.message}</p>}
        <ul className="support-links">
          {info.links.map((link, i) => (
            <li key={i}>{link.type === "text" ? <TextLine link={link} /> : <UrlLine link={link} />}</li>
          ))}
        </ul>
      </div>
    </section>
  );
}

/** Ligne `text` : libellé, valeur, bouton « Copier » qui affiche « Copié ! » deux secondes. */
function TextLine({ link }: { link: Extract<SupportLink, { type: "text" }> }) {
  const [state, setState] = useState<"idle" | "copied" | "failed">("idle");
  const timer = useRef<number | undefined>(undefined);
  useEffect(() => () => window.clearTimeout(timer.current), []);

  async function copy() {
    try {
      await navigator.clipboard.writeText(link.value);
      setState("copied");
    } catch {
      // Presse-papiers refusé (rare) : on le dit sur le bouton, la valeur reste lisible.
      setState("failed");
    }
    window.clearTimeout(timer.current);
    timer.current = window.setTimeout(() => setState("idle"), 2000);
  }

  return (
    <div className="support-line">
      <span className="support-label">{link.label}</span>
      {/* `user-select: all` : un clic sélectionne toute la valeur, pour la copier à la main au besoin. */}
      <span className="support-value">{link.value}</span>
      <button className="n-btn n-btn--sm" onClick={() => void copy()}>
        {state === "copied" ? "Copié !" : state === "failed" ? "Copie impossible" : "Copier"}
      </button>
    </div>
  );
}

/** Ligne `url` : bouton qui ouvre le lien dans le navigateur du système, plus le QR code si demandé. */
function UrlLine({ link }: { link: Extract<SupportLink, { type: "url" }> }) {
  const [opened, setOpened] = useState(false);

  async function open() {
    try {
      // `open_url` (Rust) n'ouvre que les liens https lus dans soutien.json à cette ouverture.
      await backend.openUrl(link.value);
      setOpened(true);
    } catch {
      setOpened(false);
    }
  }

  return (
    <div className="support-line support-line--url">
      <span className="support-label">{link.label}</span>
      <button className="n-btn n-btn--sm" title={link.value} onClick={() => void open()}>
        {opened ? `${link.label} ouvert ✓` : `Ouvrir ${link.label}`}
      </button>
      {link.showQr && <QrCode text={link.value} label={link.label} />}
    </div>
  );
}

/**
 * QR code calculé ici même (bibliothèque `qrcode-generator`, embarquée dans le launcher) :
 * aucun service en ligne, rien n'est envoyé nulle part. Dessiné en SVG, un carré par module.
 * Modules foncés sur fond clair, avec la marge blanche de 4 modules exigée par la norme :
 * c'est ce qui le rend lisible par l'appareil photo d'un téléphone.
 */
function QrCode({ text, label }: { text: string; label: string }) {
  const path = useMemo(() => {
    // 0 : taille choisie automatiquement ; « M » : corrige ~15 % d'erreurs (assez pour un écran).
    const qr = qrcode(0, "M");
    qr.addData(text);
    qr.make();
    const n = qr.getModuleCount();
    let d = "";
    for (let row = 0; row < n; row++) {
      for (let col = 0; col < n; col++) {
        // +4 : décalage de la marge. Chaque module foncé = un carré de 1×1 dans le SVG.
        if (qr.isDark(row, col)) d += `M${col + 4} ${row + 4}h1v1h-1z`;
      }
    }
    return { d, size: n + 8 };
  }, [text]);

  return (
    <svg
      className="support-qr"
      viewBox={`0 0 ${path.size} ${path.size}`}
      role="img"
      aria-label={`QR code ${label} à scanner avec un téléphone`}
      shapeRendering="crispEdges"
    >
      <rect width={path.size} height={path.size} className="support-qr-bg" />
      <path d={path.d} className="support-qr-fg" />
    </svg>
  );
}
