import { KeyboardEvent as ReactKeyboardEvent, PointerEvent as ReactPointerEvent, useEffect, useRef, useState } from "react";
import { createPortal } from "react-dom";
import { backend, ConsoleButton, ControlsInfo, ControlsScope, Game, KEYBOARD_PLAYER, KeyBind, PadInfo, PadLayout, PadRate, PlayerBinds, PlayerPad } from "../api/backend";
import { Icon } from "../components/Icon";
import { InstallsState } from "../hooks/useInstalls";
import { capturePad, Dir, listPads, moveFocus, onPad, padLayoutOf } from "../gamepad";
import { confirmButtons, LAYOUT_NAMES, padLabel } from "../padLayouts";

// Touches du clavier (position physique, `KeyboardEvent.code`) → nom de
// l'évènement DOSBox et code SDL 1.2 (disposition américaine), comme dans
// src-tauri/src/pc/dosbox.rs.
const DOS_KEYS: Record<string, [string, number]> = {
  Escape: ["esc", 27], Space: ["space", 32], Enter: ["enter", 13], Tab: ["tab", 9], Backspace: ["bspace", 8],
  Equal: ["equals", 61], Backquote: ["grave", 96], BracketLeft: ["lbracket", 91], BracketRight: ["rbracket", 93],
  Minus: ["minus", 45], CapsLock: ["capslock", 301], Semicolon: ["semicolon", 59], Quote: ["quote", 39],
  Backslash: ["backslash", 92], ShiftLeft: ["lshift", 304], ShiftRight: ["rshift", 303], AltLeft: ["lalt", 308],
  AltRight: ["ralt", 307], ControlLeft: ["lctrl", 306], ControlRight: ["rctrl", 305], Comma: ["comma", 44],
  Period: ["period", 46], Slash: ["slash", 47], PrintScreen: ["printscreen", 316], ScrollLock: ["scrolllock", 302],
  Pause: ["pause", 19], PageDown: ["pagedown", 281], PageUp: ["pageup", 280], Insert: ["insert", 277],
  Home: ["home", 278], Delete: ["delete", 127], End: ["end", 279], ArrowUp: ["up", 273], ArrowLeft: ["left", 276],
  ArrowDown: ["down", 274], ArrowRight: ["right", 275], NumLock: ["numlock", 300], NumpadDivide: ["kp_divide", 267],
  NumpadMultiply: ["kp_multiply", 268], NumpadSubtract: ["kp_minus", 269], NumpadAdd: ["kp_plus", 270],
  NumpadDecimal: ["kp_period", 266], NumpadEnter: ["kp_enter", 271], IntlBackslash: ["lessthan", 60],
};
for (let i = 0; i < 26; i++) {
  const c = String.fromCharCode(97 + i);
  DOS_KEYS[`Key${c.toUpperCase()}`] = [c, 97 + i];
}
for (let i = 0; i < 10; i++) {
  DOS_KEYS[`Digit${i}`] = [`${i}`, 48 + i];
  DOS_KEYS[`Numpad${i}`] = [`kp_${i}`, 256 + i];
}
for (let i = 1; i <= 12; i++) DOS_KEYS[`F${i}`] = [`f${i}`, 281 + i];

const KEY_NAMES: Record<string, string> = {
  " ": "Espace", ArrowUp: "↑", ArrowDown: "↓", ArrowLeft: "←", ArrowRight: "→", Control: "Ctrl", Escape: "Échap",
  Enter: "Entrée", Shift: "Maj", Backspace: "Retour arrière", Delete: "Suppr", AltGraph: "Alt Gr",
};

/** Nom de la touche telle qu'écrite sur le clavier du joueur. */
function keyLabel(e: KeyboardEvent): string {
  const side = e.location === 1 ? " gauche" : e.location === 2 ? " droite" : e.location === 3 ? " (pavé num.)" : "";
  const name = KEY_NAMES[e.key] ?? (e.key.length === 1 ? e.key.toUpperCase() : e.key);
  return name + side;
}

// Boutons de manette → associations DOSBox (ordre d'une manette Xbox vue par
// DirectInput, le plus courant).
const PAD_BINDS: Record<string, string> = {
  south: "stick_0 button 0", east: "stick_0 button 1", west: "stick_0 button 2", north: "stick_0 button 3",
  lb: "stick_0 button 4", rb: "stick_0 button 5", select: "stick_0 button 6", start: "stick_0 button 7",
  ls: "stick_0 button 8", rs: "stick_0 button 9", up: "stick_0 hat 0 1", right: "stick_0 hat 0 2",
  down: "stick_0 hat 0 4", left: "stick_0 hat 0 8", lt: "stick_0 axis 2 1", rt: "stick_0 axis 2 0",
};
const STICK_BINDS: Record<string, [string, string, string, string]> = {
  // axe, vers le négatif, vers le positif, libellés
  lx: ["stick_0 axis 0 0", "stick_0 axis 0 1", "Stick gauche ←", "Stick gauche →"],
  ly: ["stick_0 axis 1 0", "stick_0 axis 1 1", "Stick gauche ↑", "Stick gauche ↓"],
  rx: ["stick_0 axis 3 0", "stick_0 axis 3 1", "Stick droit ←", "Stick droit →"],
  ry: ["stick_0 axis 4 0", "stick_0 axis 4 1", "Stick droit ↑", "Stick droit ↓"],
};

const isDosbox = (exe: string | null | undefined) => !!exe && /(^|\/)dosbox[^/]*$/i.test(exe);

interface Props {
  game: Game;
  installs: InstallsState;
  onError: (e: unknown) => Promise<string>;
}

/** En dessous : manette lue lentement (affichée en rouge). */
const SLOW_HZ = 500;
/** Intervalle des relevés de fréquence (le launcher ne mesure que les 5 dernières secondes). */
const RATE_POLL_MS = 5000;
/** Bouton « retour » maintenu ce temps pendant une capture : annule. */
const CANCEL_HOLD_MS = 1000;
/** Seuil d'un stick « bougé » (ligne allumée) et d'un stick « poussé » (capture). */
const STICK_LIT = 0.3;
const STICK_PUSHED = 0.7;

const KEYBOARD_NAME = "Clavier et souris";

// ——— Sources : manette, clavier, souris ———

