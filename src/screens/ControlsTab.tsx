import { CSSProperties, useEffect, useState } from "react";
import { backend, Game, KeyBind, PadEvent, PadLayout } from "../api/backend";
import { InstallsState } from "../hooks/useInstalls";
import { capturePad, listPads, onPad, padLayoutOf } from "../gamepad";
import { faceColor, LAYOUT_NAMES, padLabel } from "../padLayouts";

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

export function ControlsTab({ game, installs, onError }: Props) {
  const installed = installs.installed.get(game.id);
  const dos = !!installed?.pc && isDosbox(installed.executable);
  return (
    <div className="gd-controls">
      <section>
        <h3 className="n-section-title">Tester le clavier et la manette</h3>
        <Tester />
      </section>
      {dos && installed && (
        <section>
          <h3 className="n-section-title">Touches du jeu</h3>
          <KeyMap romId={game.id} binds={installed.pc?.keys ?? []} installs={installs} onError={onError} />
        </section>
      )}
    </div>
  );
}

interface PadState {
  name: string;
  layout: PadLayout;
  buttons: Set<string>;
  axes: Record<string, number>;
}

/** Durée d'appui sur Start + Select pour débloquer le menu. */
const UNLOCK_HOLD_MS = 1000;

function Tester() {
  const [keys, setKeys] = useState<{ code: string; label: string }[]>([]);
  const [last, setLast] = useState<{ code: string; label: string } | null>(null);
  const [pads, setPads] = useState(new Map<number, PadState>());
  const [locked, setLocked] = useState(false);

  useEffect(() => {
    const down = (e: KeyboardEvent) => {
      if (e.repeat) return;
      if (e.key === "Escape") {
        // Menu bloqué : Échap débloque au lieu de revenir en arrière.
        if (locked) {
          e.preventDefault();
          e.stopPropagation();
          setLocked(false);
        }
        return;
      }
      const k = { code: e.code, label: keyLabel(e) };
      setLast(k);
      setKeys((ks) => (ks.some((x) => x.code === k.code) ? ks : [...ks, k]));
    };
    const up = (e: KeyboardEvent) => setKeys((ks) => ks.filter((x) => x.code !== e.code));
    const clear = () => setKeys([]);
    window.addEventListener("keydown", down, true);
    window.addEventListener("keyup", up);
    window.addEventListener("blur", clear);
    return () => {
      window.removeEventListener("keydown", down, true);
      window.removeEventListener("keyup", up);
      window.removeEventListener("blur", clear);
    };
  }, [locked]);

  // Menu bloqué : la manette ne fait plus bouger le launcher.
  useEffect(() => {
    if (!locked) return;
    const release = capturePad();
    return () => {
      // Laisse passer le relâchement des boutons avant de reprendre la navigation.
      window.setTimeout(release, 300);
    };
  }, [locked]);

  // Start + Select maintenus : déblocage.
  const unlockCombo = [...pads.values()].some((p) => p.buttons.has("start") && p.buttons.has("select"));
  useEffect(() => {
    if (!locked || !unlockCombo) return;
    const t = window.setTimeout(() => setLocked(false), UNLOCK_HOLD_MS);
    return () => window.clearTimeout(t);
  }, [locked, unlockCombo]);

  useEffect(() => {
    const blank = (name: string, layout: PadLayout): PadState => ({ name, layout, buttons: new Set(), axes: {} });
    listPads()
      .then((list) => setPads((m) => new Map([...list.map((p) => [p.pad, m.get(p.pad) ?? blank(p.name, p.layout)] as const), ...m])))
      .catch(() => undefined);
    return onPad((e: PadEvent) =>
      setPads((m) => {
        const next = new Map(m);
        if (e.kind === "disconnected") {
          next.delete(e.pad);
          return next;
        }
        if (e.kind === "connected") {
          next.set(e.pad, blank(e.name, e.layout));
          return next;
        }
        const pad = next.get(e.pad) ?? blank("Manette", padLayoutOf(e.pad));
        const copy: PadState = { ...pad, buttons: new Set(pad.buttons), axes: { ...pad.axes } };
        if (e.kind === "button") {
          if (e.pressed) copy.buttons.add(e.button);
          else copy.buttons.delete(e.button);
        } else {
          copy.axes[e.axis] = e.value;
        }
        next.set(e.pad, copy);
        return next;
      }),
    );
  }, []);

  const first = [...pads.values()][0];
  const comboNames = first ? `${padLabel(first.layout, "start")} + ${padLabel(first.layout, "select")}` : "Start + Select";

  return (
    <div className="ctl-tester">
      <div className={`ctl-lock${locked ? " n-notice" : ""}`}>
        {locked ? (
          <>
            <p>Menu bloqué : testez tous les boutons. Pour débloquer, maintenez {comboNames} ou appuyez sur Échap.</p>
            <button className="n-btn" onClick={() => setLocked(false)}>
              Débloquer
            </button>
          </>
        ) : (
          <>
            <p className="n-hint">Appuyez sur une touche ou un bouton pour vérifier qu'il est bien reconnu.</p>
            <button className="n-btn" onClick={() => setLocked(true)}>
              Bloquer le menu pour tester la manette
            </button>
          </>
        )}
      </div>
      <div className="ctl-keyboard">
        <span className="ctl-label">Clavier</span>
        <div className="ctl-keys">
          {keys.length === 0 && <span className="n-hint">Aucune touche enfoncée</span>}
          {keys.map((k) => (
            <kbd key={k.code} className="ctl-key ctl-key--on">
              {k.label}
            </kbd>
          ))}
        </div>
        {last && (
          <p className="n-hint">
            Dernière touche : {last.label} <span className="ctl-code">({last.code})</span>
          </p>
        )}
      </div>
      {pads.size === 0 && <p className="n-hint">Aucune manette détectée. Branchez-en une : elle apparaît ici en quelques secondes.</p>}
      {[...pads.entries()].map(([id, pad]) => (
        <div key={id} className="ctl-pad">
          <span className="ctl-label">
            {pad.name} · {LAYOUT_NAMES[pad.layout]}
          </span>
          <PadButtons pad={pad} />
        </div>
      ))}
    </div>
  );
}

