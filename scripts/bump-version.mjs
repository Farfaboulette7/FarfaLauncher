// Met la même version dans package.json, src-tauri/Cargo.toml et
// src-tauri/tauri.conf.json (le workflow de release vérifie qu'elles
// correspondent au tag).
//
// Usage : node scripts/bump-version.mjs 0.2.0
import { readFileSync, writeFileSync } from "node:fs";

const version = process.argv[2];
if (!/^\d+\.\d+\.\d+$/.test(version ?? "")) {
  console.error("Usage : node scripts/bump-version.mjs X.Y.Z");
  process.exit(1);
}

function update(path, transform) {
  const before = readFileSync(path, "utf8");
  const after = transform(before);
  if (after === before) throw new Error(`Version introuvable dans ${path}`);
  writeFileSync(path, after);
  console.log(`${path} → ${version}`);
}

const jsonVersion = (text) => text.replace(/("version"\s*:\s*")[^"]+(")/, `$1${version}$2`);
update("package.json", jsonVersion);
update("src-tauri/tauri.conf.json", jsonVersion);
update("src-tauri/Cargo.toml", (text) => text.replace(/^(version\s*=\s*")[^"]+(")/m, `$1${version}$2`));

console.log(`\nEnsuite :\n  git commit -am "chore: version ${version}"\n  git tag v${version}\n  git push && git push origin v${version}`);
