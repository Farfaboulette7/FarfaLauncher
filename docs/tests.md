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
| `ci.yml` | chaque push sur n'importe quelle branche, chaque pull request, ou à la main | **tests** sur Windows, Linux et Mac ; **deck** : compile et essaie `farfalauncherdeck`, binaire dans les artefacts ; **mac-app** : `.dmg` dans les artefacts |
| `release.yml` | tag `vX.Y.Z` ou `vX.Y.Z-beta.N` | **tests** (Windows, Linux) d'abord ; seulement s'ils passent : création de la release, compilation Windows, AppImage, Mac et Deck, publication et canal bêta |
| `test.yml` | tag `test-<rapport>-<n>` (poussé par `outils/surveille-rapports`) | **tests** (Windows, Linux) d'abord ; seulement s'ils passent : pré-release de test pour le joueur |
| `mac-e2e.yml` | tag `tour-mac-*` ou à la main | tour réel des émulateurs sur Mac (vrai serveur) |

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
