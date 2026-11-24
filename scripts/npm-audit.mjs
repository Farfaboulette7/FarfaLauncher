// Failles connues dans les paquets npm de l'interface (CI : .github/workflows/securite.yml).
//
// Lance « npm audit » sur les dépendances embarquées dans le launcher (pas les outils de
// développement : --omit=dev), puis trie ce qu'il trouve :
//   - gravité « high » ou « critical » : ERREUR, le script finit en échec (code 1) ;
//   - gravité « moderate » ou « low »   : simple avertissement, le script réussit ;
//   - faille listée dans securite/failles-ignorees.txt : rappelée, mais jamais bloquante.
//
// Pas besoin de node_modules : npm audit lit seulement package-lock.json.
// En local : node scripts/npm-audit.mjs
// Explications et marche à suivre en cas d'échec : docs/tests.md, « Contrôles de sécurité ».

import { spawnSync } from 'node:child_process';
import { readFileSync } from 'node:fs';

// 1. Failles ignorées : une par ligne, l'identifiant GHSA-… en début de ligne (le reste de la
//    ligne est un commentaire : date et raison).
const ignorees = new Set(
  readFileSync(new URL('../securite/failles-ignorees.txt', import.meta.url), 'utf8')
    .split('\n')
    .map((ligne) => ligne.trim().split(/\s+/)[0])
    .filter((id) => id.startsWith('GHSA-')),
);

// 2. Rapport de npm audit au format JSON. npm audit finit en code 1 dès qu'il trouve quelque
//    chose : on ne regarde donc que son rapport. shell: true est nécessaire sous Windows.
const npm = spawnSync('npm', ['audit', '--omit=dev', '--json'], {
  encoding: 'utf8',
  shell: process.platform === 'win32',
});
let rapport;
try {
  rapport = JSON.parse(npm.stdout);
} catch {
  console.error("npm audit n'a pas donné de rapport lisible (réseau ?) :\n" + npm.stdout + npm.stderr);
  process.exit(2);
}
if (rapport.error) {
  console.error('npm audit a échoué : ' + JSON.stringify(rapport.error));
  process.exit(2);
}

// 3. Une ligne par faille (un même avis peut toucher plusieurs paquets : on ne le compte
//    qu'une fois). Dans « via », les objets sont les avis eux-mêmes ; les simples noms
//    renvoient à un autre paquet vulnérable, déjà listé de son côté.
const vues = new Map();
for (const paquet of Object.values(rapport.vulnerabilities ?? {})) {
  for (const avis of paquet.via) {
    if (typeof avis !== 'object') continue;
    const id = String(avis.url ?? avis.source).split('/').pop();
    vues.set(id, { id, gravite: avis.severity, paquet: avis.name, titre: avis.title, versions: avis.range });
  }
}

let bloquantes = 0;
for (const f of vues.values()) {
  const texte = `${f.id} [${f.gravite}] ${f.paquet} ${f.versions} : ${f.titre}`;
  if (ignorees.has(f.id)) {
    console.log(`ignorée (securite/failles-ignorees.txt) : ${texte}`);
  } else if (f.gravite === 'high' || f.gravite === 'critical') {
    console.log(`::error title=Faille npm::${texte}`);
    bloquantes++;
  } else {
    console.log(`::warning title=Faille npm::${texte}`);
  }
}

console.log(`${vues.size} faille(s) trouvée(s), dont ${bloquantes} bloquante(s).`);
if (bloquantes > 0) {
  console.log('Corriger : « npm audit fix » (ou monter la version du paquet), voir docs/tests.md.');
  process.exit(1);
}
