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
// Le texte d'origine n'est jamais modifié : il reste entier à sa place, rendu
// transparent (-webkit-text-fill-color, géré par Chromium comme par WebKit),
// et une copie exactement superposée affiche les lettres déjà écrites plus la
// lettre parasite. Rien ne bouge, même un texte centré, et React garde la main
// sur le contenu. La copie vit dans le cadre qui coupe le texte (titre trop
// long) : elle est coupée avec lui.
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

interface Typing {
  node: Text;
  text: string;
  start: number;
  /** Élément rendu transparent pendant l'écriture (parent du texte). */
  hidden: HTMLElement;
  /** Élément qui accueille la copie (celui qui coupe le texte, sinon le champ). */
  host: HTMLElement;
  copy: HTMLSpanElement;
}

let typings: Typing[] = [];
let frame = 0;
/** Écritures en cours par élément masqué : on le rend visible à la dernière. */
const hiddenCount = new Map<HTMLElement, number>();

function glitchChar(text: string): string {
  const letters = text.replace(/\s/g, "");
  if (!letters) return "";
  return Math.random() < 0.5 ? letters[0] : letters[Math.floor(Math.random() * letters.length)];
}

/** Ancêtre (jusqu'au champ) qui coupe ce qui dépasse, ou le champ lui-même. */
function clipHost(from: HTMLElement, field: HTMLElement): HTMLElement {
  for (let el: HTMLElement | null = from; el && el !== field; el = el.parentElement) {
    if (getComputedStyle(el).overflow !== "visible") return el;
  }
  return field;
}

/** Texte sur une seule ligne ? (la copie superposée ne sait pas suivre un retour à la ligne) */
function singleLine(node: Text): boolean {
  const r = document.createRange();
  r.selectNodeContents(node);
  return r.getClientRects().length <= 1;
}

/** Recale la copie exactement sur le texte d'origine (suit aussi les zooms de transition). */
function place(t: Typing, shown: string) {
  const r = document.createRange();
  r.selectNodeContents(t.node);
  const box = r.getBoundingClientRect();
  const host = t.host.getBoundingClientRect();
  const scale = t.host.offsetWidth > 0 ? host.width / t.host.offsetWidth : 1;
  t.copy.textContent = shown;
  t.copy.style.left = `${(box.left - host.left) / scale - t.host.clientLeft}px`;
  t.copy.style.top = `${(box.top - host.top) / scale - t.host.clientTop}px`;
  t.copy.style.lineHeight = `${box.height / scale}px`;
}

function tick(now: number) {
  typings = typings.filter((t) => {
    if (t.node.data !== t.text || !t.node.isConnected) return release(t);
    const n = Math.floor((now - t.start) / CHAR_MS);
    if (n >= t.text.length) return release(t);
    const shown = n < 0 ? "" : t.text.slice(0, n) + (t.text[n].trim() ? glitchChar(t.text) : "");
    place(t, shown);
    return true;
  });
  frame = typings.length > 0 ? requestAnimationFrame(tick) : 0;
}

function release(t: Typing): false {
  t.copy.remove();
  const left = (hiddenCount.get(t.hidden) ?? 1) - 1;
  if (left <= 0) {
    hiddenCount.delete(t.hidden);
    t.hidden.style.removeProperty("-webkit-text-fill-color");
  } else {
    hiddenCount.set(t.hidden, left);
  }
  return false;
}

/** Arrête l'écriture en cours dans `root` (texte entier affiché). */
function stopIn(root: Element) {
  typings = typings.filter((t) => (root.contains(t.node) ? release(t) : true));
}

function makeCopy(source: HTMLElement): HTMLSpanElement {
  const copy = document.createElement("span");
  copy.className = "n-reveal-copy";
  copy.setAttribute("aria-hidden", "true");
  const style = getComputedStyle(source);
  // Propriétés une à une : le raccourci `font` calculé est vide sous certains moteurs.
  copy.style.fontFamily = style.fontFamily;
  copy.style.fontSize = style.fontSize;
  copy.style.fontWeight = style.fontWeight;
  copy.style.fontStyle = style.fontStyle;
  copy.style.fontStretch = style.fontStretch;
  copy.style.fontVariant = style.fontVariant;
  copy.style.letterSpacing = style.letterSpacing;
  copy.style.textTransform = style.textTransform;
  copy.style.color = style.color;
  return copy;
}

/** Fait apparaître les éléments cliquables de `root` à la manière de NieR. */
export function reveal(root: Element) {
  stopIn(root);
  const now = performance.now();
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

    // Textes à écrire, regroupés par élément parent (masqué d'un bloc).
    const groups = new Map<HTMLElement, Text[]>();
    const walker = document.createTreeWalker(el, NodeFilter.SHOW_TEXT);
    for (let n = walker.nextNode(); n; n = walker.nextNode()) {
      const node = n as Text;
      const parent = node.parentElement;
      if (!parent || !node.data.trim()) continue;
      groups.set(parent, [...(groups.get(parent) ?? []), node]);
    }
    for (const [parent, nodes] of groups) {
      // Un texte long ou sur plusieurs lignes : affiché tel quel.
      if (nodes.some((n) => n.data.length > MAX_TYPED || !singleLine(n))) continue;
      const host = clipHost(parent, el);
      if (getComputedStyle(host).position === "static") host.style.position = "relative";
      parent.style.setProperty("-webkit-text-fill-color", "transparent");
      hiddenCount.set(parent, (hiddenCount.get(parent) ?? 0) + nodes.length);
      // Morceaux d'un même élément (« 3 » + « nouveau » + « x ») : écrits à la suite.
      let before = 0;
      for (const node of nodes) {
        const copy = makeCopy(parent);
        host.appendChild(copy);
        const t: Typing = { node, text: node.data, start: now + delay + TEXT_DELAY_MS + before * CHAR_MS, hidden: parent, host, copy };
        before += node.data.length;
        place(t, "");
        typings.push(t);
      }
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