const AXIS_NAMES: Record<string, string> = {
  "lx-": "Stick gauche ←", "lx+": "Stick gauche →", "ly-": "Stick gauche ↑", "ly+": "Stick gauche ↓",
  "rx-": "Stick droit ←", "rx+": "Stick droit →", "ry-": "Stick droit ↑", "ry+": "Stick droit ↓",
};
const MOUSE_NAMES: Record<string, string> = {
  left: "Clic gauche", right: "Clic droit", middle: "Clic milieu",
  "x-": "Souris ←", "x+": "Souris →", "y-": "Souris ↑", "y+": "Souris ↓",
};
const CODE_NAMES: Record<string, string> = {
  Space: "Espace", Enter: "Entrée", Escape: "Échap", Tab: "Tab", Backspace: "Retour arrière", CapsLock: "Verr. maj",
  ShiftLeft: "Maj", ShiftRight: "Maj droite", ControlLeft: "Ctrl", ControlRight: "Ctrl droite", AltLeft: "Alt",
  AltRight: "Alt Gr", ArrowUp: "↑", ArrowDown: "↓", ArrowLeft: "←", ArrowRight: "→", Insert: "Inser", Delete: "Suppr",
  Home: "Début", End: "Fin", PageUp: "Page ↑", PageDown: "Page ↓", NumpadAdd: "Pavé +", NumpadSubtract: "Pavé −",
  NumpadMultiply: "Pavé ×", NumpadDivide: "Pavé ÷", NumpadDecimal: "Pavé .", NumpadEnter: "Pavé Entrée",
};
/** Touches que Dolphin sait recevoir (controls.rs). */
const KNOWN_CODE =
  /^(Key[A-Z]|Digit\d|Numpad\d|F\d{1,2}|Space|Enter|Escape|Tab|Backspace|CapsLock|(Shift|Control|Alt)(Left|Right)|Arrow(Up|Down|Left|Right)|Minus|Equal|Bracket(Left|Right)|Semicolon|Quote|Backquote|Backslash|IntlBackslash|Comma|Period|Slash|Numpad(Add|Subtract|Multiply|Divide|Decimal|Enter)|Insert|Delete|Home|End|Page(Up|Down))$/;

// Lettres écrites sur le clavier du joueur (AZERTY : la touche `KeyW` est le Z).
let layoutMap: Map<string, string> | null = null;
let layoutAsked = false;
function useKeyboardLayout(): void {
  const [, setReady] = useState(0);
  useEffect(() => {
    if (layoutAsked) return;
    layoutAsked = true;
    const kb = (navigator as unknown as { keyboard?: { getLayoutMap?: () => Promise<Map<string, string>> } }).keyboard;
    kb?.getLayoutMap?.()
      .then((m) => {
        layoutMap = new Map(m);
        setReady((n) => n + 1);
      })
      .catch(() => undefined);
  }, []);
}

function codeName(code: string): string {
  if (CODE_NAMES[code]) return CODE_NAMES[code];
  const printed = layoutMap?.get(code);
  if (printed && /^(Key|Digit|Minus|Equal|Bracket|Semicolon|Quote|Backquote|Backslash|IntlBackslash|Comma|Period|Slash)/.test(code)) {
    return printed.toUpperCase();
  }
  const m = /^Numpad(\d)$/.exec(code);
  if (m) return `Pavé ${m[1]}`;
  return code.replace(/^(Key|Digit)/, "");
}

function sourceName(source: string, layout: PadLayout): string {
  if (source.startsWith("key:")) return codeName(source.slice(4));
  if (source.startsWith("mouse:")) return MOUSE_NAMES[source.slice(6)] ?? source;
  return AXIS_NAMES[source] ?? padLabel(layout, source);
}

const isStick = (source: string) => /^[lr][xy][-+]$/.test(source);

// ——— Ce que font les manettes, le clavier et la souris, en direct ———

interface LivePad {
  buttons: Set<string>;
  /** Axes, arrondis au dixième (moins de rendus). */
  axes: Record<string, number>;
  /** Dernier appui (voyant). */
  hit: number;
}
interface LiveKbm {
  keys: Set<string>;
  mouse: Set<string>;
  /** Directions de la souris bougée à l'instant. */
  moves: Set<string>;
  hit: number;
}
interface Live {
  pads: Map<number, LivePad>;
  kbm: LiveKbm;
}

function useLive(): Live {
  const [pads, setPads] = useState(new Map<number, LivePad>());
  const [kbm, setKbm] = useState<LiveKbm>({ keys: new Set(), mouse: new Set(), moves: new Set(), hit: 0 });
  useEffect(() => {
    const off = onPad((e) => {
      if (e.kind === "connected" || e.kind === "layout") return;
      setPads((m) => {
        if (e.kind === "disconnected") {
          const next = new Map(m);
          next.delete(e.pad);
          return next;
        }
        const old = m.get(e.pad) ?? { buttons: new Set<string>(), axes: {}, hit: 0 };
        if (e.kind === "axis") {
          const v = Math.round(e.value * 10) / 10;
          if (old.axes[e.axis] === v) return m;
          const moved = Math.abs(v) >= STICK_LIT && Math.abs(old.axes[e.axis] ?? 0) < STICK_LIT;
          return new Map(m).set(e.pad, { ...old, axes: { ...old.axes, [e.axis]: v }, hit: moved ? Date.now() : old.hit });
        }
        const buttons = new Set(old.buttons);
        if (e.pressed) buttons.add(e.button);
        else buttons.delete(e.button);
        return new Map(m).set(e.pad, { ...old, buttons, hit: e.pressed ? Date.now() : old.hit });
      });
    });
    const mouseName = (b: number) => (b === 0 ? "left" : b === 1 ? "middle" : b === 2 ? "right" : null);
    const keyDown = (e: KeyboardEvent) => {
      if (!e.isTrusted || e.repeat) return;
      setKbm((k) => ({ ...k, keys: new Set(k.keys).add(e.code), hit: Date.now() }));
    };
    const keyUp = (e: KeyboardEvent) =>
      setKbm((k) => {
        if (!k.keys.has(e.code)) return k;
        const keys = new Set(k.keys);
        keys.delete(e.code);
        return { ...k, keys };
      });
    const down = (e: MouseEvent) => {
      const n = mouseName(e.button);
      if (n) setKbm((k) => ({ ...k, mouse: new Set(k.mouse).add(n), hit: Date.now() }));
    };
    const up = (e: MouseEvent) => {
      const n = mouseName(e.button);
      if (n)
        setKbm((k) => {
          const mouse = new Set(k.mouse);
          mouse.delete(n);
          return { ...k, mouse };
        });
    };
    let still: number | undefined;
    const move = (e: MouseEvent) => {
      const dirs = new Set<string>();
      if (e.movementX < -1) dirs.add("x-");
      if (e.movementX > 1) dirs.add("x+");
      if (e.movementY < -1) dirs.add("y-");
      if (e.movementY > 1) dirs.add("y+");
      if (dirs.size === 0) return;
      setKbm((k) => ([...dirs].every((d) => k.moves.has(d)) && k.moves.size === dirs.size ? k : { ...k, moves: dirs }));
      window.clearTimeout(still);
      still = window.setTimeout(() => setKbm((k) => (k.moves.size ? { ...k, moves: new Set() } : k)), 150);
    };
    const clear = () => setKbm((k) => ({ ...k, keys: new Set(), mouse: new Set(), moves: new Set() }));
    window.addEventListener("keydown", keyDown, true);
    window.addEventListener("keyup", keyUp, true);
    window.addEventListener("mousedown", down, true);
    window.addEventListener("mouseup", up, true);
    window.addEventListener("mousemove", move, { passive: true });
    window.addEventListener("blur", clear);
    return () => {
      off();
      window.clearTimeout(still);
      window.removeEventListener("keydown", keyDown, true);
      window.removeEventListener("keyup", keyUp, true);
      window.removeEventListener("mousedown", down, true);
      window.removeEventListener("mouseup", up, true);
      window.removeEventListener("mousemove", move);
      window.removeEventListener("blur", clear);
    };
  }, []);
  return { pads, kbm };
}

