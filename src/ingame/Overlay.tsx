// Colonne du menu en jeu (fenêtre « ingame », voir src-tauri/src/ingame.rs) :
// en haut la durée de la partie, l'heure et le jeu ; en dessous les actions,
// pilotées à la manette (seule cette fenêtre reçoit la manette tant qu'elle est ouverte).
import { useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { GAMEPAD_EVENT, PadEvent, PadLayout } from "../api/backend";
import { confirmButtons, padLabel } from "../padLayouts";
import "./Overlay.css";

interface MenuInfo {
  romId: number;
  name: string;
  platform: string | null;
  sessionMs: number;
  quitting: boolean;
}

type Action = "resume" | "quit";
const ACTIONS: { id: Action; label: string }[] = [
  { id: "resume", label: "Reprendre" },
  { id: "quit", label: "Quitter le jeu" },
];

/** « 12:04 » ou « 1:02:09 ». */
function duration(ms: number): string {
  const s = Math.floor(ms / 1000);
  const two = (n: number) => String(n).padStart(2, "0");
  const h = Math.floor(s / 3600);
  return h > 0 ? `${h}:${two(Math.floor(s / 60) % 60)}:${two(s % 60)}` : `${Math.floor(s / 60)}:${two(s % 60)}`;
}

export function Overlay() {
  const [menu, setMenu] = useState<(MenuInfo & { at: number }) | null>(null);
  const [selected, setSelected] = useState(0);
  const [now, setNow] = useState(() => new Date());
  const [layout, setLayout] = useState<PadLayout>("xbox");
  const stickDir = useRef(0);

  useEffect(() => {
    const show = (info: MenuInfo | null) => {
      setMenu((m) => (info ? { ...info, at: m && m.romId === info.romId && info.quitting ? m.at : Date.now() } : null));
      if (info && !info.quitting) setSelected(0);
    };
    invoke<MenuInfo | null>("ingame_state").then(show).catch(() => undefined);
    const off = listen<MenuInfo | null>("ingame-menu", ({ payload }) => show(payload));
    const tick = window.setInterval(() => setNow(new Date()), 1000);
    return () => {
      off.then((f) => f());
      window.clearInterval(tick);
    };
  }, []);

  const run = (action: Action) => {
    if (!menu || menu.quitting) return;
    if (action === "resume") invoke("ingame_resume").catch(() => undefined);
    else invoke("quit_game", { romId: menu.romId }).catch(() => undefined);
  };

  // Manette : haut/bas pour choisir, validation pour agir, retour ou Start pour reprendre.
  const state = useRef({ menu, selected, layout, run });
  state.current = { menu, selected, layout, run };
  useEffect(() => {
    const move = (step: number) => setSelected((i) => (i + step + ACTIONS.length) % ACTIONS.length);
    const off = listen<PadEvent>(GAMEPAD_EVENT, ({ payload: e }) => {
      const { menu, selected, layout, run } = state.current;
      if (e.kind === "layout") {
        setLayout(e.layout);
        return;
      }
      if (!menu || menu.quitting) return;
      if (e.kind === "axis" && e.axis === "ly") {
        const dir = e.value > 0.6 ? 1 : e.value < -0.6 ? -1 : Math.abs(e.value) < 0.4 ? 0 : stickDir.current;
        if (dir !== stickDir.current && dir !== 0) move(dir);
        stickDir.current = dir;
        return;
      }
      if (e.kind !== "button" || !e.pressed) return;
      const { confirm, back } = confirmButtons(layout);
      if (e.button === "up") move(-1);
      else if (e.button === "down") move(1);
      else if (e.button === confirm) run(ACTIONS[selected].id);
      else if (e.button === back || e.button === "start") run("resume");
    });
    return () => {
      off.then((f) => f());
    };
  }, []);

  if (!menu) return null;
  const session = menu.sessionMs + (now.getTime() - menu.at);
  const { confirm, back } = confirmButtons(layout);

  return (
    <div className="igo" key={menu.at}>
      <header className="igo-head">
        <p className="igo-session">{duration(session)}</p>
        <p className="igo-label">Partie en cours</p>
        <p className="igo-clock">
          {now.toLocaleTimeString("fr-FR", { hour: "2-digit", minute: "2-digit" })}
          <span> · {now.toLocaleDateString("fr-FR", { weekday: "long", day: "numeric", month: "long" })}</span>
        </p>
        <div className="n-rule igo-rule" />
        <h1 className="igo-name">{menu.name}</h1>
        {menu.platform && <p className="igo-platform">{menu.platform}</p>}
      </header>

      {menu.quitting ? (
        <p className="igo-quitting">
          <span className="n-spinner" /> Fermeture du jeu…
        </p>
      ) : (
        <ul className="igo-actions">
          {ACTIONS.map((a, i) => (
            <li key={a.id}>
              <button className={`igo-action${i === selected ? " igo-action--on" : ""}`} onMouseEnter={() => setSelected(i)} onClick={() => run(a.id)}>
                {a.label}
              </button>
            </li>
          ))}
        </ul>
      )}

      {!menu.quitting && (
        <footer className="igo-hints">
          <span>
            <kbd>{padLabel(layout, confirm)}</kbd> Valider
          </span>
          <span>
            <kbd>{padLabel(layout, back)}</kbd> Reprendre
          </span>
        </footer>
      )}
    </div>
  );
}
