# Tests et CI

Ce qui vérifie le launcher avant qu'il parte chez les joueurs : comment lancer les tests sur
ce PC, ce que fait chaque workflow GitHub, et comment lire un échec.

## 1. Lancer les tests sur ce PC (Windows)

Trois vérifications, les mêmes que sur GitHub. Depuis le dossier du dépôt (`FarfaLauncher`).

**PowerShell :**

```powershell
npx tsc --noEmit -p .                         # 1. types TypeScript de l'interface
cd src-tauri
$env:CARGO_TARGET_DIR = "target-test"         # toujours ce dossier de build, jamais un autre
cargo test --lib                              # 2. tests Rust (environ 240, quelques secondes)
cd ..
```

**Git Bash :**

```bash
npx tsc --noEmit -p .
cd src-tauri && CARGO_TARGET_DIR=target-test cargo test --lib
```

Depuis une autre copie de travail (`FarfaLauncher-ci`…), mettre le chemin complet pour partager
le même dossier de build : `CARGO_TARGET_DIR=C:/Users/Farfa/Documents/CLAUDE/FarfaLauncher/src-tauri/target-test`.

Après, vider `target-test\debug\incremental` (il grossit vite).

Résultat attendu : `tsc` n'écrit rien, et `cargo test` finit par
`test result: ok. N passed; 0 failed; M ignored`.

Autres commandes utiles :

| Commande (dans `src-tauri`) | Pour quoi |
|---|---|
| `cargo test --lib registry` | seulement les tests dont le nom contient `registry` |
| `cargo test --lib -- --nocapture` | voir ce que les tests affichent |
| `cargo test --lib -- --ignored` | lancer les tests ignorés (voir plus bas) |
| `cargo test` | comme la CI : bibliothèque, binaires et exemples de la doc |
| `cargo build --no-default-features --bin farfalauncherdeck` | compiler le binaire du Steam Deck |

`npm install` et `npm ci` ne sont pas nécessaires pour les tests : `node_modules` existe déjà.

### Tests ignorés

`cargo test` ne lance pas les tests marqués `#[ignore]`. Il y en a de deux sortes :

- **e2e** (`*_e2e`, `real_*`, `live_*`…) : ils ont besoin d'un vrai serveur RomM, du
  gestionnaire ou d'un fichier présent seulement sur ce PC. Lancés à la main.
- **bugs connus** : `#[ignore = "bug Lx de docs/audit-2026-11.md, corrigé à l'étape 3"]`. Le test
  décrit le bon comportement et échoue tant que le bug Lx n'est pas corrigé. Quand on corrige
  le bug, on retire la ligne `#[ignore …]` : le test doit alors passer.

La liste s'affiche avec `cargo test --lib -- --list --ignored`.

### Où sont les tests

Dans chaque fichier Rust, en bas, dans un bloc `#[cfg(test)] mod … { … }`. Chaque test a une
ligne de commentaire au-dessus qui dit ce qu'il garantit. Les tests qui écrivent des fichiers
le font dans un dossier temporaire (`%TEMP%\farfa-…`). Les nouveaux tests le suppriment à la
fin s'ils réussissent ; une quinzaine de tests plus anciens laissent encore leurs dossiers
(`%TEMP%\farfa-test-*`, `farfa-detect-*`, `farfa-queue`, `farfa-cache`) : sans danger, on
peut les effacer.

Il n'y a pas de tests de l'interface (React) : `tsc` vérifie seulement les types.

## 2. Les workflows GitHub (`.github/workflows/`)