/** Le voyant reste allumé un instant après l'appui. */
function useBlink(hit: number, held: boolean): boolean {
  const [, tick] = useState(0);
  const fresh = Date.now() - hit < 250;
  useEffect(() => {
    if (!fresh) return;
    const t = window.setTimeout(() => tick((n) => n + 1), 260);
    return () => window.clearTimeout(t);
  }, [hit, fresh]);
  return held || fresh;
}

function sourceActive(source: string, pad: LivePad | undefined, kbm: LiveKbm | null): boolean {
  if (source.startsWith("key:")) return !!kbm?.keys.has(source.slice(4));
  if (source.startsWith("mouse:")) {
    const m = source.slice(6);
    return !!kbm && (kbm.mouse.has(m) || kbm.moves.has(m));
  }
  if (!pad) return false;
  if (isStick(source)) {
    const v = pad.axes[source.slice(0, 2)] ?? 0;
    return source.endsWith("-") ? v <= -STICK_LIT : v >= STICK_LIT;
  }
  return pad.buttons.has(source);
}

// ——— Manettes branchées et fréquences ———

/** Manettes branchées, dans l'ordre de branchement, tenues à jour. */
function useConnectedPads(): PadInfo[] {
  const [pads, setPads] = useState<PadInfo[]>([]);
  useEffect(() => {
    let live = true;
    listPads()
      .then((list) => live && setPads(list))
      .catch(() => undefined);
    const off = onPad((e) => {
      if (e.kind === "connected") {
        const { kind: _kind, ...info } = e;
        setPads((ps) => [...ps.filter((p) => p.pad !== info.pad), info]);
      } else if (e.kind === "disconnected") {
        setPads((ps) => ps.filter((p) => p.pad !== e.pad));
      }
    });
    return () => {
      live = false;
      off();
    };
  }, []);
  return pads;
}

const rateKey = (vendor: number, product: number) => `${vendor}:${product}`;

/**
 * Fréquence de chaque manette (Windows) : relevée toutes les 5 s tant que l'onglet est ouvert.
 * Une manette qu'on ne bouge pas, ou pendant une partie, garde sa dernière valeur.
 */
function usePadRates(): Map<string, PadRate> | null {
  // `null` : pas de mesure sur ce système (Linux, Mac).
  const [rates, setRates] = useState<Map<string, PadRate> | null>(new Map());
  useEffect(() => {
    let live = true;
    const poll = () =>
      backend
        .padRates()
        .then((list) => {
          if (!live) return;
          if (list === null) {
            setRates(null);
            window.clearInterval(timer);
            return;
          }
          if (list.length === 0) return;
          setRates((old) => {
            const next = new Map(old ?? []);
            for (const r of list) next.set(rateKey(r.vendor, r.product), r);
            return next;
          });
        })
        .catch(() => undefined);
    poll();
    const timer = window.setInterval(poll, RATE_POLL_MS);
    return () => {
      live = false;
      window.clearInterval(timer);
    };
  }, []);
  return rates;
}

const rateOf = (pad: PadInfo | undefined, rates: Map<string, PadRate> | null) =>
  pad && rates ? rates.get(rateKey(pad.vendor, pad.product)) : undefined;

const guidOf = (key: string) => key.split("/")[0];

/** Manette branchée qui correspond à ce choix : la même, sinon une du même modèle (comme au lancement). */
function findPad(want: PlayerPad | null, pads: PadInfo[]): PadInfo | undefined {
  if (!want) return undefined;
  return pads.find((p) => p.key === want.key) ?? pads.find((p) => p.guid === guidOf(want.key));
}

// ——— Joueurs ———

type Slot = { kind: "pad"; want: PlayerPad; pad: PadInfo | undefined } | { kind: "keyboard" } | null;

/** Ce qu'on déplace (glisser-déposer) ou qu'on a choisi (clavier, manette) : une manette, ou un joueur. */
type Item = { type: "device"; key: string } | { type: "slot"; index: number };

const sameItem = (a: Item | null, b: Item) =>
  !!a && a.type === b.type && (a.type === "device" ? a.key === (b as { key: string }).key : a.index === (b as { index: number }).index);

/** Entrée et Espace valident un élément `role="button"` (A à la manette passe par `click`). */
function pressable(onActivate: () => void) {
  return {
    role: "button",
    tabIndex: 0,
    onKeyDown: (e: ReactKeyboardEvent) => {
      if (e.target !== e.currentTarget) return;
      if (e.key === "Enter" || e.key === " ") {
        e.preventDefault();
        onActivate();
      }
    },
  };
}

/** Glisser-déposer à la souris (pointeur) : plus fiable que le glisser natif dans la fenêtre Tauri. */
function useDrag(onDrop: (item: Item, slot: number) => void) {
  const [ghost, setGhost] = useState<{ item: Item; label: string; x: number; y: number; over: number | null } | null>(null);
  const start = useRef<{ item: Item; label: string; x: number; y: number } | null>(null);
  const moved = useRef(false);
  const endedAt = useRef(0);
  const drop = useRef(onDrop);
  drop.current = onDrop;

  useEffect(() => {
    const slotAt = (x: number, y: number) => {
      const el = document.elementFromPoint(x, y)?.closest<HTMLElement>("[data-slot]");
      return el ? Number(el.dataset.slot) : null;
    };
    const move = (e: PointerEvent) => {
      const s = start.current;
      if (!s) return;
      if (!moved.current && Math.hypot(e.clientX - s.x, e.clientY - s.y) < 6) return;
      moved.current = true;
      setGhost({ item: s.item, label: s.label, x: e.clientX, y: e.clientY, over: slotAt(e.clientX, e.clientY) });
    };
    const up = (e: PointerEvent) => {
      const s = start.current;
      start.current = null;
      if (!s || !moved.current) return;
      moved.current = false;
      endedAt.current = Date.now();
      setGhost(null);
      const slot = slotAt(e.clientX, e.clientY);
      if (slot !== null) drop.current(s.item, slot);
    };
    window.addEventListener("pointermove", move);
    window.addEventListener("pointerup", up);
    window.addEventListener("pointercancel", up);
    return () => {
      window.removeEventListener("pointermove", move);
      window.removeEventListener("pointerup", up);
      window.removeEventListener("pointercancel", up);
    };
  }, []);

  return {
    ghost,
    /** À poser sur ce qu'on peut glisser. */
    handle: (item: Item, label: string) => ({
      onPointerDown: (e: ReactPointerEvent) => {
        if (e.pointerType !== "mouse" || e.button !== 0) return;
        if ((e.target as HTMLElement).closest("button")) return;
        start.current = { item, label, x: e.clientX, y: e.clientY };
        moved.current = false;
      },
    }),
    /** Le clic qui termine un glisser n'est pas un choix. */
    wasDrag: () => Date.now() - endedAt.current < 150,
  };
}

// ——— L'onglet ———

