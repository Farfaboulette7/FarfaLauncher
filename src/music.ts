// Musique de fond, téléchargée depuis le serveur des assets (voir
// src-tauri/src/commands/music.rs). Version normale dans la bibliothèque,
// version 8-bit sur la fiche d'un jeu.
//
// Les deux versions sont décodées en mémoire (Web Audio) pour des boucles sans
// blanc. Au changement, la version cible repart à la position équivalente dans
// sa propre boucle, puis les deux se croisent en fondu : les morceaux restent
// calés même quand leurs intros n'ont pas la même durée.
import { convertFileSrc } from "@tauri-apps/api/core";
import { backend, MusicTrack, MusicVariant } from "./api/backend";
import { logToFile } from "./components/ErrorBoundary";

export type MusicVersion = "normal" | "eightBit";

const SETTINGS_KEY = "farfa.music";
const LAST_TRACK_KEY = "farfa.music.last";
const CROSSFADE = 0.8; // secondes
const MUTE_FADE = 1.5;
const TRACK_FADE = 1.5;

interface MusicSettings {
  enabled: boolean;
  volume: number;
}

interface Playing {
  version: MusicVersion;
  source: AudioBufferSourceNode;
  gain: GainNode;
  /** Instant (horloge audio) correspondant à la position 0 + `offset`. */
  startedAt: number;
  offset: number;
}

function readSettings(): MusicSettings {
  try {
    const s = JSON.parse(localStorage.getItem(SETTINGS_KEY) ?? "{}");
    return { enabled: s.enabled !== false, volume: typeof s.volume === "number" ? s.volume : 0.25 };
  } catch {
    return { enabled: true, volume: 0.25 };
  }
}

let settings = readSettings();
let ctx: AudioContext | null = null;
let master: GainNode | null = null;
let track: MusicTrack | null = null;
let buffers: Record<MusicVersion, AudioBuffer> | null = null;
let playing: Playing | null = null;
let wanted: MusicVersion = "normal";
/** Coupée pendant qu'un jeu tourne. */
let muted = false;
let loading: Promise<void> | null = null;
/** Incrémenté à l'arrêt : un chargement en cours devenu obsolète s'abandonne. */
let generation = 0;

export function musicSettings(): MusicSettings {
  return settings;
}

export function setMusicSettings(next: Partial<MusicSettings>) {
  settings = { ...settings, ...next };
  try {
    localStorage.setItem(SETTINGS_KEY, JSON.stringify(settings));
  } catch {
    // préférence non mémorisée : sans gravité
  }
  applyVolume(0.2);
  setState({ enabled: settings.enabled });
  if (settings.enabled) void loadMusic();
}

function targetVolume() {
  return settings.enabled && !muted ? settings.volume : 0;
}

function applyVolume(fade: number) {
  if (!ctx || !master) return;
  const now = ctx.currentTime;
  master.gain.cancelScheduledValues(now);
  master.gain.setValueAtTime(master.gain.value, now);
  master.gain.linearRampToValueAtTime(targetVolume(), now + fade);
}

function variant(t: MusicTrack, v: MusicVersion): MusicVariant {
  return v === "normal" ? t.normal : t.eightBit;
}

/** Position actuelle (secondes) dans le fichier, boucles comprises. */
function position(p: Playing, v: MusicVariant, now: number): number {
  const t = p.offset + (now - p.startedAt);
  if (t < v.loopEnd) return t;
  const len = v.loopEnd - v.loopStart;
  return v.loopStart + ((t - v.loopStart) % len);
}

/**
 * Position équivalente dans l'autre version : même avancée dans la boucle ;
 * pendant l'intro, même distance avant le début de la boucle.
 */
export function mapPosition(t: number, from: MusicVariant, to: MusicVariant): number {
  if (t < from.loopStart) return Math.max(0, to.loopStart - (from.loopStart - t));
  const p = (t - from.loopStart) % (from.loopEnd - from.loopStart);
  return to.loopStart + (p % (to.loopEnd - to.loopStart));
}

function startVersion(version: MusicVersion, offset: number, fadeIn: number): Playing {
  const c = ctx!;
  const v = variant(track!, version);
  const buffer = buffers![version];
  const source = c.createBufferSource();
  source.buffer = buffer;
  source.loop = true;
  source.loopStart = v.loopStart;
  source.loopEnd = Math.min(v.loopEnd, buffer.duration);
  const gain = c.createGain();
  const now = c.currentTime;
  gain.gain.setValueAtTime(fadeIn > 0 ? 0 : 1, now);
  if (fadeIn > 0) gain.gain.linearRampToValueAtTime(1, now + fadeIn);
  source.connect(gain).connect(master!);
  source.start(now, offset);
  return { version, source, gain, startedAt: now, offset };
}

function fadeOutAndStop(p: Playing, fade: number) {
  const now = ctx!.currentTime;
  p.gain.gain.cancelScheduledValues(now);
  p.gain.gain.setValueAtTime(p.gain.gain.value, now);
  p.gain.gain.linearRampToValueAtTime(0, now + fade);
  p.source.stop(now + fade + 0.05);
}

/** Bascule entre version normale et 8-bit, calée sur le même moment du morceau. */
export function setMusicVersion(version: MusicVersion) {
  wanted = version;
  if (!ctx || !track || !buffers || !playing || playing.version === version) return;
  const now = ctx.currentTime;
  const from = variant(track, playing.version);
  const offset = mapPosition(position(playing, from, now), from, variant(track, version));
  fadeOutAndStop(playing, CROSSFADE);
  playing = startVersion(version, offset, CROSSFADE);
}