| Fichier | Quand | Ce qu'il fait |
|---|---|---|
| `tests.yml` | appelé par les trois suivants, jamais seul | `npm ci`, `tsc`, `npm run build` (l'interface est embarquée dans le Rust), `cargo test` |
| `ci.yml` | chaque push sur n'importe quelle branche, chaque pull request, ou à la main | **tests** sur Windows, Linux et Mac ; **deck** : compile et essaie `farfalauncherdeck`, binaire dans les artefacts ; **mac-app** : `.dmg` dans les artefacts, **sans** la clé de signature |
| `release.yml` | tag `vX.Y.Z` ou `vX.Y.Z-beta.N` | **tests** (Windows, Linux) d'abord ; seulement s'ils passent : création de la release et Deck, puis **attente de votre approbation** (environnement `signature`), compilation signée Windows, AppImage, Mac, publication et canal bêta |
| `test.yml` | tag `test-<rapport>-<n>` (poussé par `outils/surveille-rapports`) | **tests** (Windows, Linux) d'abord ; seulement s'ils passent : **attente de votre approbation** (environnement `signature`), puis pré-release de test signée pour le joueur |
| `mac-e2e.yml` | tag `tour-mac-*` ou à la main | tour réel des émulateurs sur Mac (vrai serveur) |
| `securite.yml` | chaque push sur une branche, chaque pull request, chaque lundi, ou à la main | **contrôles de sécurité** : secrets (gitleaks), failles connues des dépendances npm et Rust (voir section 4) |

L'ancien `deck.yml` (branche `deck`, fusionnée) a été retiré : `ci.yml` compile le binaire du
Deck à chaque push, et le bouton « Run workflow » de `ci.yml` le donne à la demande.

Les tags ne déclenchent pas `ci.yml` : `release.yml` et `test.yml` lancent leurs propres tests.

## 3. Lire un échec de CI

1. Sur GitHub : onglet **Actions**, cliquer le run en rouge. Le schéma montre les jobs ; celui
   qui a échoué a une croix.
2. Cliquer le job, puis l'étape en rouge :
   - **Types TypeScript (tsc)** : erreur de type dans `src/`. Le message donne
     `fichier.tsx(ligne,colonne): error TS…`. Même erreur avec `npx tsc --noEmit -p .` ici.
   - **Build de l'interface** : erreur Vite (import manquant…).
   - **Tests Rust (cargo test)** :
     - `error[E…]` : le code ne compile pas sur ce système (souvent du code `#[cfg(windows)]`
       ou `#[cfg(unix)]` qu'on n'a pas compilé ici) ;
     - `test … FAILED` puis `panicked at src\fichier.rs:ligne` : un test a échoué ; le
       message sous `assertion` donne la valeur obtenue (`left`) et celle attendue (`right`).
       Relancer ce test ici avec `cargo test --lib nom_du_test`.
   - **Compiler** (job `deck`) : les erreurs sont aussi en annotations en haut du run.
   - **Dépendances système** / `npm ci` : souvent un souci passager de GitHub ou d'un miroir
     de paquets. Bouton « Re-run failed jobs ».
3. Un test qui échoue seulement sous Linux ou Mac : chercher un chemin écrit à la Windows
   (`\`), une différence de casse dans un nom de fichier, ou un `#[cfg]`.

**Release ou version de test bloquée par les tests** : rien n'a été créé ni publié. Corriger
sur `main`, puis :

- release : supprimer le tag (`git tag -d vX.Y.Z` puis `git push origin :refs/tags/vX.Y.Z`),
  le reposer sur le commit corrigé et le repousser ;
- version de test : le run en échec fait passer le rapport « à décider » (surveillance des
  rapports) ; la suite se décide dans `valider.bat`.

## 4. Contrôles de sécurité (`securite.yml`, Dependabot)

Le workflow **Sécurité** est séparé de la CI : un rouge de sécurité ne cache pas le résultat
des tests, et un rouge des tests ne cache pas une faille. Il ne tourne pas sur les tags :
**une faille ne bloque jamais `release.yml` ni `test.yml`**. Il tourne aussi chaque lundi, car
de nouvelles failles sont publiées même quand le code ne bouge pas : un rouge du lundi sans
aucun push veut dire « nouvelle faille publiée », pas « j'ai cassé quelque chose ».

| Job | Ce qu'il attrape | Bloquant ? |
|---|---|---|
| **Secrets (gitleaks)** | jeton, mot de passe, clé privée, webhook… dans **n'importe quel commit** de la branche, même effacé depuis | oui |
| **Dépendances npm (npm audit)** | paquet npm embarqué dans le launcher (pas les outils de dev) qui a une faille publiée | oui pour `high` et `critical` ; simple avertissement pour `moderate` et `low` |
| **Dépendances Rust (cargo audit)** | crate de `src-tauri/Cargo.lock` qui a une faille publiée (base RustSec) | oui pour toute faille (RustSec ne donne pas toujours de gravité) ; crates « non maintenues » ou « unsound » : avertissement |

### Lancer les contrôles sur ce PC

```bash
node scripts/npm-audit.mjs        # même contrôle npm que la CI (pas besoin d'installer quoi que ce soit)
```

`cargo audit` et `gitleaks` ne sont pas installés sur ce PC. Pour les avoir (facultatif) :
`cargo install cargo-audit --locked`, puis `cd src-tauri && cargo audit` ; gitleaks se
télécharge depuis ses releases GitHub (`gitleaks git --redact -v .` à la racine du dépôt).
Sinon, le bouton « Run workflow » du workflow **Sécurité** les lance sur GitHub.

### Lire un échec

- **Secrets (gitleaks)** : le journal donne `RuleID`, `File`, `Line`, `Commit` ; le secret
  lui-même est masqué (`REDACTED`, le dépôt est public).
  1. **Vrai secret** : le considérer comme volé (le dépôt est public). Le révoquer ou le
     changer tout de suite (jeton RomM, webhook Discord, clé…), puis le retirer du code.
     Le retirer du code ne suffit pas : il reste dans l'historique. Une fois révoqué, il ne
     risque plus rien : ajouter son empreinte dans `.gitleaksignore` (voir plus bas).
  2. **Faux positif** (exemple de la doc, faux jeton d'un test, clé publique) : ajouter une
     exception précise dans `.gitleaks.toml` (fichier + motif), avec sa raison en
     commentaire. Exemple déjà là : la clé **publique** de mise à jour dans `tauri.conf.json`.
  - `.gitleaksignore` : une ligne par découverte, l'empreinte `Fingerprint` affichée par
    gitleaks (`commit:fichier:règle:ligne`), avec un commentaire `#` au-dessus (date, raison).
- **npm audit** : une ligne `::error` par faille bloquante : avis `GHSA-…`, gravité, paquet,
  versions touchées. Corriger : `npm audit fix` (sans `--force`), ou monter la version du
  paquet dans `package.json`, puis `npm run build` et les tests. Les avertissements
  (`::warning`) se corrigent quand on peut.
- **cargo audit** : chaque faille donne `Crate`, `Version`, `ID` (`RUSTSEC-…`) et
  `Solution: Upgrade to …`. Le plus souvent : `cd src-tauri && cargo update -p <crate>`
  (reste dans les versions permises par `Cargo.toml`), puis les tests. Si c'est une crate
  tirée par une autre (`cargo tree -i <crate>` dit laquelle), c'est cette autre qu'il faut
  monter. Crates Tauri : monter la crate `tauri*` **et** le paquet npm `@tauri-apps/*`
  ensemble (même version mineure).

### Ignorer proprement une faille sans correctif

Seulement quand **aucune version corrigée n'existe** (ou qu'on ne peut pas encore la
prendre), et après avoir vérifié que le launcher n'utilise pas la partie touchée.

Ajouter une ligne dans `securite/failles-ignorees.txt` (lu par npm audit **et** cargo audit) :

```
RUSTSEC-2099-0001  2026-11-24  paquet-x : pas de version corrigée ; la fonction touchée
                               (lecture de fichiers .xyz) n'est jamais appelée par le launcher.
```

L'identifiant en début de ligne (`GHSA-…` pour npm, `RUSTSEC-…` pour cargo), puis la date et
la raison. Le contrôle passe alors en vert et rappelle la faille ignorée dans son journal.
À chaque mise à jour des dépendances, relire ce fichier et retirer les lignes devenues inutiles.

### Dependabot (`.github/dependabot.yml`)

Chaque lundi matin, Dependabot ouvre au plus deux pull requests par écosystème : actions des
workflows, paquets npm, crates Rust. Les mises à jour mineures sont groupées en une seule pull
request ; les versions majeures arrivent une par une. Chaque pull request passe par la CI et
par **Sécurité** : fusionner seulement si tout est vert (et, pour une majeure, après un essai
de la version de test). Le job `mac-app` de la CI construit toujours le `.dmg` sans paquet de
mise à jour signé : la CI ne reçoit jamais la clé de signature (section 5).

### Durcissement des workflows

- Chaque workflow est en **lecture seule** par défaut (`permissions: contents: read`). Seuls
  les jobs de `release.yml` et `test.yml` qui créent, remplissent ou publient la release ont
  `contents: write`.
- Pas de `pull_request_target`, et aucune valeur venant de l'extérieur (`github.event.…`)
  écrite directement dans un `run:` (elle passerait par `env:`).
- Les actions qui ne sont pas de GitHub (`dtolnay/rust-toolchain`, `Swatinem/rust-cache`,
  `tauri-apps/tauri-action`) sont épinglées par empreinte de commit, avec la version en
  commentaire : un tag déplacé chez eux ne change rien chez nous. Dependabot propose les
  nouvelles versions. Les binaires téléchargés (gitleaks) sont vérifiés par leur SHA-256.
- `gitleaks` : version et empreinte en tête du job `secrets` de `securite.yml`, à changer
  ensemble à la main (Dependabot ne les voit pas). On n'utilise pas l'action
  `gitleaks/gitleaks-action` : le binaire seul n'a besoin ni de licence ni de droits en
  écriture, et relit tout l'historique à chaque fois (l'action ne relit que les commits
  poussés). La licence de l'action n'est de toute façon exigée que pour les comptes
  d'organisation, pas pour un compte personnel comme celui-ci.

## 5. La clé de signature et l'environnement `signature`

La clé qui signe les mises à jour (`TAURI_SIGNING_PRIVATE_KEY` et son mot de passe) est la
chose la plus précieuse du projet : avec elle, n'importe qui peut envoyer une fausse mise à
jour que **tous** les launchers installeront. Or les workflows compilent du code que personne
n'a forcément relu : branches `correctif/<rapport>` et versions de test écrites par le
support automatique (`claude -p`, d'après le message d'un joueur), bêtas publiées par la
surveillance des rapports. Pendant la compilation, un `build.rs`, un script npm ou un plugin
Vite piégé peut lire tout ce qui est dans l'environnement du job (faille SG3 / SL6 de l'audit
de sécurité).