export function ControlsTab({ game, installs, onError }: Props) {
  useKeyboardLayout();
  const installed = installs.installed.get(game.id);
  const dos = !!installed?.pc && isDosbox(installed.executable);
  const installedHere = !!installed && !installed.pc;
  const [info, setInfo] = useState<ControlsInfo | null>(null);
  useEffect(() => {
    if (!installedHere) {
      setInfo(null);
      return;
    }
    let live = true;
    backend
      .controlsInfo(game.id)
      .then((i) => live && setInfo(i))
      .catch(() => live && setInfo(null));
    return () => {
      live = false;
    };
  }, [game.id, installedHere]);

  const pads = useConnectedPads();
  const rates = usePadRates();
  const live = useLive();
  const count = info?.ports ?? 0;
  // Boutons réglables ici : Dolphin, jeux GameCube.
  const remap = !!info?.layout && game.console === "ngc";
  const keyboardWhy = !installedHere
    ? null
    : !info?.layout
      ? "Ce jeu se joue à la manette : son émulateur ne reçoit pas le clavier et la souris du launcher."
      : !remap
        ? "Clavier et souris : seulement pour les jeux GameCube pour l'instant."
        : !info.keyboard
          ? "Clavier et souris : pas encore sur ce système."
          : null;
  const keyboardOk = remap && keyboardWhy === null;

  const [saved, setSaved] = useState(0);
  const [error, setError] = useState<string | null>(null);
  const done = () => {
    setError(null);
    setSaved(Date.now());
  };

  // Joueurs : choix enregistré, sinon ordre de branchement (le clavier au joueur 1 sans manette).
  const players = installed?.players ?? [];
  const chosen = players.some(Boolean);
  const slots: Slot[] = Array.from({ length: count }, (_, i) => {
    if (chosen) {
      const p = players[i];
      if (!p) return null;
      if (p.key === KEYBOARD_PLAYER) return keyboardOk ? { kind: "keyboard" } : null;
      return { kind: "pad", want: p, pad: findPad(p, pads) };
    }
    if (pads[i]) return { kind: "pad", want: { key: pads[i].key, name: pads[i].name }, pad: pads[i] };
    return i === 0 && pads.length === 0 && keyboardOk ? { kind: "keyboard" } : null;
  });

  async function savePlayers(next: (PlayerPad | null)[]) {
    try {
      installs.setInstalled(await backend.setPlayers(game.id, next));
      done();
    } catch (e) {
      setError(await onError(e));
    }
  }
  const asChoice = (s: Slot): PlayerPad | null => (s?.kind === "keyboard" ? { key: KEYBOARD_PLAYER, name: KEYBOARD_NAME } : s?.kind === "pad" ? s.want : null);
  const deviceKey = (s: Slot) => (s?.kind === "keyboard" ? KEYBOARD_PLAYER : s?.kind === "pad" ? (s.pad?.key ?? s.want.key) : null);

  function assign(index: number, key: string) {
    const pad = pads.find((p) => p.key === key);
    const choice: PlayerPad | null = key === KEYBOARD_PLAYER ? { key, name: KEYBOARD_NAME } : pad ? { key, name: pad.name } : null;
    if (!choice) return;
    // Une manette ne sert qu'à un joueur : elle quitte sa place précédente.
    const next = slots.map((s) => (deviceKey(s) === key ? null : asChoice(s)));
    next[index] = choice;
    ignored.current.delete(key);
    savePlayers(next);
  }
  function swap(a: number, b: number) {
    const next = slots.map(asChoice);
    [next[a], next[b]] = [next[b], next[a]];
    savePlayers(next);
  }
  const ignored = useRef(new Set<string>());
  function remove(index: number) {
    const key = deviceKey(slots[index]);
    // Retirée à la main : elle ne revient pas toute seule au prochain appui.
    if (key) ignored.current.add(key);
    const next = slots.map(asChoice);
    next[index] = null;
    savePlayers(next);
  }

  // Choix au clavier ou à la manette (A sur une manette, puis A sur un joueur).
  const [pick, setPick] = useState<Item | null>(null);
  function act(item: Item) {
    if (sameItem(pick, item)) return setPick(null);
    if (item.type === "slot") {
      if (pick?.type === "device") {
        setPick(null);
        return assign(item.index, pick.key);
      }
      if (pick?.type === "slot") {
        setPick(null);
        return swap(pick.index, item.index);
      }
    } else if (pick?.type === "slot") {
      setPick(null);
      return assign(pick.index, item.key);
    }
    setPick(item);
  }
  const drag = useDrag((item, slot) => {
    setPick(null);
    if (item.type === "device") assign(slot, item.key);
    else if (item.index !== slot) swap(item.index, slot);
  });
  useEffect(() => {
    if (!pick) return;
    const esc = (e: KeyboardEvent) => {
      if (e.key !== "Escape") return;
      e.preventDefault();
      e.stopPropagation();
      setPick(null);
    };
    window.addEventListener("keydown", esc, true);
    return () => window.removeEventListener("keydown", esc, true);
  }, [pick]);

  // Place des outils du tableau dans l'en-tête du bloc « Boutons ».
  const [buttonsHead, setButtonsHead] = useState<HTMLDivElement | null>(null);
  // Capture d'un bouton en cours (tableau « Boutons ») : rien d'autre ne réagit.
  const [capturing, setCapturing] = useState(false);

  // « Appuie sur un bouton pour rejoindre » : la première manette libre qui appuie prend la première place vide.
  const join = useRef<(pad: number) => void>(() => undefined);
  join.current = (padNum: number) => {
    if (capturing || pick || drag.ghost) return;
    const pad = pads.find((p) => p.pad === padNum);
    if (!pad || ignored.current.has(pad.key)) return;
    if (slots.some((s) => s?.kind === "pad" && s.pad?.pad === padNum)) return;
    const empty = slots.findIndex((s) => s === null);
    if (empty >= 0) assign(empty, pad.key);
  };
  useEffect(() => onPad((e) => e.kind === "button" && e.pressed && join.current(e.pad)), []);

  // Joueur affiché dans « Boutons » et « Réactivité » (ou l'appareil, quand le jeu n'attribue pas les joueurs).
  const [selected, setSelected] = useState(0);
  const devices: Slot[] = count > 0 ? slots : [...pads.map((p): Slot => ({ kind: "pad", want: { key: p.key, name: p.name }, pad: p })), { kind: "keyboard" }];
  const current = devices[Math.min(selected, Math.max(0, devices.length - 1))] ?? null;
  const currentPad = current?.kind === "pad" ? current.pad : undefined;

  // Flèches du clavier : d'un élément à l'autre, comme la croix de la manette.
  function onBoardKey(e: ReactKeyboardEvent) {
    if (capturing) return;
    const dir = ({ ArrowUp: "up", ArrowDown: "down", ArrowLeft: "left", ArrowRight: "right" } as Record<string, Dir>)[e.key];
    if (!dir || (e.target as HTMLElement).closest("input, textarea")) return;
    e.preventDefault();
    moveFocus(dir);
  }

  return (
    <div className="ctl-board" onKeyDown={onBoardKey}>
      <section className="ctl-block ctl-block--players" aria-labelledby="ctl-players-title">
        <h3 id="ctl-players-title" className="n-section-title">
          Joueurs
        </h3>
        <div className="ctl-block-body">
          {!installed ? (
            <p className="n-hint">Installez le jeu pour choisir la manette de chaque joueur.</p>
          ) : count === 0 ? (
            <p className="n-hint">Ce jeu choisit lui-même ses manettes : il prend celles qui sont branchées.</p>
          ) : (
            <ol className="ctl-cards">
              {slots.map((slot, i) => (
                <PlayerCard
                  key={i}
                  index={i}
                  slot={slot}
                  live={live}
                  rate={slot?.kind === "pad" ? rateOf(slot.pad, rates) : undefined}
                  picked={sameItem(pick, { type: "slot", index: i })}
                  target={!!pick && !sameItem(pick, { type: "slot", index: i })}
                  over={drag.ghost?.over === i}
                  drag={drag.handle({ type: "slot", index: i }, slot ? `Joueur ${i + 1}` : "")}
                  onActivate={() => !drag.wasDrag() && act({ type: "slot", index: i })}
                  onRemove={() => remove(i)}
                />
              ))}
            </ol>
          )}
          {pick && (
            <p className="n-hint" role="status">
              {pick.type === "device" ? "Choisissez maintenant le joueur." : "Choisissez une manette ou un autre joueur pour échanger."} Échap ou B pour
              annuler.
            </p>
          )}
          {error && <p className="n-alert">{error}</p>}
        </div>
      </section>

      <section className="ctl-block ctl-block--pads" aria-labelledby="ctl-pads-title">
        <h3 id="ctl-pads-title" className="n-section-title">
          Manettes détectées
        </h3>
        <div className="ctl-block-body">
          <ul className="ctl-devices">
            {pads.map((p) => {
              const at = slots.findIndex((s) => s?.kind === "pad" && s.pad?.pad === p.pad);
              const rate = rateOf(p, rates);
              const item: Item = { type: "device", key: p.key };
              return (
                <li
                  key={p.key}
                  className={`ctl-device${sameItem(pick, item) ? " is-picked" : ""}`}
                  {...(count > 0 ? pressable(() => act(item)) : {})}
                  {...(count > 0 ? drag.handle(item, p.name) : {})}
                  onClick={count > 0 ? () => !drag.wasDrag() && act(item) : undefined}
                  title={count > 0 ? "Glissez-la sur un joueur, ou validez puis choisissez le joueur" : undefined}
                >
                  <span className="ctl-device-name">
                    <Icon name="manette" /> {p.name}
                  </span>
                  <span className="ctl-device-meta">{rate ? (rate.bluetooth ? "Bluetooth" : "USB") : LAYOUT_NAMES[p.layout]}</span>
                  <Hz rate={rate} />
                  {count > 0 && <span className={`ctl-tag${at < 0 ? " ctl-tag--free" : ""}`}>{at < 0 ? "libre" : `J${at + 1}`}</span>}
                </li>
              );
            })}
            <KeyboardRow
              count={count}
              why={keyboardWhy}
              at={slots.findIndex((s) => s?.kind === "keyboard")}
              picked={sameItem(pick, { type: "device", key: KEYBOARD_PLAYER })}
              drag={drag.handle({ type: "device", key: KEYBOARD_PLAYER }, KEYBOARD_NAME)}
              onActivate={() => !drag.wasDrag() && act({ type: "device", key: KEYBOARD_PLAYER })}
            />
          </ul>
          {pads.length === 0 && <p className="n-hint">Aucune manette branchée. Branchez-en une : elle apparaît ici en quelques secondes.</p>}
        </div>
      </section>

      <section className="ctl-block ctl-block--buttons" aria-labelledby="ctl-buttons-title">
        <div className="ctl-block-head">
          <h3 id="ctl-buttons-title" className="n-section-title">
            Boutons
          </h3>
          <div className="ctl-head-tools" ref={setButtonsHead} />
        </div>
        <div className="ctl-block-body">
          {dos && installed ? (
            <KeyMap romId={game.id} binds={installed.pc?.keys ?? []} installs={installs} onError={onError} />
          ) : (
            <ButtonsTable
              romId={game.id}
              info={info}
              remap={remap}
              devices={devices}
              byPlayer={count > 0}
              selected={Math.min(selected, Math.max(0, devices.length - 1))}
              onSelect={setSelected}
              live={live}
              onInfo={(i) => {
                setInfo(i);
                done();
              }}
              onError={async (e) => setError(await onError(e))}
              capturing={capturing}
              onCapturing={setCapturing}
              head={buttonsHead}
            />
          )}
        </div>
      </section>

      <section className="ctl-block ctl-block--rate" aria-labelledby="ctl-rate-title">
        <h3 id="ctl-rate-title" className="n-section-title">
          Réactivité
        </h3>
        <div className="ctl-block-body">
          <Reactivity slot={current} rate={rateOf(currentPad, rates)} measured={rates !== null} />
        </div>
      </section>

      <section className="ctl-block ctl-block--profile" aria-labelledby="ctl-profile-title">
        <h3 id="ctl-profile-title" className="n-section-title">
          Profil
        </h3>
        <div className="ctl-block-body">
          <Profile
            romId={game.id}
            info={remap ? info : null}
            saved={saved}
            chosen={chosen}
            onRevert={() => savePlayers([])}
            onInfo={(i) => {
              setInfo(i);
              done();
            }}
            onError={async (e) => setError(await onError(e))}
          />
        </div>
      </section>

      {drag.ghost && (
        <div className="ctl-ghost" style={{ left: drag.ghost.x, top: drag.ghost.y }} aria-hidden="true">
          {drag.ghost.label}
        </div>
      )}
    </div>
  );
}

