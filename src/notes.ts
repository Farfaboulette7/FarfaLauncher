/** Notes de version (Markdown) en lignes simples : titres, puces, texte sans balises. */
export type NoteLine = { kind: "title" | "item" | "text"; text: string };

export function noteLines(md: string): NoteLine[] {
  const clean = (s: string) =>
    s
      .replace(/!\[[^\]]*\]\([^)]*\)/g, "")
      .replace(/\[([^\]]*)\]\([^)]*\)/g, "$1")
      .replace(/<[^>]+>/g, "")
      .replace(/(\*\*|__|`)/g, "")
      // Émojis : le moteur web de l'AppImage se fige en les dessinant sous Linux.
      .replace(/[\p{Extended_Pictographic}\u{1F3FB}-\u{1F3FF}\u{FE0F}\u{200D}\u{20E3}]/gu, "")
      .trim();
  // Les notes sont souvent coupées à ~120 caractères : une ligne qui n'ouvre ni titre, ni puce,
  // ni paragraphe (après une ligne vide) prolonge la précédente.
  const out: NoteLine[] = [];
  let open = false;
  for (const line of md.split(/\r?\n/)) {
    if (line.trim() === "") {
      open = false;
      continue;
    }
    const title = /^\s*#{1,6}\s+(.*)$/.exec(line);
    const item = /^\s*(?:[-*+]|\d+\.)\s+(.*)$/.exec(line);
    if (title) {
      out.push({ kind: "title", text: clean(title[1]) });
      open = false;
    } else if (item) {
      out.push({ kind: "item", text: clean(item[1]) });
      open = true;
    } else if (open && out.length > 0) {
      out[out.length - 1].text = `${out[out.length - 1].text} ${clean(line)}`.trim();
    } else {
      out.push({ kind: "text", text: clean(line) });
      open = true;
    }
  }
  return out.filter((l) => l.text !== "" && !/^[-=*_]{3,}$/.test(l.text));
}

/** Notes du launcher embarquées à la compilation (`docs/changelog/vX.Y.Z.md`), la plus récente d'abord. */
const LAUNCHER_NOTES = import.meta.glob("../docs/changelog/v*.md", { query: "?raw", import: "default", eager: true }) as Record<string, string>;

function versionOf(path: string): number[] {
  return (/v(\d+)\.(\d+)\.(\d+)\.md$/.exec(path) ?? []).slice(1).map(Number);
}

/** Versions du launcher jusqu'à `current` comprise, avec leurs notes (sans la ligne de téléchargement). */
export function launcherNotes(current: string, max = 5): { version: string; lines: NoteLine[] }[] {
  const cur = current.split(".").map(Number);
  const cmp = (a: number[], b: number[]) => a[0] - b[0] || a[1] - b[1] || a[2] - b[2];
  return Object.entries(LAUNCHER_NOTES)
    .map(([path, md]) => ({ v: versionOf(path), md }))
    .filter((n) => n.v.length === 3 && cmp(n.v, cur) <= 0)
    .sort((a, b) => cmp(b.v, a.v))
    .slice(0, max)
    .map((n) => ({
      version: n.v.join("."),
      lines: noteLines(n.md.split(/\r?\n/).filter((l) => !l.startsWith("Téléchargez")).join("\n")),
    }));
}
