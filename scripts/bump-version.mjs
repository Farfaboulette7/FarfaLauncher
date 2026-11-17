// Met la même version dans package.json, src-tauri/Cargo.toml et
// src-tauri/tauri.conf.json (le workflow de release vérifie qu'elles
// correspondent au tag).
//
// Usage : node scripts/bump-version.mjs 0.2.0          (version stable)
//         node scripts/bump-version.mjs 1.0.1-beta.1   (version de test, canal bêta seulement)
import { readFileSync, writeFileSync } from "node:fs";

const version = process.argv[2];
if (!/^\d+\.\d+\.\d+(-beta\.\d+)?$/.test(version ?? "")) {
  console.error("Usage : node scripts/bump-version.mjs X.Y.Z (stable) ou X.Y.Z-beta.N (test)");
  process.exit(1);
}
console.log(version.includes("-") ? "Version de test : canal bêta seulement.\n" : "Version stable : tous les launchers.\n");

function update(path, transform) {
  const before = readFileSync(path, "utf8");
  const after = transform(before);
  if (after === before && !before.includes(version)) throw new Error(`Version introuvable dans ${path}`);
  writeFileSync(path, after);
  console.log(`${path} → ${version}`);
}

const jsonVersion = (text) => text.replace(/("version"\s*:\s*")[^"]+(")/, `$1${version}$2`);
update("package.json", jsonVersion);
update("src-tauri/tauri.conf.json", jsonVersion);
update("src-tauri/Cargo.toml", (text) => text.replace(/^(version\s*=\s*")[^"]+(")/m, `$1${version}$2`));
// Fichiers de verrouillage : version du paquet racine uniquement.
update("package-lock.json", (text) => {
  const lock = JSON.parse(text);
  lock.version = version;
  if (lock.packages?.[""]) lock.packages[""].version = version;
  return JSON.stringify(lock, null, 2) + "\n";
});
update("src-tauri/Cargo.lock", (text) =>
  text.replace(/(\[\[package\]\]\r?\nname = "farfalauncher"\r?\nversion = ")[^"]+(")/, `$1${version}$2`),
);

console.log(`\nEnsuite :\n  git commit -am "chore: version ${version}"\n  git tag v${version}\n  git push && git push origin v${version}`);