function Hz({ rate }: { rate: PadRate | undefined }) {
  if (!rate) return null;
  const ms = 1000 / rate.hz;
  return (
    <span className={`ctl-hz${rate.hz < SLOW_HZ ? " ctl-hz--slow" : ""}`} title={`Un état toutes les ${ms.toFixed(ms < 10 ? 1 : 0)} ms`}>
      {rate.hz} Hz
    </span>
  );
}

interface CardProps {
  index: number;
  slot: Slot;
  live: Live;
  rate: PadRate | undefined;
  picked: boolean;
  target: boolean;
  over: boolean;
  drag: { onPointerDown: (e: ReactPointerEvent) => void };
  onActivate: () => void;
  onRemove: () => void;
}

function PlayerCard({ index, slot, live, rate, picked, target, over, drag, onActivate, onRemove }: CardProps) {
  const pad = slot?.kind === "pad" && slot.pad ? live.pads.get(slot.pad.pad) : undefined;
  const kb = slot?.kind === "keyboard" ? live.kbm : null;
  const held = pad ? pad.buttons.size > 0 || Object.values(pad.axes).some((v) => Math.abs(v) >= STICK_LIT) : !!kb && (kb.keys.size > 0 || kb.mouse.size > 0);
  const lit = useBlink(pad?.hit ?? kb?.hit ?? 0, held);
  const name = slot?.kind === "keyboard" ? KEYBOARD_NAME : slot?.kind === "pad" ? (slot.pad ? slot.pad.name : `${slot.want.name} (non branchée)`) : null;
  const cls = ["ctl-card", slot ? "" : "ctl-card--empty", picked ? "is-picked" : "", target ? "is-target" : "", over ? "is-over" : ""].filter(Boolean).join(" ");
  return (
    <li className={cls} data-slot={index} {...pressable(onActivate)} {...drag} onClick={onActivate} aria-label={`Joueur ${index + 1} : ${name ?? "libre"}`}>
      <span className="ctl-card-num">J{index + 1}</span>
      {slot ? (
        <>
          <span className="ctl-card-pad">
            <span className={`ctl-card-icon${lit ? " is-lit" : ""}`}>
              <Icon name={slot.kind === "keyboard" ? "clavier" : "manette"} />
            </span>
            <span className={`ctl-card-name${slot.kind === "pad" && !slot.pad ? " ctl-off" : ""}`}>{name}</span>
          </span>
          {slot.kind === "pad" && <Hz rate={rate} />}
          <button
            className="ctl-card-x"
            title="Retirer"
            aria-label={`Retirer ${name} du joueur ${index + 1}`}
            onClick={(e) => {
              e.stopPropagation();
              onRemove();
            }}
          >
            ×
          </button>
        </>
      ) : (
        <span className="ctl-card-join">Appuie sur un bouton pour rejoindre</span>
      )}
    </li>
  );
}

