// Navigation à la manette : croix ou stick gauche pour déplacer le focus vers
// l'élément le plus proche dans la direction, A pour valider, B pour revenir
// (comme Échap), LB/RB pour changer d'onglet, stick droit pour défiler.
// Manettes lues par le Rust (gamepad.rs), sauf sous Windows (voir plus bas).
import { listen } from "@tauri-apps/api/event";
import { backend, GAMEPAD_EVENT, PadEvent, PadInfo, PadLayout } from "./api/backend";
import { confirmButtons } from "./padLayouts";

type Dir = "up" | "down" | "left" | "right";
type Listener = (e: PadEvent) => void;

const listeners = new Set<Listener>();
const layouts = new Map<number, PadLayout>();
let captures = 0;

/** Évènements bruts des manettes (page « Contrôles »). */
export function onPad(fn: Listener): () => void {
  listeners.add(fn);
  return () => listeners.delete(fn);
}

/** Famille de la manette (noms des boutons). */
export function padLayoutOf(pad: number): PadLayout {
  return layouts.get(pad) ?? "generic";
}

/** Suspend la navigation (capture d'un bouton) ; renvoie la fonction de reprise. */
export function capturePad(): () => void {
  captures++;
  let done = false;
  return () => {
    if (!done) captures--;
    done = true;
  };
}

const FOCUSABLE =
  "button:not([disabled]), a[href], input:not([disabled]):not([type='hidden']), select:not([disabled]), textarea:not([disabled]), [tabindex]:not([tabindex='-1'])";
const REPEAT_DELAY = 380;
const REPEAT_EVERY = 110;

let held: Dir | null = null;
let repeat: number | undefined;
const stick = { lx: 0, ly: 0, ry: 0 };
let stickDir: Dir | null = null;
let scrolling = false;

function visible(el: HTMLElement): boolean {
  const r = el.getBoundingClientRect();
  if (r.width === 0 || r.height === 0) return false;
  const style = getComputedStyle(el);
  return style.visibility !== "hidden" && style.display !== "none";
}

/** Zone de navigation : la fenêtre modale ouverte, sinon toute la page. */
function scope(): ParentNode {
  const modals = document.querySelectorAll<HTMLElement>("[aria-modal='true'], .dialog-backdrop");
  return modals.length ? modals[modals.length - 1] : document;
}

function candidates(): HTMLElement[] {
  return Array.from(scope().querySelectorAll<HTMLElement>(FOCUSABLE)).filter(visible);
}

function current(): HTMLElement | null {
  const el = document.activeElement as HTMLElement | null;
  if (!el || el === document.body || !visible(el) || !scope().contains(el)) return null;
  return el;
}

function focus(el: HTMLElement) {
  el.focus({ preventScroll: true });
  el.scrollIntoView({ block: "nearest", inline: "nearest" });
}

/** Élément le plus proche dans la direction : écart sur l'axe, puis décalage sur l'autre. */
function move(dir: Dir) {
  const from = current();
  const all = candidates();
  if (!from) {
    // Premier appui : la première carte de jeu, sinon le premier élément.
    const first = all.find((e) => e.classList.contains("card")) ?? all[0];
    if (first) focus(first);
    return;
  }
  const a = from.getBoundingClientRect();
  let best: HTMLElement | null = null;
  let bestScore = Infinity;
  for (const el of all) {
    if (el === from || el.contains(from) || from.contains(el)) continue;
    const b = el.getBoundingClientRect();
    let gap: number;
    let side: number;
    if (dir === "down" || dir === "up") {
      gap = dir === "down" ? b.top - a.bottom : a.top - b.bottom;
      if ((dir === "down" ? b.top + b.height / 2 <= a.top + a.height / 2 : b.top + b.height / 2 >= a.top + a.height / 2) || gap < -a.height / 2) continue;
      side = Math.max(0, b.left - a.right, a.left - b.right);
      side += Math.abs(b.left + b.width / 2 - (a.left + a.width / 2)) / 10;
    } else {
      gap = dir === "right" ? b.left - a.right : a.left - b.right;
      if ((dir === "right" ? b.left + b.width / 2 <= a.left + a.width / 2 : b.left + b.width / 2 >= a.left + a.width / 2) || gap < -a.width / 2) continue;
      side = Math.max(0, b.top - a.bottom, a.top - b.bottom);
      side += Math.abs(b.top + b.height / 2 - (a.top + a.height / 2)) / 10;
    }
    const score = Math.max(0, gap) + side * 3;
    if (score < bestScore) {
      bestScore = score;
      best = el;
    }
  }
  if (best) focus(best);
}

