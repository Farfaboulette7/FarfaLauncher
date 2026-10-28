// Noms des boutons selon la famille de manette (comme les glyphes de Steam).
// Les boutons sont désignés par leur position (`south` = bouton du bas).
import { PadLayout } from "./api/backend";

const XBOX: Record<string, string> = {
  south: "A", east: "B", west: "X", north: "Y", lb: "LB", rb: "RB", lt: "LT", rt: "RT",
  select: "View", start: "Menu", mode: "Xbox", ls: "LS", rs: "RS",
};
const PLAYSTATION: Record<string, string> = {
  south: "✕", east: "○", west: "□", north: "△", lb: "L1", rb: "R1", lt: "L2", rt: "R2",
  select: "Create", start: "Options", mode: "PS", ls: "L3", rs: "R3",
};
const NINTENDO: Record<string, string> = {
  south: "B", east: "A", west: "Y", north: "X", lb: "L", rb: "R", lt: "ZL", rt: "ZR",
  select: "−", start: "+", mode: "Home", ls: "LS", rs: "RS",
};
const DPAD: Record<string, string> = { up: "Croix ↑", down: "Croix ↓", left: "Croix ←", right: "Croix →" };

export const PAD_BUTTONS = ["south", "east", "west", "north", "lb", "rb", "lt", "rt", "select", "start", "mode", "ls", "rs", "up", "down", "left", "right"];

export function padLabel(layout: PadLayout, button: string): string {
  const names = layout === "playstation" ? PLAYSTATION : layout === "nintendo" ? NINTENDO : XBOX;
  return names[button] ?? DPAD[button] ?? button;
}

export const LAYOUT_NAMES: Record<PadLayout, string> = {
  xbox: "Xbox",
  playstation: "PlayStation",
  nintendo: "Nintendo",
  generic: "Manette",
};

/** Boutons valider / retour : chez Nintendo, A (à droite) valide, comme dans Steam. */
export function confirmButtons(layout: PadLayout): { confirm: string; back: string } {
  return layout === "nintendo" ? { confirm: "east", back: "south" } : { confirm: "south", back: "east" };
}

// Couleurs des boutons de façade (Xbox, PlayStation) ; neutres ailleurs.
const FACE_COLORS: Partial<Record<PadLayout, Record<string, string>>> = {
  xbox: { south: "#4f9e3a", east: "#c9353d", west: "#2f6fc4", north: "#c99a12" },
  playstation: { south: "#5577c9", east: "#c94a55", west: "#b06aa6", north: "#2f9c86" },
};

export function faceColor(layout: PadLayout, button: string): string | null {
  return FACE_COLORS[layout]?.[button] ?? null;
}
