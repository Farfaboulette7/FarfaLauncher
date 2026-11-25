// Icônes du launcher (src/icons/*.svg), posées telles quelles dans la page : `fill="currentColor"`
// leur donne la couleur du texte autour. Taille : 1 em, carrée, alignée sur le texte (.n-icon).
import clavier from "../icons/clavier.svg?raw";
import manette from "../icons/manette.svg?raw";

const ICONS = { manette, clavier } as const;

export type IconName = keyof typeof ICONS;

export function Icon({ name }: { name: IconName }) {
  return <span className="n-icon" aria-hidden="true" dangerouslySetInnerHTML={{ __html: ICONS[name] }} />;
}
