// Bruitages de l'interface, téléchargés depuis le serveur RomM (voir
// src-tauri/src/commands/sounds.rs). Sans fichier, `play` ne fait rien.
import { convertFileSrc } from "@tauri-apps/api/core";
import { backend } from "./api/backend";

export type SoundName =
  | "OpeningMenu"
  | "Click"
  | "deplacementFocus"
  | "deplacementOption"
  | "DownloadEnd"
  | "LancementGame"
  | "retourSelection"
  | "Error";

const SETTINGS_KEY = "farfa.sounds";
// Lus par Web Audio, comme la musique : sous Linux, un <audio> ne lit pas
// les fichiers locaux servis par le launcher (protocole asset).
const sources = new Map<SoundName, AudioBuffer>();
let ctx: AudioContext | null = null;
const lastPlayed = new Map<SoundName, number>();
let loading: Promise<void> | null = null;
/** Son demandé avant le chargement (ex. ouverture) : joué dès qu'il arrive. */
let pending: { name: SoundName; at: number } | null = null;

/** Niveau maximal (curseur à 100 %) : des bruitages discrets. */
const BASE_LEVEL = 0.5;

interface SoundSettings {
  enabled: boolean;
  volume: number;
}

function readSettings(): SoundSettings {
  try {
    const s = JSON.parse(localStorage.getItem(SETTINGS_KEY) ?? "{}");
    return { enabled: s.enabled !== false, volume: typeof s.volume === "number" ? s.volume : 0.3 };
  } catch {
    return { enabled: true, volume: 0.3 };
  }
}

let settings = readSettings();

export function soundSettings(): SoundSettings {
  return settings;
}

export function setSoundSettings(next: Partial<SoundSettings>) {
  settings = { ...settings, ...next };
  try {
    localStorage.setItem(SETTINGS_KEY, JSON.stringify(settings));
  } catch {
    // préférence non mémorisée : sans gravité
  }
  // Bruitages réactivés : on les charge s'ils ne l'étaient pas.
  if (settings.enabled) void loadSounds();
}

/**
 * Charge les sons (une fois connecté). Sans effet s'ils sont déjà là, ou s'ils
 * sont coupés : aucun lecteur audio n'est alors créé.
 */
export function loadSounds(): Promise<void> {
  if (!settings.enabled) return Promise.resolve();
  loading ??= backend
    .soundPaths()
    .then(async (paths) => {
      ctx ??= new AudioContext();
      const c = ctx;
      await Promise.all(
        Object.entries(paths).map(async ([name, path]) => {
          try {
            const resp = await fetch(convertFileSrc(path));
            sources.set(name as SoundName, await c.decodeAudioData(await resp.arrayBuffer()));
          } catch (e) {
            console.warn("son illisible", name, e);
          }
        }),
      );
      resumeOnGesture(c);
      if (pending && Date.now() - pending.at < 4000) play(pending.name);
      pending = null;
    })
    .catch((e) => {
      loading = null;
      console.warn("sons indisponibles", e);
    });
  return loading;
}

/** Recharge les sons (après un changement d'adresse du serveur des assets). */
export function reloadSounds(): Promise<void> {
  loading = null;
  sources.clear();
  return loadSounds();
}

export function play(name: SoundName) {
  if (!settings.enabled) return;
  const src = sources.get(name);
  if (!src) {
    if (name === "OpeningMenu") pending = { name, at: Date.now() };
    return;
  }
  // Pas de rafale du même son (survols rapides, double déclenchement). « Error »
  // part à la fois de `errorMessage` et de l'affichage de l'alerte : un seul son.
  const now = performance.now();
  if (now - (lastPlayed.get(name) ?? -1e9) < (name === "Error" ? 600 : 60)) return;
  lastPlayed.set(name, now);
  if (!ctx || ctx.state !== "running") return;
  const gain = ctx.createGain();
  gain.gain.value = settings.volume * BASE_LEVEL;
  gain.connect(ctx.destination);
  const node = ctx.createBufferSource();
  node.buffer = src;
  node.connect(gain);
  node.onended = () => gain.disconnect();
  node.start();
}

/** Le moteur peut démarrer l'audio en pause : reprise au premier geste. */
function resumeOnGesture(c: AudioContext) {
  if (c.state !== "suspended") return;
  const resume = () => {
    c.resume().catch(() => undefined);
    window.removeEventListener("pointerdown", resume, true);
    window.removeEventListener("keydown", resume, true);
  };
  window.addEventListener("pointerdown", resume, true);
  window.addEventListener("keydown", resume, true);
}

/**
 * Sons de navigation, branchés une fois pour toute l'application :
 * - clic sur un bouton → « Click » (ou le son de `data-sound`, « none » = muet) ;
 * - survol / focus d'un jeu (`data-sound-hover="game"`) → « deplacementFocus » ;
 * - survol / focus d'un autre bouton → « deplacementOption » ;
 * - apparition d'un message d'erreur (`.n-alert`, sauf `data-sound="none"`) → « Error ».
 */
export function installNavigationSounds() {
  new MutationObserver((records) => {
    for (const r of records) {
      for (const node of r.addedNodes) {
        if (!(node instanceof Element)) continue;
        const alert = node.matches(".n-alert") ? node : node.querySelector(".n-alert");
        if (alert && (alert as HTMLElement).dataset.sound !== "none") return play("Error");
      }
    }
  }).observe(document.body, { childList: true, subtree: true });

  const target = (e: Event) =>
    (e.target as Element | null)?.closest?.("button, [role='tab'], a, input[type='checkbox'], select") as HTMLElement | null;
  let lastHover: Element | null = null;

  document.addEventListener(
    "click",
    (e) => {
      const el = target(e);
      if (!el || (el as HTMLButtonElement).disabled) return;
      const custom = el.dataset.sound;
      if (custom === "none") return;
      play((custom as SoundName) || "Click");
    },
    true,
  );

  const onHover = (e: Event) => {
    const el = target(e);
    if (!el || el === lastHover || (el as HTMLButtonElement).disabled) return;
    lastHover = el;
    play(el.dataset.soundHover === "game" ? "deplacementFocus" : "deplacementOption");
  };
  document.addEventListener("mouseover", onHover, true);
  document.addEventListener("focusin", onHover, true);
  document.addEventListener(
    "mouseout",
    (e) => {
      if (lastHover && !lastHover.contains((e as MouseEvent).relatedTarget as Node | null)) lastHover = null;
    },
    true,
  );
}