/** Boutons de la manette, nommés et colorés selon sa famille ; allumés quand ils sont pressés. */
function PadButtons({ pad }: { pad: PadState }) {
  const chip = (b: string) => {
    const color = faceColor(pad.layout, b);
    return (
      <kbd
        key={b}
        className={`ctl-key${pad.buttons.has(b) ? " ctl-key--on" : ""}`}
        style={color ? ({ "--ctl-face": color } as CSSProperties) : undefined}
        data-face={color ? "" : undefined}
      >
        {padLabel(pad.layout, b)}
      </kbd>
    );
  };
  const group = (title: string, buttons: string[]) => (
    <div className="ctl-group">
      <span className="ctl-group-title">{title}</span>
      <div className="ctl-keys">{buttons.map(chip)}</div>
    </div>
  );
  return (
    <div className="ctl-pad-body">
      {group("Gâchettes", ["lt", "lb", "rb", "rt"])}
      {group("Boutons", ["south", "east", "west", "north"])}
      {group("Croix", ["up", "down", "left", "right"])}
      {group("Centre", ["select", "mode", "start"])}
      <div className="ctl-group">
        <span className="ctl-group-title">Sticks</span>
        <div className="ctl-keys">
          <Stick x={pad.axes.lx ?? 0} y={pad.axes.ly ?? 0} pressed={pad.buttons.has("ls")} label={padLabel(pad.layout, "ls")} />
          <Stick x={pad.axes.rx ?? 0} y={pad.axes.ry ?? 0} pressed={pad.buttons.has("rs")} label={padLabel(pad.layout, "rs")} />
        </div>
      </div>
    </div>
  );
}

/** Position d'un stick (point dans un carré) ; le carré s'allume quand le stick est enfoncé. */
function Stick({ x, y, pressed, label }: { x: number; y: number; pressed: boolean; label: string }) {
  return (
    <span className={`ctl-stick${pressed ? " ctl-stick--on" : ""}`} title={label} aria-label={label}>
      <span style={{ left: `${50 + x * 40}%`, top: `${50 + y * 40}%` }} />
      <em>{label}</em>
    </span>
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