D'où trois règles :

| Workflow / job | Clé ? | Approbation ? |
|---|---|---|
| `ci.yml` (tous les jobs, dont `mac-app`) | **jamais** | non |
| `tests.yml` (appelé par les autres) | non | non : les tests passent **avant** l'approbation |
| `release.yml` → `create-release`, `deck`, `publish-release` | non | non |
| `release.yml` → `build` | oui | **oui** (environnement `signature`) |
| `test.yml` → `create-release`, `publish-release` | non | non |
| `test.yml` → `build` | oui | **oui** (environnement `signature`) |

- Les deux secrets sont rangés dans l'**environnement GitHub `signature`** (*Settings →
  Environments*), pas dans les secrets du dépôt. Un job ne les voit que s'il déclare
  `environment: signature` **et** que le propriétaire a approuvé ce run.
- L'environnement n'accepte que les tags `v*.*.*` et `test-*`, et exige l'approbation du
  propriétaire (*Required reviewers*). Mise en place : `release.md` §12.
- Pendant l'attente, le run est en jaune, statut *Waiting* : ce n'est pas une panne. La
  surveillance des rapports le sait et attend aussi.
- Approuver = avoir relu le diff du commit (ce qu'il faut regarder : `release.md` §6 bis).
- Les jobs `build` signés n'utilisent pas le cache Rust partagé (`Swatinem/rust-cache`) : il
  est rempli par des runs non relus et n'est pas vérifié. Ils compilent donc un peu plus
  lentement.
- Un test Rust (`commands::update::tests::signing_key_only_behind_owner_approval`) échoue si
  la clé réapparaît dans `ci.yml` ou si un workflow la donne sans `environment: signature`.

Suite possible, pas encore faite : séparer la compilation (sans clé) de la signature (un job
qui ne fait que signer les paquets avec `tauri signer sign`, sans exécuter le code du dépôt).
La clé ne pourrait plus être volée du tout, mais il faudrait remplacer `tauri-action` pour
écrire `latest.json` à la main, ce qui ne peut s'essayer qu'en publiant ; et le job de
signature signerait quand même ce que la compilation non relue a produit : l'approbation
reste nécessaire.
