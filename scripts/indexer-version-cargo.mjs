// Prépare le commit de src-tauri/Cargo.toml en n'y mettant QUE le changement de version.
//
// Pourquoi : sur le PC du propriétaire, src-tauri/Cargo.toml a un réglage local ([profile.dev])
// qui ne doit jamais être commité. Un simple « git add » le commiterait. Ce script prend
// Cargo.toml tel qu'il est dans le dernier commit (HEAD), y change seulement la ligne
// « version = … » du paquet, et met ce contenu dans l'index de git (ce qui sera commité).
// Le fichier sur le disque n'est pas touché : le réglage local y reste.
//
// Utilisé par scripts/publier.ps1. Usage : node scripts/indexer-version-cargo.mjs 1.2.2
import { execFileSync } from "node:child_process";

const version = process.argv[2];
if (!/^\d+\.\d+\.\d+(-beta\.\d+)?$/.test(version ?? "")) {
  console.error("Usage : node scripts/indexer-version-cargo.mjs X.Y.Z");
  process.exit(1);
}

const chemin = "src-tauri/Cargo.toml";
// Contenu exact du dernier commit (octets bruts : fins de ligne du dépôt conservées).
const avant = execFileSync("git", ["show", `HEAD:${chemin}`]).toString("utf8");
// Première ligne « version = "…" » en début de ligne : celle de [package] (comme bump-version.mjs).
const apres = avant.replace(/^(version\s*=\s*")[^"]+(")/m, `$1${version}$2`);
if (apres === avant) {
  console.error(`Version introuvable (ou déjà ${version}) dans ${chemin} du dernier commit`);
  process.exit(1);
}
// Écrit le contenu dans la base de git, puis le place dans l'index à la place de Cargo.toml.
const blob = execFileSync("git", ["hash-object", "-w", "--stdin"], { input: Buffer.from(apres, "utf8") })
  .toString()
  .trim();
const mode = execFileSync("git", ["ls-files", "-s", "--", chemin]).toString().split(" ")[0] || "100644";
execFileSync("git", ["update-index", "--cacheinfo", `${mode},${blob},${chemin}`]);
console.log(`${chemin} : seule la version ${version} est prête à être commitée (réglages locaux laissés de côté).`);
