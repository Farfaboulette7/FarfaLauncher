# FarfaLauncher (FaRFa project)

Launcher de bureau pour les jeux du serveur RomM de l'utilisateur et de quelques amis : recomps,
émulateurs, jeux PC. Interface en français. Dépôt **public** `github.com/Farfaboulette7/FarfaLauncher`,
branche `main` : jamais d'URL de serveur, de jeton ni de clé dans le dépôt. Le catalogue (recomps,
émulateurs, réglages) vient de FarfaRomManager (`../FarfaRomManager`).

Stack : Tauri 2 (Rust, `src-tauri/`) + React 19 / TypeScript / Vite (`src/`). Feature Cargo `gui`
par défaut ; le binaire `farfalauncherdeck` (CLI JSON pour NieRDeck) se compile sans elle.

Documentation (à lire avant de toucher à une partie) :

| Fichier | Contenu |
|---|---|
| `docs/architecture.md` | modules Rust, écrans React, flux complet, stockage local, canaux de mise à jour, moteur du Deck |
| `docs/release.md` | publier une version, pas à pas (numéro, notes, tag, annonce, bêtas automatiques, clé de signature) |
| `docs/runbook.md` | problèmes des joueurs et solutions, journaux, smoke test |
| `docs/tests.md` | lancer les tests, rôle de chaque workflow, lire un échec de CI |
| `docs/smoke-test.md` | contrôle de bout en bout contre les vrais serveurs |
| `docs/romm-api.md` | l'API RomM utilisée |
| `docs/audit-2026-11.md` | audit de la phase 1 (bugs L1-L19, ce qui reste) |
| `AGENTS.md` | règles courtes pour un agent IA |

