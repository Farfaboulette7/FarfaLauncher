// « Rafraîchir » : tout ce qui se lit sur le serveur se relit (bibliothèque, catalogue, amis,
// messages, page du jeu ouverte…). Les composants qui gardent leurs propres données écoutent cet
// évènement.
export const REFRESH_EVENT = "farfa-refresh";

export function announceRefresh() {
  window.dispatchEvent(new Event(REFRESH_EVENT));
}