/** Coupe la musique en fondu (ex. pendant qu'un jeu tourne), puis la reprend. */
export function setMusicMuted(value: boolean) {
  if (muted === value) return;
  muted = value;
  applyVolume(MUTE_FADE);
}

/** Morceau en cours pour toute la session : une reconnexion ne retélécharge rien. */
let sessionTrackId: string | null = null;

/** Tire un morceau, sauf `exclude` (et sauf le dernier joué au lancement précédent). */
function pickTrack(tracks: MusicTrack[], exclude: string | null): MusicTrack {
  const pool = tracks.length > 1 ? tracks.filter((t) => t.id !== exclude) : tracks;
  const chosen = pool[Math.floor(Math.random() * pool.length)];
  rememberTrack(chosen);
  return chosen;
}

function rememberTrack(t: MusicTrack) {
  sessionTrackId = t.id;
  try {
    localStorage.setItem(LAST_TRACK_KEY, t.id);
  } catch {
    // sans mémoire du dernier morceau : tirage libre au prochain lancement
  }
}

function lastTrackId(): string | null {
  try {
    return localStorage.getItem(LAST_TRACK_KEY);
  } catch {
    return null;
  }
}

async function decode(c: AudioContext, path: string): Promise<AudioBuffer> {
  const resp = await fetch(convertFileSrc(path));
  if (!resp.ok) throw new Error(`lecture du fichier local impossible (${resp.status})`);
  return c.decodeAudioData(await resp.arrayBuffer());
}

/** Télécharge et décode les deux versions d'un morceau. */
async function loadPair(c: AudioContext, t: MusicTrack): Promise<Record<MusicVersion, AudioBuffer>> {
  const files = await backend.musicFiles(t);
  const [normal, eightBit] = await Promise.all([decode(c, files.normal), decode(c, files.eightBit)]);
  return { normal, eightBit };
}

function logFailure(e: unknown) {
  console.warn("musique indisponible", e);
  logToFile("warn", `musique indisponible: ${e instanceof Error ? e.message : JSON.stringify(e)}`);
}

/** Le WebView peut démarrer l'audio en pause : reprise au premier geste. */
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

// --- État lisible par l'interface (useSyncExternalStore) ---------------------

export interface MusicState {
  /** Titre du morceau en cours ; null si aucune musique. */
  title: string | null;
  /** Musique activée dans les paramètres. */
  enabled: boolean;
  /** Changement de morceau en cours (téléchargement, décodage). */
  switching: boolean;
}

let state: MusicState = { title: null, switching: false, enabled: settings.enabled };
const listeners = new Set<() => void>();

function setState(next: Partial<MusicState>) {
  state = { ...state, ...next };
  listeners.forEach((l) => l());
}

export function subscribeMusic(listener: () => void): () => void {
  listeners.add(listener);
  return () => listeners.delete(listener);
}

export function musicState(): MusicState {
  return state;
}

// --- Chargement, changement de morceau, arrêt --------------------------------

/** Choisit un morceau, le télécharge et le lance (une fois connecté). */
export function loadMusic(): Promise<void> {
  if (!settings.enabled) return Promise.resolve();
  const gen = generation;
  loading ??= (async () => {
    const tracks = await backend.musicTracks();
    if (tracks.length === 0) return;
    const chosen = tracks.find((t) => t.id === sessionTrackId) ?? pickTrack(tracks, lastTrackId());
    const c = new AudioContext();
    const pair = await loadPair(c, chosen);
    if (gen !== generation) {
      c.close().catch(() => undefined);
      return;
    }
    ctx = c;
    master = c.createGain();
    master.gain.setValueAtTime(0, c.currentTime);
    master.connect(c.destination);
    track = chosen;
    buffers = pair;
    playing = startVersion(wanted, 0, 0);
    applyVolume(1.5);
    setState({ title: chosen.title });
    logToFile("info", `musique : « ${chosen.title} » (audio ${c.state})`);
    resumeOnGesture(c);
  })().catch((e) => {
    loading = null;
    logFailure(e);
  });
  return loading;
}

/**
 * Passe à un autre morceau tiré au sort : l'ancien s'efface pendant que le
 * nouveau monte, dans la version du moment (8-bit sur la fiche d'un jeu).
 */
export async function nextTrack(): Promise<void> {
  if (!ctx || !track || state.switching) return;
  const gen = generation;
  const c = ctx;
  setState({ switching: true });
  try {
    const tracks = await backend.musicTracks();
    if (tracks.length < 2) return;
    const chosen = pickTrack(tracks, track.id);
    const pair = await loadPair(c, chosen);
    if (gen !== generation || ctx !== c) return;
    if (playing) fadeOutAndStop(playing, TRACK_FADE);
    track = chosen;
    buffers = pair;
    playing = startVersion(wanted, 0, TRACK_FADE);
    setState({ title: chosen.title });
    logToFile("info", `musique : « ${chosen.title} »`);
  } catch (e) {
    logFailure(e);
  } finally {
    setState({ switching: false });
  }
}

/** Arrête tout (déconnexion, changement d'adresse des assets). */
export function stopMusic() {
  generation++;
  loading = null;
  playing = null;
  buffers = null;
  track = null;
  master = null;
  ctx?.close().catch(() => undefined);
  ctx = null;
  setState({ title: null, switching: false });
}

export function reloadMusic(): Promise<void> {
  stopMusic();
  return loadMusic();
}
