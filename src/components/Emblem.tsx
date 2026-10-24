import emblem from "../assets/yorha-emblem.png";

const MASK = `url("${emblem}")`;

/** Emblème YoRHa, dessiné dans la couleur du texte courant (masque CSS). */
export function Emblem({ className }: { className?: string }) {
  return (
    <span
      className={`emblem ${className ?? ""}`}
      style={{ WebkitMaskImage: MASK, maskImage: MASK }}
      aria-hidden="true"
    />
  );
}
