// Apparition des champs comme dans les menus de NieR:Automata (analysée image
// par image sur une capture du jeu) :
// 1. les éléments cliquables (boutons, onglets, cartes, liens) arrivent un par
//    un, de haut en bas, en fondu avec un léger glissement (~50 ms d'écart),
//    déjà à leur taille finale ;
// 2. leur texte s'écrit ensuite de gauche à droite, environ deux lettres toutes
//    les 50 ms ; la lettre en tête est une lettre « parasite » tirée du mot
//    lui-même, le plus souvent sa première (« RestoraR », « EnhancemE »…).
// Les informations non cliquables (titres, panneaux, textes) sont là d'emblée,
// comme les fenêtres d'information du jeu.
//
// Le texte n'est jamais modifié : il est déjà entier à sa place, seules les
// lettres pas encore écrites sont masquées (API CSS Highlight). Ainsi rien ne
// bouge, même un texte centré, et React garde la main sur le contenu. La lettre
// parasite est dessinée par-dessus, à l'emplacement exact de la lettre suivante.
import { DependencyList, RefObject, useLayoutEffect, useRef } from "react";

/** Éléments cliquables : ce sont eux qui apparaissent un par un. */
const FIELDS = [".n-btn", ".lib-tab", ".n-link", ".n-input", ".card", ".pcard", ".now-playing-pause"].join(",");

const STAGGER_MS = 45;
/** Au-delà, tout le reste arrive en même temps (grandes grilles). */
const MAX_DELAY_MS = 650;
/** Le texte commence à s'écrire une fois le champ posé (après son glissement de 160 ms). */
const TEXT_DELAY_MS = 170;
const CHAR_MS = 24;
/** Textes plus longs : affichés d'un coup. */
const MAX_TYPED = 60;
/** Nom du surlignage CSS qui rend transparentes les lettres pas encore écrites. */
const HIDDEN = "n-reveal-hidden";

interface Typing {
  node: Text;
  text: string;
  start: number;
  /** Partie encore masquée : de la lettre courante à la fin. */
  range: Range;
  field: HTMLElement;
  glitch: HTMLSpanElement | null;
}

let typings: Typing[] = [];
let frame = 0;
let highlight: Highlight | null = null;

function supported(): boolean {
  return typeof CSS !== "undefined" && "highlights" in CSS && typeof Highlight !== "undefined";
}

function hiddenRanges(): Highlight {
  if (!highlight) {
    highlight = new Highlight();
    CSS.highlights.set(HIDDEN, highlight);
  }
  return highlight;
}

function glitchChar(text: string): string {
  const letters = text.replace(/\s/g, "");
  if (!letters) return "";
  return Math.random() < 0.5 ? letters[0] : letters[Math.floor(Math.random() * letters.length)];
}

/** Lettre parasite posée exactement sur la lettre `index` (cachée en dessous). */
function placeGlitch(t: Typing, index: number) {
  const r = document.createRange();
  r.setStart(t.node, index);
  r.setEnd(t.node, index + 1);
  const box = r.getBoundingClientRect();
  const field = t.field.getBoundingClientRect();
  // Échelle d'un éventuel zoom de transition sur un parent.
  const scale = t.field.offsetWidth > 0 ? field.width / t.field.offsetWidth : 1;
  if (!t.glitch) {
    const g = document.createElement("span");
    g.className = "n-reveal-glitch";
    g.setAttribute("aria-hidden", "true");
    const style = getComputedStyle(t.node.parentElement ?? t.field);
    g.style.font = style.font;
    g.style.letterSpacing = style.letterSpacing;
    g.style.textTransform = style.textTransform;
    g.style.color = style.color;
    t.glitch = g;
    t.field.appendChild(g);
  }
  const g = t.glitch;
  g.textContent = glitchChar(t.text);
  g.style.left = `${(box.left - field.left) / scale}px`;
  g.style.top = `${(box.top - field.top) / scale}px`;
  g.style.lineHeight = `${box.height / scale}px`;
}