function press(dir: Dir) {
  held = dir;
  window.clearTimeout(repeat);
  move(dir);
  const again = (delay: number) => {
    repeat = window.setTimeout(() => {
      if (held !== dir) return;
      move(dir);
      again(REPEAT_EVERY);
    }, delay);
  };
  again(REPEAT_DELAY);
}

function release(dir: Dir) {
  if (held !== dir) return;
  held = null;
  window.clearTimeout(repeat);
}

function key(name: string) {
  const target = document.activeElement ?? document.body;
  target.dispatchEvent(new KeyboardEvent("keydown", { key: name, code: name, bubbles: true }));
}

/** Onglet précédent ou suivant de la barre d'onglets la plus profonde (fiche du jeu, sinon bibliothèque). */
function switchTab(step: number) {
  const lists = Array.from(scope().querySelectorAll<HTMLElement>("[role='tablist']")).filter(visible);
  const list = lists[lists.length - 1];
  if (!list) return;
  const tabs = Array.from(list.querySelectorAll<HTMLElement>("[role='tab']")).filter((t) => !(t as HTMLButtonElement).disabled);
  const at = tabs.findIndex((t) => t.getAttribute("aria-selected") === "true");
  const next = tabs[(at + step + tabs.length) % tabs.length];
  if (next) {
    next.click();
    focus(next);
  }
}

function scrollParent(el: Element | null): HTMLElement {
  for (let e = el; e && e !== document.body; e = e.parentElement) {
    const s = getComputedStyle(e);
    if (/(auto|scroll)/.test(s.overflowY) && e.scrollHeight > e.clientHeight) return e as HTMLElement;
  }
  const main = document.querySelector<HTMLElement>(".lib-grid-wrap, .gd-content");
  return main ?? (document.scrollingElement as HTMLElement);
}

function scrollLoop() {
  if (Math.abs(stick.ry) < 0.2) {
    scrolling = false;
    return;
  }
  scrollParent(document.activeElement).scrollBy(0, stick.ry * 18);
  requestAnimationFrame(scrollLoop);
}

function onStick() {
  const { lx, ly } = stick;
  const mag = Math.max(Math.abs(lx), Math.abs(ly));
  const dir: Dir | null = mag > 0.6 ? (Math.abs(lx) > Math.abs(ly) ? (lx > 0 ? "right" : "left") : ly > 0 ? "down" : "up") : mag < 0.4 ? null : stickDir;
  if (dir === stickDir) return;
  if (stickDir) release(stickDir);
  stickDir = dir;
  if (dir) press(dir);
}

function navigate(e: PadEvent) {
  if (e.kind === "axis") {
    if (e.axis === "lx" || e.axis === "ly") {
      stick[e.axis] = e.value;
      onStick();
    } else if (e.axis === "ry") {
      stick.ry = e.value;
      if (!scrolling && Math.abs(e.value) >= 0.2) {
        scrolling = true;
        requestAnimationFrame(scrollLoop);
      }
    }
    return;
  }
  if (e.kind !== "button") return;
  if (e.button === "up" || e.button === "down" || e.button === "left" || e.button === "right") {
    if (e.pressed) press(e.button);
    else release(e.button);
    return;
  }
  if (!e.pressed) return;
  const { confirm, back } = confirmButtons(layouts.get(e.pad) ?? "generic");
  if (e.button === confirm) {
    const el = current();
    if (el) el.click();
    else move("down");
    return;
  }
  if (e.button === back) {
    key("Escape");
    return;
  }
  switch (e.button) {
    case "lb":
      switchTab(-1);
      break;
    case "rb":
      switchTab(1);
      break;
  }
}

// Windows : les manettes Xbox (XInput) sont lues par le Rust, les autres
// (PlayStation, Nintendo) par le moteur web.
const WEB_PADS = /Windows/.test(navigator.userAgent);
const pads = new Map<number, PadInfo>();
/** Numéros des manettes du moteur web, distincts de ceux du Rust. */
const WEB_PAD_BASE = 100;