function KeyboardRow(p: { count: number; why: string | null; at: number; picked: boolean; drag: { onPointerDown: (e: ReactPointerEvent) => void }; onActivate: () => void }) {
  if (p.count > 0 && p.why) {
    return (
      <li className="ctl-device ctl-device--off" aria-disabled="true" title={p.why}>
        <span className="ctl-device-name">
          <Icon name="clavier" /> {KEYBOARD_NAME}
        </span>
        <span className="ctl-device-why">{p.why}</span>
      </li>
    );
  }
  const assignable = p.count > 0;
  return (
    <li
      className={`ctl-device${p.picked ? " is-picked" : ""}`}
      {...(assignable ? pressable(p.onActivate) : {})}
      {...(assignable ? p.drag : {})}
      onClick={assignable ? p.onActivate : undefined}
    >
      <span className="ctl-device-name">
        <Icon name="clavier" /> {KEYBOARD_NAME}
      </span>
      <span className="ctl-device-meta" />
      <span />
      {assignable && <span className={`ctl-tag${p.at < 0 ? " ctl-tag--free" : ""}`}>{p.at < 0 ? "libre" : `J${p.at + 1}`}</span>}
    </li>
  );
}

// ——— Boutons ———

interface TableProps {
  romId: number;
  info: ControlsInfo | null;
  remap: boolean;
  devices: Slot[];
  byPlayer: boolean;
  selected: number;
  onSelect: (i: number) => void;
  live: Live;
  onInfo: (i: ControlsInfo) => void;
  onError: (e: unknown) => void;
  capturing: boolean;
  onCapturing: (on: boolean) => void;
  /** En-tête du bloc, où vont le choix du joueur et « Par défaut ». */
  head: HTMLElement | null;
}

/** Lignes du test quand les boutons se règlent dans l'émulateur : chaque bouton de la manette. */
const TEST_SOURCES = ["south", "east", "west", "north", "lb", "rb", "lt", "rt", "select", "start", "mode", "ls", "rs", "up", "down", "left", "right", ...Object.keys(AXIS_NAMES)];
const TEST_KEYS = ["mouse:left", "mouse:right", "mouse:middle", "mouse:x-", "mouse:x+", "mouse:y-", "mouse:y+"];