Architecture d'ensemble (NAS, RomM, gestionnaire, fiches) : dépôt FarfaRomManager,
[docs/architecture.md](https://github.com/Farfaboulette7/FarfaRomManager/blob/master/docs/architecture.md).

## Commandes (vérifiées)

```bash
npx tsc --noEmit -p .                                              # types
cd src-tauri && CARGO_TARGET_DIR=target-test cargo test --lib       # tests Rust (~280, ~24 ignorés)
npm run build && cd src-tauri && CARGO_TARGET_DIR=target-test cargo build --features tauri/custom-protocol
```

La dernière ligne construit **la** version de test :
`src-tauri/target-test/debug/farfalauncher.exe`. C'est celle que l'utilisateur lance pour tester.
- Une seule version de test, toujours dans `target-test` : il se perd s'il y en a d'autres, et
  les builds ne doivent pas remplir son disque. Vider `target-test/debug/incremental` à
  chaque build de test (16 Go accumulés le 2026-11-16).
- Depuis une autre copie de travail : `CARGO_TARGET_DIR=C:/Users/Farfa/Documents/CLAUDE/FarfaLauncher/src-tauri/target-test`
  (même dossier, jamais un nouveau).
- « Accès refusé » à la construction : l'exe de test tourne encore. Demander de le fermer.
- Sans `npm run build` avant, l'exe embarque l'ancienne interface.
- Binaire du Deck : `cd src-tauri && cargo build --no-default-features --bin farfalauncherdeck`.
- Smoke test (vrais serveurs, lecture seule) : double-clic sur `scripts\smoke-test.bat`
  (`docs/smoke-test.md`).

## CI (`.github/workflows/`, détails dans `docs/tests.md`)

- `tests.yml` : `tsc`, `npm run build`, `cargo test`. Appelé par les trois suivants.
- `ci.yml` : chaque push, toutes branches : tests Windows/Linux/Mac, binaire du Deck, `.dmg`.
- `release.yml` : tag `vX.Y.Z` ou `vX.Y.Z-beta.N`. **Tests d'abord** ; s'ils échouent, rien
  n'est publié.
- `test.yml` : tag `test-<rapport>-<n>` (version de test d'un rapport de bug). Tests d'abord.
- `smoke.yml` : chaque matin, partie réseau du smoke test (secrets `ROMM_SMOKE_*`).
- `mac-e2e.yml` : à la main, tour réel des émulateurs sur Mac.

## Publier (seulement après le test de l'utilisateur)

Pas à pas complet pour un humain : `docs/release.md`. En bref :

1. `git fetch` puis `git log main..origin/main` : le support des rapports publie lui-même
   des bêtas (fusion d'un `correctif/<rapport>` validé par le joueur). Si `origin/main` est
   en avance, reprendre ses commits (et sa version) avant de choisir le numéro.
2. Numéro : dernière version **+0.0.1**. Demander à l'utilisateur si ça mérite +0.1.0 ou
   +1.0.0, et **« bêta ou stable ? »**.
3. Écrire `docs/changelog/vX.Y.Z.md` (ou `vX.Y.Z-beta.N.md`) : corps de la release, lu par
   les amis, en français, orienté joueur, avec « Nouveautés » et « Corrections ».
4. `node scripts/bump-version.mjs X.Y.Z` (ou `X.Y.Z-beta.N`). Il met à jour package.json,
   package-lock, tauri.conf.json, Cargo.toml et Cargo.lock.
5. Commiter **sans** le changement local de `[profile.dev]` dans `src-tauri/Cargo.toml` :
   - il appartient à l'utilisateur et ne doit jamais être commité ;
   - donc pas de `git commit -am` ;
   - n'indexer que la ligne `version` : `git show HEAD:src-tauri/Cargo.toml | sed … | git hash-object -w --stdin`
     puis `git update-index --cacheinfo` (un humain fait `git add -p src-tauri/Cargo.toml`).
   Un commit des changements, puis un commit « Version X.Y.Z ».
6. `git tag vX.Y.Z`, `git push origin main`, `git push origin vX.Y.Z`.
7. Juste après le push du tag, stable seulement (pas pour une bêta, sauf demande) : annonce
   Discord dans le salon farfalauncher, par `DISCORD_LAUNCHER` (`/z/FarfaRomManager/.env`, sans
   l'afficher), en embed, avec « disponible dans un petit moment ». Sans attendre la compilation.
8. Rapports de bug corrigés par cette version : dans
   `/z/FarfaRomManager/data/reports/<id>/infos.json`, mettre `status: "corrigé"`,
   `corrigeDans: "X.Y.Z"`, `updatedAt` et une `reponse` pour le joueur.

`release.yml` compile Windows, Linux (AppImage), Mac et `FarfaLauncherDeck-linux-x86_64`, signe,
publie `latest.json` et le copie dans la release technique `canal-beta` (lue par le canal
bêta, `commands/update.rs`).

## Architecture (src-tauri/src), détail dans `docs/architecture.md`

- `romm/` : client RomM (bibliothèque, détails, fichiers, favoris, temps de jeu, sauvegardes).
  `romm/client.rs::with_base` n'envoie le jeton qu'à la même machine ou au même domaine
  enregistrable que RomM (L16). Un 401 n'efface le jeton que s'il vient de RomM (JSON).
- `manager.rs` : catalogue de FarfaRomManager (recomps, émulateurs, choix de la recomp par jeu).
  Clés lues depuis la phase 1 : `remarque:` (`note`, sans « Conseillé »),
  `systemes-manquants:` (`missingSystems`, raison affichée), `pour:` (client de jeu PC,
  `for`), `proton-steam:`, `contenu-externe:` et `shaders:` (émulateurs, `OrOff` : valeur,
  « non », ou absente = ancienne règle), `lien:` aussi pour les émulateurs,
  `manettes: x | joueurs=N`, `{profil}` dans les sauvegardes. Format des fiches :
  `FarfaRomManager/docs/regles-des-fiches.md`.
- `install/` : téléchargement, extraction sans casse (`merge_into` ne supprime jamais),
  préparation (`prepare.rs`, outils `preparer:`), état des jeux installés (`state.rs`,
  verrou `REGISTRY_LOCK`). Les ROM `*.extract.zip` sont extraites à côté de l'exe, puis
  supprimées une par une. `rel_to_path` ne sort jamais du dossier du jeu.
- `safe_file.rs` : `installed.json` et `settings.json` écrits par un fichier temporaire unique ;
  un fichier illisible est renommé `<nom>.illisible-<date>`, jamais écrasé.
- `links.rs` : `lien:` (recomps et émulateurs). Un vrai dossier déjà là est déplacé dans le jeu
  sans perte (copie via `.farfa-part`, vérifiée), repère `.farfa-migre` ; à la
  désinstallation il retourne à sa place, sinon erreur `FolderNotRestored` et rien n'est effacé.
- `play.rs` : tout ce qui se fait avant le lancement (liens, sauvegarde, langue, manettes,
  réglages optimisés, shaders). `launch/` lance et suit le processus (`kill_tree`).
- `pc/` : jeux PC de la plateforme « Ordinateur » : GOG, archives, jeux prêts à lancer et Call
  of Duty. **Tout `.exe` s'y lance avec Proton sous Linux.**
  - Call of Duty (`pc/cod.rs`) : `<jeu>_game_files.zip`, ou en morceaux
    `<jeu>_game_files-NN.zip`, plus un client choisi par la clé `pour:` de sa fiche.
  - Les clients viennent de CB Servers (Plutonium, IW4x…). Pour Black Ops III, c'est boiii,
    servi par la fiche `boiii` de FarfaRomManager (`sync_boiii`, lien `%LOCALAPPDATA%\boiii`
    dans le préfixe Proton sous Linux).
  - Ne jamais relier un jeu PC à une recomp : il perdrait Proton.
- `language.rs` : règles `langue:` et jokers `{exedir}`, `{appdata}`, `{localappdata}`,
  `{config}`, `{documents}`.
- `secrets.rs` : jeton dans le trousseau ; sous Linux sans Secret Service, repli
  `<config>/token.json` (0600).
- `error.rs` : `lock_or_recover()` au lieu de `.lock().unwrap()` (jamais de panique sur un
  verrou empoisonné).
- `deck.rs` + `src/bin/farfalauncherdeck.rs` : moteur partagé avec NieRDeck. Toucher à
  `play.rs`, `save_sync.rs` ou `install/`, c'est aussi toucher au Deck.

## Interface (voulue par l'utilisateur, 1.0.0)

**Ne pas changer l'apparence d'un écran qu'il n'a pas demandé à changer.**

- En haut, le joueur et les touches de fonction ; en dessous, la recherche de jeux.
- En-tête de la bibliothèque en deux colonnes (`lib-header`) :
  - à gauche : nom, puis onglets ;
  - à droite (`lib-status`) : pseudo avec l'avatar RomM, sans cadre (`/api/users/<id>/avatar`,
    servi par `image_path`), puis `lib-actions`.
- Boutons : `n-btn--sm` (compact, 38 px) ou `n-icon-btn` (icône carrée avec `title` et
  `aria-label`). Les actions secondaires sont des liens discrets (`+ Demander un jeu` près de
  la recherche).
- Jamais de `<select>` ni de `datalist` natifs : `components/Choice.tsx`. Une valeur qui doit
  exister dans RomM se choisit dans la liste, avec « Autre » pour un champ libre.
- Musique : `components/MusicMenu.tsx` (icône ♪, panneau animé). Le filet audio
  (`audioGuard.ts`) ne coupe le son que sous Linux.
- Messages (`ReportsInbox.tsx`) : Nouvelles versions, Toutes les demandes (admin), Mes demandes,
  Mes signalements.
- Un bouton montre toujours qu'il a agi (« Envoyé ✓ »…).

## Pièges

- Ne jamais changer de branche ici : d'autres sessions Claude travaillent en parallèle.
  Les branches `macos` et `deck` sont déjà fusionnées dans `main`.
- Ce qui est fait pour Windows doit l'être aussi pour Linux (AppImage) et le Steam Deck, ou
  l'utilisateur doit être prévenu.
- Les musiques et bruitages viennent de `assets.<domaine>` (`assets.rs`), servis par
  FarfaRomManager (`Z:/FarfaRomManager/assets`), plus par le conteneur `farfa-assets`.
- Une recomp Windows ne tourne pas sous Linux : il n'y a pas de Proton pour les recomps,
  seulement pour les jeux PC.
- Les noms de jeu dans les fiches du gestionnaire doivent être exactement le `fs_name` de RomM
  (ou son nom sans extension). Le slug de console est le `slug` RomM (`win` pour
  « Ordinateur »), pas le `fs_slug`.
- Les plateformes `portage-*` n'existent plus (retirées de RomM le 08/11/2026, code retiré,
  écart E14). Il reste seulement la reprise des anciens ID (`previous_rom_id`).
- Ne jamais lire la clé de signature `~/.tauri/farfalauncher.key`.
- `docs/romm-api.md` décrit l'API RomM 5.4 utilisée.