/** Manettes connues (pour la page « Contrôles »). */
export async function listPads(): Promise<PadInfo[]> {
  const native = await backend.gamepads().catch(() => []);
  return [...native, ...pads.values()];
}

function webLayout(id: string): PadLayout {
  const s = id.toLowerCase();
  if (/xbox|xinput|045e|28de/.test(s)) return "xbox";
  if (/054c|dualsense|dualshock|playstation|wireless controller/.test(s)) return "playstation";
  if (/057e|nintendo|pro controller|joy-con/.test(s)) return "nintendo";
  return "generic";
}

/** Nom lisible : l'identifiant du moteur ajoute « (STANDARD GAMEPAD Vendor: …) ». */
function webName(id: string): string {
  return id.replace(/\s*\(.*\)\s*$/, "").trim() || "Manette";
}

// Disposition « standard » de l'API Gamepad.
const STANDARD = ["south", "east", "west", "north", "lb", "rb", "lt", "rt", "select", "start", "ls", "rs", "up", "down", "left", "right", "mode"];
const AXES = ["lx", "ly", "rx", "ry"] as const;

/** Lecture des manettes par le moteur web, à chaque image ; mêmes évènements que le Rust. */
function pollWebPads(dispatch: (e: PadEvent) => void) {
  const last = new Map<number, { buttons: boolean[]; axes: number[] }>();
  const tick = () => {
    const seen = new Set<number>();
    for (const gp of navigator.getGamepads()) {
      // Manettes XInput : déjà lues par le Rust.
      if (!gp || !gp.connected || /xinput/i.test(gp.id)) continue;
      const id = WEB_PAD_BASE + gp.index;
      seen.add(id);
      if (!pads.has(id)) {
        const info: PadInfo = { pad: id, name: webName(gp.id), layout: webLayout(gp.id) };
        pads.set(gp.index, info);
        dispatch({ kind: "connected", ...info });
      }
      const prev = last.get(id) ?? { buttons: [], axes: [] };
      const buttons = gp.buttons.map((b) => b.pressed);
      const axes = gp.axes.map((v) => Math.round(v * 20) / 20);
      // Jeu au premier plan : on suit l'état sans rien envoyer.
      if (document.hasFocus()) {
        buttons.forEach((pressed, i) => {
          const name = STANDARD[i];
          if (name && pressed !== (prev.buttons[i] ?? false)) dispatch({ kind: "button", pad: id, button: name, pressed });
        });
        AXES.forEach((axis, i) => {
          if (i < axes.length && axes[i] !== (prev.axes[i] ?? 0)) dispatch({ kind: "axis", pad: id, axis, value: axes[i] });
        });
      }
      last.set(id, { buttons, axes });
    }
    for (const id of [...pads.keys()]) {
      if (!seen.has(id)) {
        pads.delete(id);
        last.delete(id);
        dispatch({ kind: "disconnected", pad: id });
      }
    }
    requestAnimationFrame(tick);
  };
  requestAnimationFrame(tick);
}

/** À appeler au démarrage. */
export function startGamepad() {
  const root = document.documentElement;
  // Contour de focus visible seulement en navigation à la manette.
  // (mousemove sans déplacement : envoyé par le moteur après un défilement.)
  const mouse = (e: MouseEvent) => (e.type === "mousedown" || e.movementX || e.movementY) && root.classList.remove("pad-nav");
  window.addEventListener("mousemove", mouse, { passive: true });
  window.addEventListener("mousedown", mouse, { passive: true });
  const dispatch = (payload: PadEvent) => {
    if (payload.kind === "connected") layouts.set(payload.pad, payload.layout);
    listeners.forEach((fn) => fn(payload));
    if (captures > 0 || payload.kind === "connected" || payload.kind === "disconnected") return;
    root.classList.add("pad-nav");
    navigate(payload);
  };
  if (WEB_PADS) pollWebPads(dispatch);
  backend
    .gamepads()
    .then((list) => list.forEach((p) => layouts.set(p.pad, p.layout)))
    .catch(() => undefined);
  listen<PadEvent>(GAMEPAD_EVENT, ({ payload }) => dispatch(payload));
}