function tick(now: number) {
  typings = typings.filter((t) => {
    if (t.node.data !== t.text || !t.node.isConnected) return release(t);
    const n = Math.floor((now - t.start) / CHAR_MS);
    if (n >= t.text.length) return release(t);
    t.range.setStart(t.node, Math.max(0, n));
    if (n >= 0 && t.text[n].trim()) placeGlitch(t, n);
    else if (t.glitch) t.glitch.textContent = "";
    return true;
  });
  frame = typings.length > 0 ? requestAnimationFrame(tick) : 0;
}

function release(t: Typing): false {
  highlight?.delete(t.range);
  t.glitch?.remove();
  return false;
}

/** Arrête l'écriture en cours dans `root` (texte entier affiché). */
function stopIn(root: Element) {
  typings = typings.filter((t) => (root.contains(t.node) ? release(t) : true));
}

/** Fait apparaître les éléments cliquables de `root` à la manière de NieR. */
export function reveal(root: Element) {
  stopIn(root);
  const now = performance.now();
  const typing = supported();
  const fields = Array.from(root.querySelectorAll<HTMLElement>(FIELDS)).filter(
    // Un champ dans un autre champ suit son parent.
    (el, _i, all) => !all.some((p) => p !== el && p.contains(el)),
  );
  fields.forEach((el, i) => {
    const delay = Math.min(i * STAGGER_MS, MAX_DELAY_MS);
    el.style.setProperty("--reveal-delay", `${delay}ms`);
    // Attribut plutôt que classe : React réécrit `className` quand l'état d'un
    // composant change (ex. une boîte 3D devenue prête) et effacerait la classe.
    el.removeAttribute("data-reveal");
    void el.offsetWidth; // relance l'animation si elle était déjà là
    el.setAttribute("data-reveal", "");
    const done = (e: AnimationEvent) => {
      if (e.target !== el) return; // fin d'une animation d'un élément enfant
      el.removeAttribute("data-reveal");
      el.removeEventListener("animationend", done);
    };
    el.addEventListener("animationend", done);
    if (!typing) return;
    // Repère de la lettre parasite (positionnée en absolu dans le champ).
    if (getComputedStyle(el).position === "static") el.style.position = "relative";

    // Morceaux d'un même élément (« 3 » + « nouveau » + « x ») : écrits à la
    // suite, comme un seul texte. Des lignes différentes s'écrivent en parallèle.
    const written = new Map<Node | null, number>();
    const walker = document.createTreeWalker(el, NodeFilter.SHOW_TEXT);
    for (let n = walker.nextNode(); n; n = walker.nextNode()) {
      const node = n as Text;
      if (!node.data.trim() || node.data.length > MAX_TYPED) continue;
      const before = written.get(node.parentNode) ?? 0;
      written.set(node.parentNode, before + node.data.length);
      const range = document.createRange();
      range.setStart(node, 0);
      range.setEnd(node, node.data.length);
      hiddenRanges().add(range);
      const start = now + delay + TEXT_DELAY_MS + before * CHAR_MS;
      typings.push({ node, text: node.data, start, range, field: el, glitch: null });
    }
  });
  if (!frame && typings.length > 0) frame = requestAnimationFrame(tick);
}

interface RevealOptions {
  /** Seulement aux changements (une zone déjà révélée avec l'écran). */
  skipFirst?: boolean;
  /** Écran entier : le lecteur de musique, en haut à droite, réapparaît avec lui. */
  screen?: boolean;
}

/** Révèle `ref` au premier affichage, puis à chaque changement de `deps`. */
export function useReveal(ref: RefObject<Element | null>, deps: DependencyList, { skipFirst = false, screen = false }: RevealOptions = {}) {
  const first = useRef(true);
  useLayoutEffect(() => {
    const skip = skipFirst && first.current;
    first.current = false;
    const el = ref.current;
    if (el && !skip) reveal(el);
    const music = screen && !skip ? document.querySelector(".now-playing") : null;
    if (el && music) reveal(music);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, deps);
}