function ButtonsTable({ romId, info, remap, devices, byPlayer, selected, onSelect, live, onInfo, onError, capturing, onCapturing, head }: TableProps) {
  const slot = devices[selected] ?? null;
  const keys = slot?.kind === "keyboard";
  const padInfo = slot?.kind === "pad" ? slot.pad : undefined;
  const layout: PadLayout = padInfo?.layout ?? "generic";
  const livePad = padInfo ? live.pads.get(padInfo.pad) : undefined;
  const kbm = keys ? live.kbm : null;
  const [row, setRow] = useState<string | null>(null);
  const changeButtons = useRef(new Map<string, HTMLButtonElement>());

  const binds = info?.binds ?? [];
  const mine = binds[selected] ?? null;
  const sourceOf = (b: ConsoleButton) => (keys ? mine?.keys[b.id] : mine?.pad[b.id]) ?? (keys ? b.keys : b.pad);

  async function save(next: PlayerBinds) {
    const all = Array.from({ length: Math.max(binds.length, selected + 1) }, (_, i) => (i === selected ? next : (binds[i] ?? null)));
    try {
      onInfo(await backend.setControls(romId, all));
    } catch (e) {
      onError(e);
    }
  }

  function record(id: string, source: string) {
    if (!info) return;
    const side = keys ? "keys" : "pad";
    const next: PlayerBinds = { pad: { ...(mine?.pad ?? {}) }, keys: { ...(mine?.keys ?? {}) } };
    // Source déjà prise par un autre bouton : les deux échangent leur place.
    const before = info.buttons.find((b) => b.id === id);
    const other = info.buttons.find((b) => b.id !== id && sourceOf(b) === source);
    if (before && other) next[side][other.id] = sourceOf(before);
    next[side][id] = source;
    for (const b of info.buttons) if (next[side][b.id] === (keys ? b.keys : b.pad)) delete next[side][b.id];
    save(next);
  }

  // Capture du prochain bouton, de la prochaine touche ou du prochain clic.
  useEffect(() => {
    if (!row) return;
    onCapturing(true);
    let finished = false;
    const stop = (source: string | null) => {
      if (finished) return;
      finished = true;
      setRow(null);
      if (source) record(row, source);
      window.setTimeout(() => changeButtons.current.get(row)?.focus({ preventScroll: true }), 0);
    };
    const offs: (() => void)[] = [];
    const later: (() => void)[] = [];
    const listen = <K extends keyof WindowEventMap>(type: K, fn: (e: WindowEventMap[K]) => void, list = offs) => {
      window.addEventListener(type, fn, true);
      list.push(() => window.removeEventListener(type, fn, true));
    };
    listen("keydown", (e) => {
      e.preventDefault();
      e.stopPropagation();
      if (e.repeat) return;
      if (e.key === "Escape") return stop(null);
      if (keys && KNOWN_CODE.test(e.code)) stop(`key:${e.code}`);
    });
    if (keys) {
      // Le clic choisi ne doit rien déclencher d'autre dans la page.
      const swallow = (e: Event) => {
        e.preventDefault();
        e.stopPropagation();
      };
      listen("mousedown", (e) => {
        swallow(e);
        // Clic sur « Annuler » de la ligne : annule.
        if ((e.target as Element | null)?.closest?.("[data-cancel]")) return stop(null);
        const m = e.button === 0 ? "left" : e.button === 1 ? "middle" : e.button === 2 ? "right" : null;
        if (m) stop(`mouse:${m}`);
      });
      listen("contextmenu", swallow, later);
      listen("click", swallow, later);
      listen("auxclick", swallow, later);
      return () => {
        offs.forEach((off) => off());
        onCapturing(false);
        // Le relâchement du clic suit : il ne doit pas cliquer ce qui est dessous.
        window.setTimeout(() => later.forEach((off) => off()), 400);
      };
    }
    const release = capturePad();
    const back = confirmButtons(layout).back;
    let holding: number | undefined;
    const offPad = onPad((e) => {
      if (padInfo && e.pad !== padInfo.pad) return;
      if (e.kind === "button") {
        if (e.button === back) {
          // B : bouton comme un autre si on le lâche vite, annulation si on le maintient.
          if (e.pressed) holding = window.setTimeout(() => stop(null), CANCEL_HOLD_MS);
          else if (holding !== undefined) {
            window.clearTimeout(holding);
            stop(back);
          }
          return;
        }
        if (e.pressed) stop(e.button);
      } else if (e.kind === "axis" && Math.abs(e.value) >= STICK_PUSHED) {
        stop(`${e.axis}${e.value < 0 ? "-" : "+"}`);
      }
    });
    return () => {
      offPad();
      window.clearTimeout(holding);
      offs.forEach((off) => off());
      onCapturing(false);
      // Laisse passer le relâchement du bouton avant de reprendre la navigation.
      window.setTimeout(release, 300);
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [row]);

  // Joueur changé : capture abandonnée.
  useEffect(() => setRow(null), [selected]);

  const selector = devices.length > 0 && (
    <div className="ctl-select" role="group" aria-label={byPlayer ? "Joueur affiché" : "Appareil affiché"}>
      {devices.map((d, i) => (
        <button
          key={i}
          className="n-btn n-btn--sm ctl-mini"
          aria-pressed={i === selected}
          disabled={capturing}
          title={byPlayer ? undefined : d?.kind === "keyboard" ? KEYBOARD_NAME : d?.kind === "pad" ? d.want.name : undefined}
          onClick={() => onSelect(i)}
        >
          {byPlayer ? `J${i + 1}` : d?.kind === "keyboard" ? "Clavier" : `Manette ${i + 1}`}
        </button>
      ))}
    </div>
  );

  const deviceName = keys ? KEYBOARD_NAME : padInfo ? padInfo.name : slot?.kind === "pad" ? `${slot.want.name} (non branchée)` : "Aucune manette";

  if (!remap || !info) {
    // Test seulement : les boutons se règlent dans l'émulateur (ou le jeu).
    const rows = keys ? [...[...live.kbm.keys].map((c) => `key:${c}`), ...TEST_KEYS] : TEST_SOURCES;
    return (
      <div className="ctl-buttons">
        {head && selector && createPortal(selector, head)}
        <p className="n-hint">
          {info?.emulator
            ? `Les boutons de ce jeu se règlent dans ${info.emulator}. Ce tableau sert de test : chaque ligne s'allume quand vous appuyez.`
            : "Ce tableau sert de test : chaque ligne s'allume quand vous appuyez."}
        </p>
        <div className="ctl-table-cols">
          {halves(rows).map((half, n) => (
            <table key={n} className="ctl-table">
              <thead>
                <tr>
                  <th>{keys ? "Touche ou souris" : deviceName}</th>
                </tr>
              </thead>
              <tbody>
                {half.map((s) => (
                  <tr key={s} className={sourceActive(s, livePad, kbm) ? "is-on" : undefined}>
                    <td>{sourceName(s, layout)}</td>
                  </tr>
                ))}
              </tbody>
            </table>
          ))}
        </div>
        {keys && <p className="n-hint">Appuyez sur des touches : elles s'ajoutent au tableau.</p>}
      </div>
    );
  }

  return (
    <div className="ctl-buttons">
      {head &&
        createPortal(
          <>
            {selector}
            <button
              className="n-btn n-btn--sm ctl-mini"
              disabled={capturing || !mine || (Object.keys(mine.pad).length === 0 && Object.keys(mine.keys).length === 0)}
              onClick={() => save({ pad: {}, keys: {} })}
            >
              Par défaut
            </button>
          </>,
          head,
        )}
      {!slot && <p className="n-hint">Aucune manette pour ce joueur : réglage de la manette qu'il aura.</p>}
      <div className="ctl-table-cols">
        {buttonColumns(info.buttons).map((half, n) => (
          <table key={n} className="ctl-table">
            <thead>
              <tr>
                <th>GameCube</th>
                <th className="ctl-table-device">{deviceName}</th>
                <th aria-label="Action" />
              </tr>
            </thead>
            <tbody>
              {half.map((b) => {
                const source = sourceOf(b);
                const waiting = row === b.id;
                return (
                  <tr key={b.id} className={waiting ? "is-waiting" : sourceActive(source, livePad, kbm) ? "is-on" : undefined}>
                    <td className="ctl-table-console">{b.label}</td>
                    <td>
                      {waiting ? (
                        <span className="ctl-waiting" role="status">
                          {keys ? "Touche ou clic…" : "Appuie sur un bouton…"}
                        </span>
                      ) : (
                        sourceName(source, layout)
                      )}
                    </td>
                    <td className="ctl-table-action">
                      <button
                        ref={(el) => {
                          if (el) changeButtons.current.set(b.id, el);
                          else changeButtons.current.delete(b.id);
                        }}
                        className="n-link"
                        data-cancel={waiting ? "" : undefined}
                        disabled={capturing && !waiting}
                        onClick={() => setRow(waiting ? null : b.id)}
                      >
                        {waiting ? "Annuler" : "Changer"}
                      </button>
                    </td>
                  </tr>
                );
              })}
            </tbody>
          </table>
        ))}
      </div>
      {row && <p className="n-hint">{keys ? "Échap pour annuler." : "Maintiens B ou appuie sur Échap pour annuler."}</p>}
    </div>
  );
}

/** Deux moitiés d'une liste (tableau sur deux colonnes). */
function halves<T>(list: T[]): T[][] {
  const cut = Math.ceil(list.length / 2);
  return [list.slice(0, cut), list.slice(cut)].filter((h) => h.length > 0);
}

/** GameCube : boutons et croix à gauche, gâchettes et sticks à droite. */
const LEFT_COLUMN = new Set(["a", "b", "x", "y", "z", "start", "d-up", "d-down", "d-left", "d-right"]);
function buttonColumns(buttons: ConsoleButton[]): ConsoleButton[][] {
  const left = buttons.filter((b) => LEFT_COLUMN.has(b.id));
  if (left.length === 0 || left.length === buttons.length) return halves(buttons);
  return [left, buttons.filter((b) => !LEFT_COLUMN.has(b.id))];
}

// ——— Réactivité ———

function Reactivity({ slot, rate, measured }: { slot: Slot; rate: PadRate | undefined; measured: boolean }) {
  const [open, setOpen] = useState(false);
  if (!slot || (slot.kind === "pad" && !slot.pad)) return <p className="n-hint">Aucune manette branchée pour ce joueur.</p>;
  if (slot.kind === "keyboard") return <p className="n-hint">{KEYBOARD_NAME} : fréquence non mesurée.</p>;
  if (!measured) return <p className="n-hint">La fréquence se mesure sous Windows seulement.</p>;
  if (!rate) {
    return (
      <div className="ctl-rate">
        <span className="ctl-rate-value">— Hz</span>
        <p className="n-hint">Bouge un stick pour mesurer la fréquence de la manette.</p>
      </div>
    );
  }
  const ms = 1000 / rate.hz;
  const slow = rate.hz < SLOW_HZ;
  return (
    <div className="ctl-rate">
      <span className={`ctl-rate-value${slow ? " ctl-hz--slow" : ""}`}>{rate.hz} Hz</span>
      <span className="ctl-gauge" role="meter" aria-valuemin={0} aria-valuemax={1000} aria-valuenow={Math.min(rate.hz, 1000)} aria-label="Fréquence sur 1000 Hz">
        <span style={{ width: `${Math.min(100, rate.hz / 10)}%` }} />
      </span>
      <p className="ctl-rate-line">
        Un état toutes les {ms.toFixed(ms < 10 ? 1 : 0)} ms{rate.bluetooth ? " · Bluetooth" : ""}
      </p>
      {slow && (
        <>
          <button className="n-link" aria-expanded={open} onClick={() => setOpen(!open)}>
            Comment débrider ?
          </button>
          {open && (
            <p className="n-hint ctl-explain" role="note">
              {rate.bluetooth
                ? "En Bluetooth, la fréquence dépend de la manette et de l'adaptateur, pas du port USB. Branchée avec un câble, elle répond en général plus vite."
                : "Windows lit beaucoup de manettes USB à 125 Hz. Un outil comme hidusbf force le port à 1000 Hz, mais il installe un pilote non signé : droits administrateur, souvent le mode test de Windows, que certains anti-triche refusent. À faire en connaissance de cause ; l'outil sait retirer son pilote."}
            </p>
          )}
        </>
      )}
    </div>
  );
}

// ——— Profil ———

const SCOPES: { scope: ControlsScope; label: (emulator: string | null) => string }[] = [
  { scope: "jeu", label: () => "Ce jeu uniquement" },
  { scope: "emulateur", label: (e) => (e ? `Tous les jeux de ${e}` : "Tous les jeux du même émulateur") },
  { scope: "tous", label: () => "Tous les jeux" },
];

function Profile(p: {
  romId: number;
  info: ControlsInfo | null;
  saved: number;
  chosen: boolean;
  onRevert: () => void;
  onInfo: (i: ControlsInfo) => void;
  onError: (e: unknown) => void;
}) {
  const [, tick] = useState(0);
  const recent = p.saved > 0 && Date.now() - p.saved < 2500;
  useEffect(() => {
    if (!recent) return;
    const t = window.setTimeout(() => tick((n) => n + 1), 2600);
    return () => window.clearTimeout(t);
  }, [p.saved, recent]);
  async function choose(scope: ControlsScope) {
    try {
      p.onInfo(await backend.setControlsScope(p.romId, scope));
    } catch (e) {
      p.onError(e);
    }
  }
  return (
    <div className="ctl-profile">
      <div className="ctl-scopes" role="group" aria-label="Les boutons réglés valent pour">
        {SCOPES.map((s) => (
          <button
            key={s.scope}
            className="n-btn n-btn--sm"
            aria-pressed={p.info?.scope === s.scope}
            disabled={!p.info}
            onClick={() => p.info?.scope !== s.scope && choose(s.scope)}
          >
            {s.label(p.info?.emulator ?? null)}
          </button>
        ))}
      </div>
      {!p.info && <p className="n-hint">Les boutons de ce jeu se règlent dans son émulateur.</p>}
      <div className="ctl-profile-foot">
        {p.chosen && (
          <button className="n-link" onClick={p.onRevert}>
            Revenir à l'ordre de branchement
          </button>
        )}
        <span className={`ctl-saved${recent ? " is-on" : ""}`} role="status">
          {recent ? "Enregistré" : ""}
        </span>
      </div>
    </div>
  );
}

type Step = { kind: "target" } | { kind: "source"; target: string; targetLabel: string };

function KeyMap({ romId, binds, installs, onError }: { romId: number; binds: KeyBind[]; installs: InstallsState; onError: (e: unknown) => Promise<string> }) {
  const [step, setStep] = useState<Step | null>(null);
  const [error, setError] = useState<string | null>(null);

  async function save(next: KeyBind[]) {
    setError(null);
    try {
      installs.setInstalled(await backend.setKeyBinds(romId, next));
    } catch (e) {
      setError(await onError(e));
    }
  }

  // Capture : la touche du jeu, puis la touche ou le bouton du joueur.
  useEffect(() => {
    if (!step) return;
    const finish = (source: string, sourceLabel: string) => {
      if (step.kind !== "source") return;
      setStep(null);
      const rest = binds.filter((b) => b.source !== source);
      save([...rest, { target: step.target, source, targetLabel: step.targetLabel, sourceLabel }]);
    };
    const onKey = (e: KeyboardEvent) => {
      e.preventDefault();
      e.stopPropagation();
      if (e.repeat) return;
      const dos = DOS_KEYS[e.code];
      if (!dos) return;
      if (step.kind === "target") setStep({ kind: "source", target: `key_${dos[0]}`, targetLabel: keyLabel(e) });
      else finish(`key ${dos[1]}`, keyLabel(e));
    };
    window.addEventListener("keydown", onKey, true);
    // Manette : seulement comme touche du joueur.
    const release = step.kind === "source" ? capturePad() : null;
    const offPad =
      step.kind === "source"
        ? onPad((e) => {
            if (e.kind === "button" && e.pressed && PAD_BINDS[e.button]) finish(PAD_BINDS[e.button], padLabel(padLayoutOf(e.pad), e.button));
            if (e.kind === "axis" && Math.abs(e.value) > 0.7) {
              const [neg, pos, negLabel, posLabel] = STICK_BINDS[e.axis];
              finish(e.value < 0 ? neg : pos, e.value < 0 ? negLabel : posLabel);
            }
          })
        : null;
    return () => {
      window.removeEventListener("keydown", onKey, true);
      offPad?.();
      // Laisse passer le relâchement du bouton avant de reprendre la navigation.
      if (release) window.setTimeout(release, 250);
    };
  }, [step]);

  return (
    <div className="ctl-map">
      {binds.length === 0 && !step && <p className="n-hint">Touches d'origine du jeu.</p>}
      {binds.length > 0 && (
        <ul className="ctl-binds">
          {binds.map((b) => (
            <li key={b.source}>
              <kbd className="ctl-key">{b.sourceLabel ?? b.source}</kbd>
              <span className="ctl-arrow">→</span>
              <span>
                touche <kbd className="ctl-key">{b.targetLabel ?? b.target.replace(/^key_/, "")}</kbd> du jeu
              </span>
              <button className="n-link" onClick={() => save(binds.filter((x) => x !== b))}>
                Retirer
              </button>
            </li>
          ))}
        </ul>
      )}
      {step ? (
        <div className="n-notice ctl-capture">
          <p>
            {step.kind === "target"
              ? "Appuyez sur la touche du jeu à remplacer (par exemple la flèche ↑ pour avancer)."
              : `Appuyez sur la touche ou le bouton de manette qui fera « ${step.targetLabel} ».`}
          </p>
          <button className="n-btn" onClick={() => setStep(null)}>
            Annuler
          </button>
        </div>
      ) : (
        <div className="ctl-actions">
          <button className="n-btn" onClick={() => setStep({ kind: "target" })}>
            Ajouter une touche
          </button>
          {binds.length > 0 && (
            <button className="n-btn" onClick={() => save([])}>
              Touches d'origine
            </button>
          )}
        </div>
      )}
      {error && <p className="n-alert">{error}</p>}
      <p className="n-hint">
        Appliqué au prochain lancement. Une touche remplacée ne garde pas son rôle d'origine. En jeu, Ctrl+F1 ouvre aussi l'éditeur de
        touches de DOSBox.
      </p>
    </div>
  );
}
