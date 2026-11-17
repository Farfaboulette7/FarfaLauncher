// Filet de sécurité pour le son. Sous Linux, un GStreamer incompatible peut figer
// tout le moteur web dès qu'un son est préparé : l'appli devient inutilisable et
// rien n'arrive jusqu'aux logs. On note le début de l'initialisation du son ; si
// le lancement suivant trouve cette marque, c'est que le précédent s'est figé :
// le son démarre alors coupé (réactivable dans les Paramètres).
import { logToFile } from "./components/ErrorBoundary";
import { setMusicSettings } from "./music";
import { setSoundSettings } from "./sounds";

/** Linux de bureau (AppImage), où le moteur web passe par GStreamer. */
const LINUX = /Linux/.test(navigator.userAgent) && !/Android/.test(navigator.userAgent);
const PROBE_KEY = "farfa.audio.probe";
const NOTICE_KEY = "farfa.audio.autoDisabled";
/** Délai au bout duquel l'initialisation est jugée réussie (le moteur répond encore). */
const OK_AFTER_MS = 2500;

function read(key: string): string | null {
  try {
    return localStorage.getItem(key);
  } catch {
    return null;
  }
}

function write(key: string, value: string | null) {
  try {
    if (value === null) localStorage.removeItem(key);
    else localStorage.setItem(key, value);
  } catch {
    // stockage indisponible : pas de filet, sans gravité
  }
}

/**
 * À appeler juste avant de charger sons et musique. Renvoie `false` si le son
 * vient d'être coupé parce que le lancement précédent s'est figé.
 */
export function beginAudio(): boolean {
  // Le blocage ne vient que de GStreamer, sous Linux. Ailleurs, la marque restait aussi quand
  // le launcher était fermé pendant les premières secondes (version de test…) et coupait le
  // son à tort : on la retire, et un son coupé ainsi est remis.
  if (!LINUX) {
    write(PROBE_KEY, null);
    if (read(NOTICE_KEY) === "1") {
      write(NOTICE_KEY, null);
      setSoundSettings({ enabled: true });
      setMusicSettings({ enabled: true });
      logToFile("info", "son coupé à tort par le filet de Linux : réactivé");
    }
    return true;
  }
  if (read(PROBE_KEY) === "start") {
    write(PROBE_KEY, null);
    write(NOTICE_KEY, "1");
    setSoundSettings({ enabled: false });
    setMusicSettings({ enabled: false });
    logToFile("warn", "le lancement précédent s'est figé pendant l'initialisation du son : son coupé");
    return false;
  }
  write(PROBE_KEY, "start");
  window.setTimeout(() => write(PROBE_KEY, null), OK_AFTER_MS);
  return true;
}

/** Le son a été coupé automatiquement (message à afficher dans les Paramètres). */
export function audioAutoDisabled(): boolean {
  return read(NOTICE_KEY) === "1";
}

/** L'utilisateur a réactivé le son : on oublie le message. */
export function clearAudioNotice() {
  write(NOTICE_KEY, null);
}
