# AGENTS.md : règles pour un agent IA

Ce dépôt est le launcher FaRFa project (Tauri 2 : Rust dans `src-tauri/`, React/TypeScript
dans `src/`). Les consignes détaillées sont dans [CLAUDE.md](CLAUDE.md) ; ce fichier en est le
résumé obligatoire. Tout se fait **en français** : code, commentaires, commits, notes de
version, messages aux joueurs.

## Avant de commencer

- Lire `CLAUDE.md`, puis `docs/architecture.md` pour la partie touchée.
- Le dépôt est **public** : jamais d'adresse de serveur, de jeton, de webhook ni de clé dans un
  fichier, un commit, un journal ou un message.
- Ne jamais lire ni afficher `~/.tauri/farfalauncher.key` ni le `.env` de FarfaRomManager.

## Tester

Depuis la racine du dépôt (Git Bash) :

```bash
npx tsc --noEmit -p .                                          # types de l'interface
cd src-tauri && CARGO_TARGET_DIR=target-test cargo test --lib   # tests Rust
```

Résultat attendu : `tsc` n'écrit rien ; `cargo test` finit par `test result: ok`.
Détails et lecture d'un échec : `docs/tests.md`.

Version de test pour l'utilisateur (c'est lui qui teste, toujours, avant toute publication) :

```bash
npm run build && cd src-tauri && CARGO_TARGET_DIR=target-test cargo build --features tauri/custom-protocol
rm -rf target-test/debug/incremental
```

## Interdits

- **Un seul dossier de build : `src-tauri/target-test`.** Jamais d'autre `target*`, de copie
  de l'exe, ni de build laissé dans un dossier temporaire. Depuis une autre copie de travail,
  viser ce même dossier par son chemin complet.
- **Pas de `git commit -am`** ni de `git add -A` à l'aveugle : `src-tauri/Cargo.toml` porte un
  changement local de `[profile.dev]` qui ne doit jamais être commité. Indexer fichier par
  fichier.
- **Ne jamais changer de branche** dans `FarfaLauncher` (`main`) : d'autres sessions y
  travaillent. Ne toucher qu'aux fichiers de sa tâche.
- **Ne pas changer l'apparence d'un écran** que l'utilisateur n'a pas demandé à changer.
  Pour un nouvel élément : règles de la section « Interface » de `CLAUDE.md` (pas de
  `<select>` natif, boutons compacts, bouton qui montre qu'il a agi).
- **Ne pas publier** (tag `v*`, annonce) sans que l'utilisateur ait testé et dit oui. Le
  numéro est +0.0.1 ; demander pour +0.1.0 ou +1.0.0, et « bêta ou stable ? ».
- Pas de `npm install` ni d'ajout de dépendance sans accord : `node_modules` existe déjà.
- Rien dans AppData pour les jeux : émulateurs, recomps et outils gardent leurs fichiers dans
  le dossier du jeu (mode portable, `user/`, liens `lien:`).

## Parité des systèmes

Ce qui est fait pour Windows doit l'être aussi pour Linux (AppImage), le Steam Deck
(`farfalauncherdeck`, plugin NieRDeck) et, si possible, Mac. Sinon, le dire à l'utilisateur.

- Tout `.exe` de la plateforme « Ordinateur » passe par `pc/` (Proton sous Linux), jamais par
  une recomp.
- `play.rs`, `save_sync.rs` et `install/` sont partagés avec le Deck : vérifier aussi
  `cd src-tauri && cargo build --no-default-features --bin farfalauncherdeck`.
- Code `#[cfg(windows)]` / `#[cfg(unix)]` : la CI compile les trois systèmes à chaque push
  (`ci.yml`) ; regarder son résultat.

## Commits

Un commit par sujet, message en français, terminé par la ligne d'attribution demandée par la
session (par exemple `Co-Authored-By: …`). Ne pas pousser sans qu'on le demande.
