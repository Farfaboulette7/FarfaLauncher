// Réglages d'affichage appliqués sur <html> : fond animé (désactivable dans
// les Paramètres) et effets allégés sous Linux, où le moteur web (WebKitGTK)
// peine sur les flous plein écran.
const MOTION_KEY = "farfa.backdrop.motion";

export function backdropMotion(): boolean {
  try {
    return localStorage.getItem(MOTION_KEY) !== "off";
  } catch {
    return true;
  }
}

export function setBackdropMotion(on: boolean) {
  try {
    localStorage.setItem(MOTION_KEY, on ? "on" : "off");
  } catch {
    // préférence non mémorisée : sans gravité
  }
  document.documentElement.classList.toggle("no-backdrop-motion", !on);
}

/** À appeler au démarrage. */
export function applyDisplaySettings() {
  const root = document.documentElement;
  root.classList.toggle("no-backdrop-motion", !backdropMotion());
  root.classList.toggle("fx-lite", /Linux/.test(navigator.userAgent) && !/Android/.test(navigator.userAgent));
}
