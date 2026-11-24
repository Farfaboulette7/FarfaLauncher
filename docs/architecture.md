# Architecture du launcher

Comment FaRFa project (nom technique `FarfaLauncher`) est construit, et ce qui se passe entre
l'ouverture du launcher et la fin d'une partie. État du 24/11/2026 (version 1.2.1 + phase 1).

Ce document ne parle que du launcher. Le reste de l'installation (NAS, RomM, FarfaRomManager,
fiches des recomps et des émulateurs) est décrit dans le dépôt du gestionnaire :
[architecture globale](https://github.com/Farfaboulette7/FarfaRomManager/blob/master/docs/architecture.md).

## 1. Vue d'ensemble

```
            ┌──────────── PC du joueur ────────────┐
            │  Interface React (src/)              │
            │        │ invoke("commande") / événements
            │  Cœur Rust (src-tauri/src/)          │
            └───┬──────────────┬──────────────┬────┘
                │ jeton RomM   │ jeton RomM   │ jeton RomM
                ▼              ▼              ▼
        romm.<domaine>   manager.<domaine>  assets.<domaine>
        (RomM : jeux,    (FarfaRomManager : (FarfaRomManager :
         sauvegardes,     catalogue, fichiers musiques et
         favoris,         des recomps et      bruitages)
         temps de jeu)    émulateurs, stats,
                          rapports, présence)

        github.com/Farfaboulette7/FarfaLauncher/releases : mises à jour du launcher
```

- **Deux parties.** L'interface (React 19 + TypeScript, dans `src/`) ne touche ni au disque
  ni au réseau ni au jeton. Elle appelle des commandes Rust (`src-tauri/src/commands/`) avec
  `invoke`, et reçoit des événements (progression d'une installation, jeu lancé, jeu
  arrêté…). Tout le reste est en Rust.
- **Un seul jeton.** Le joueur se connecte à RomM. Le même jeton sert d'identifiant auprès du
  gestionnaire et du serveur des assets, qui le vérifient auprès de RomM. Le launcher ne
  l'envoie qu'à une adresse de la même machine ou du même domaine que RomM
  (`romm/client.rs`, `with_base`).
- **Adresses déduites.** Le joueur ne saisit que l'adresse de RomM. Celles du gestionnaire et
  des assets s'en déduisent en remplaçant le premier niveau (`romm.exemple.fr` →
  `manager.exemple.fr`, `assets.exemple.fr`). Elles se changent dans *Paramètres →
  Émulateurs* et *Paramètres → Sons*.
- **Le launcher ne contacte jamais GitHub pour les jeux.** Les recomps et émulateurs sont
  servis par le gestionnaire, qui suit lui-même leurs nouvelles versions. GitHub ne sert qu'aux
  mises à jour du launcher (et aux clients de quelques jeux PC, `pc/net.rs`).

## 2. Les modules Rust (`src-tauri/src/`)

| Module | Rôle |
|---|---|
| `lib.rs`, `main.rs` | Démarrage : dossiers, journaux, état partagé, liste des commandes. |
| `commands/` | Commandes appelées par l'interface : `session` (connexion), `library`, `install`, `launch`, `saves`, `mods`, `update`, `report`, `requests`, `community`, `stats`, `presence`, `music`, `sounds`, `steam`, `system`, `window`. |
| `romm/` | Client de l'API RomM : `auth` (connexion), `client` (HTTP, délais, règle du jeton), `library` (consoles, jeux), `saves`, `favorites`, `playtime`. Voir `docs/romm-api.md`. |
| `manager.rs` | Catalogue de FarfaRomManager : recomps, émulateurs, choix de la recomp par jeu, et toutes les clés des fiches. |
| `assets.rs` | Adresse du serveur des musiques et bruitages. |
| `install/` | Installation : `download` (reprise, contrôle SHA-1/MD5), `extract` (fusion sans casse), `emulator` (jeux d'émulateur), `prepare` (outils `preparer:`), `detect` (trouver l'exe), `state` (registre `installed.json`). |
| `pc/` | Jeux PC de la plateforme « Ordinateur » : `gog`, `cod` (Call of Duty et ses clients), `proton` (Linux), `steam`, `dosbox`, `net`, `install`. |
| `play.rs` | Tout ce qui se fait avant et après une partie, sans Tauri (partagé avec le Deck). |
| `launch/` | Lancement du processus, suivi de la durée, `kill_tree` (arrêt de tout l'arbre). |
| `links.rs` | Clé `lien:` : un dossier hors du jeu remplacé par un lien vers un dossier du jeu. |
| `language.rs` | Clé `langue:` et jokers `{exedir}`, `{appdata}`, `{localappdata}`, `{config}`, `{documents}`. |
| `controllers.rs`, `gamepad.rs` | Manettes (SDL 3) et leur écriture dans la configuration de l'émulateur ou de la recomp. |
| `optimize.rs`, `hardware.rs` | Réglages optimisés selon le PC (clé `optimiser:`). |
| `shaders.rs`, `switch_content.rs`, `switch_saves.rs`, `n3ds.rs` | Cas propres à Switch et 3DS, pilotés par les clés des fiches. |
| `save_sync.rs`, `save_store.rs`, `saves.rs` | Sauvegardes : comparaison, envoi, restauration. |
| `stats.rs`, `presence.rs`, `report.rs` | Statistiques entre amis, présence, rapports de bug (vers le gestionnaire). |
| `runtimes.rs` | Recomps à plusieurs jeux (Gen1recomp…). |
| `settings.rs`, `secrets.rs`, `safe_file.rs`, `cache.rs`, `logging.rs`, `state.rs`, `error.rs` | Réglages, jeton, écriture sûre, cache, journaux, état partagé, erreurs. |
| `dialogs.rs`, `ingame.rs` | Windows seulement : réponse automatique aux fenêtres d'un émulateur, menu en jeu. |
| `deck.rs`, `bin/farfalauncherdeck.rs` | Moteur sans interface pour le Steam Deck (§8). |

## 3. Les écrans React (`src/`)

| Fichier | Rôle |
|---|---|
| `App.tsx` | Choisit l'écran : connexion ou bibliothèque. Intro, fenêtre de mise à jour. |
| `screens/Login.tsx` | Adresse du serveur, puis code appareil (ou code d'appairage, ou jeton collé). |
| `screens/Library.tsx` | Bibliothèque : en-tête (joueur, actions), onglets, recherche, grille. Ouvre les Paramètres et la fiche d'un jeu. |
| `screens/GameDetail.tsx` | Fiche d'un jeu, onglets Informations, Médias, Contrôles, Communauté. |
| `screens/InstallPanel.tsx` | Installer, Jouer, Mettre à jour, Désinstaller, dossier des sauvegardes. |
| `screens/ControlsTab.tsx` | Une manette par joueur. |
| `screens/Settings.tsx`, `PcPanel.tsx` | Paramètres : Compte, Installation, Affichage, Sons, Émulateurs, Cache et dépannage, À propos (mises à jour, canal bêta). Informations du PC. |
| `components/` | Éléments communs : `Choice` (listes YoRHa, jamais de `<select>`), `MusicMenu`, `ReportsInbox` (Messages), `UpdateDialog`, `Downloads`, `BoxArt` (boîte 3D)… |
| `api/backend.ts` | Toutes les commandes Rust, typées. |
| `offline.ts`, `refresh.ts`, `music.ts`, `sounds.ts`, `audioGuard.ts` | Hors ligne, rafraîchissement, musique, bruitages, filet audio (Linux). |
| `ingame/Overlay.tsx` | Menu en jeu (Select + Start maintenus). |
| `theme/` | Thème NieR (CSS, police). |

L'apparence est voulue par le propriétaire : voir la section « Interface » de `CLAUDE.md`.

## 4. Le parcours d'un joueur

| Étape | Code | Ce qui se passe |
|---|---|---|
| **Connexion** | `commands/session.rs`, `romm/auth.rs`, `secrets.rs` | Vérifie que l'adresse est un RomM (`/api/heartbeat`), puis code appareil (le joueur clique « Approve » dans RomM), code d'appairage ou jeton collé. Le jeton va dans le trousseau du système. |
| **Bibliothèque** | `commands/library.rs` (`start_refresh`), `manager.rs`, `romm/library.rs` | Charge `assets.json`, le catalogue du gestionnaire, puis les consoles et les jeux de RomM. Tout est gardé en cache : la bibliothèque s'affiche aussitôt, même hors ligne. |
| **Installation** | `commands/install.rs`, `install/`, `pc/` | Place disque, téléchargements (RomM ou gestionnaire, avec reprise et contrôle d'intégrité), extraction sans écraser les sauvegardes, `preparer:`, entrée dans `installed.json`. |
| **Lancement** | `commands/launch.rs` → `play.rs` → `launch/` | Liens (`lien:`), comparaison des sauvegardes, langue, manettes, réglages optimisés, shaders, puis lancement et suivi du processus. |
| **Fin de partie** | `play.rs` (`finish`), `save_sync.rs`, `stats.rs` | Envoi de la sauvegarde si elle a changé et du temps de jeu. Hors ligne : mis en file, envoyé au retour du serveur. |

Trois chemins d'installation, selon ce que le catalogue dit du jeu :

- **recomp** (portage PC natif) : archive du gestionnaire, plus la ROM de RomM si besoin ;
- **émulateur** : l'émulateur s'installe une fois dans `Emulateurs/<id>/`, la ROM dans
  `Roms/<console>/<jeu>/` ;
- **jeu PC** (« Ordinateur ») : `pc/`, toujours lancé avec Proton sous Linux (GE-Proton par
  umu-launcher, préfixe par jeu).

Si un jeu a une recomp et un émulateur, la recomp est proposée en premier.

## 5. Sauvegardes

- **Avant de jouer**, `save_sync::check` compare la sauvegarde locale à celle du serveur.
  Si elles diffèrent, le joueur choisit (celle de cet appareil par défaut). Prendre celle du
  serveur garde d'abord une copie dans `.farfalauncher/backups/` du jeu (5 copies au plus).
- **Après la partie**, la sauvegarde est envoyée si elle a changé : une archive
  `farfa-save.zip` par jeu et par joueur.
- **Où** (`save_store.rs`) : sur RomM, sauf pour une recomp, dont la sauvegarde vit sur le
  gestionnaire (sa fiche RomM peut disparaître).
- **Hors ligne** : `pending_upload` dans `installed.json`, envoi au retour (`flush_pending`).

Côté serveur (historique, restauration) :
[sauvegardes](https://github.com/Farfaboulette7/FarfaRomManager/blob/master/docs/sauvegardes.md).

## 6. Stockage local

`<id>` = `fr.farfaboulette.farfalauncher`.

| Quoi | Windows | Linux / Steam Deck (AppImage) |
|---|---|---|
| Réglages et état (`settings.json`, `installed.json`, `catalog.json`…) | `%APPDATA%\<id>\` | `~/.config/<id>/` |
| Jeton | Gestionnaire d'identification Windows | Secret Service (KWallet, GNOME Keyring), sinon `~/.config/<id>/token.json` (droits 0600) |
| Journaux (un fichier par jour) | `%LOCALAPPDATA%\<id>\logs\` | `~/.local/share/<id>/logs/` |
| Cache (liste des jeux, jaquettes, livrets, sons) | `%LOCALAPPDATA%\<id>\cache\` | `~/.cache/<id>/cache/` |
| Préfixes Proton | — | dans le dossier de réglages |
| Jeux (par défaut) | `Jeux\` à côté du launcher installé (`%LOCALAPPDATA%\FarfaLauncher\Jeux`) | `Jeux/` à côté de l'AppImage, sinon `~/FarfaLauncher/Jeux` |

Le dossier des jeux se change dans *Paramètres → Installation*. Mac : réglages dans
`~/Library/Application Support/<id>/`, journaux dans `~/Library/Logs/<id>/`.

**Écriture sûre (`safe_file.rs`, phase 1).** `installed.json` et `settings.json` sont écrits
dans un fichier temporaire au nom unique, puis renommés : un arrêt brutal laisse l'ancien
fichier entier. Un fichier illisible n'est jamais écrasé : il est renommé
`installed.json.illisible-AAAAMMJJ-HHMMSS` (ou `settings.json.illisible-…`) et le launcher
repart d'un fichier vide. Les `*.tmp` abandonnés depuis plus d'une heure sont effacés au
démarrage. Le registre des jeux est modifié sous un verrou (`REGISTRY_LOCK`).

**Liens (`links.rs`, phase 1).** Une fiche peut dire qu'un dossier hors du jeu (par exemple
`%LOCALAPPDATA%\boiii`) doit être un lien vers un dossier du jeu. Si un vrai dossier existe
déjà, il est copié dans le jeu (fichiers `.farfa-part` pendant la copie, comparaison avant
d'effacer l'original), et un repère `.farfa-migre` est posé. À la désinstallation, ce dossier
retourne à son chemin d'origine ; si c'est impossible, rien n'est effacé et la
désinstallation s'arrête avec un message.

## 7. Mises à jour du launcher et canaux

`commands/update.rs`, plugin updater de Tauri. Vérification au démarrage puis toutes les
30 minutes (`hooks/useUpdater.ts`), et avec *Paramètres → À propos → Vérifier les mises à jour*.

| Canal | Qui le reçoit | Ce que lit le launcher |
|---|---|---|
| **stable** | tout le monde | `releases/latest/download/latest.json` (dernière release non bêta) |
| **bêta** | ceux qui ont coché « Recevoir les versions de test » (*À propos*) | `latest.json` de la release technique `canal-beta`, remplacé à chaque publication, bêta ou stable ; repli sur le stable |
| **test d'un rapport** | le joueur qui a signalé le problème | pré-release `test-<rapport>-<n>`, installée par « Installer la version de test » dans son signalement |

Chaque mise à jour est signée : le launcher refuse un paquet qui n'est pas signé par la clé
privée du propriétaire (clé publique dans `src-tauri/tauri.conf.json`). Publier une version :
[release.md](release.md).

## 8. Steam Deck : le moteur `farfalauncherdeck`

Le même code Rust, compilé sans l'interface (`--no-default-features`, sans la feature `gui`),
donne un petit programme en ligne de commande : `FarfaLauncherDeck-linux-x86_64`, joint à
chaque release.

- Il est piloté par le plugin Decky **NieRDeck** (onglet « Farfa project ») et lancé par
  Steam (raccourci `jouer <id>` de chaque jeu installé).
- Chaque commande (`consoles`, `jeux`, `installer`, `jouer`, `synchroniser`, `essai`…)
  répond en lignes JSON. Liste : `farfalauncherdeck aide`.
- Réglages, jeton (`token`, 0600) et journaux (`logs/`) dans `~/.config/farfalauncherdeck/`.
- NieRDeck télécharge lui-même le moteur depuis la dernière release quand il est plus récent.
- Conflit de sauvegarde sur le Deck : la plus récente gagne.

Conséquence : toucher à `play.rs`, `save_sync.rs`, `install/` ou `pc/`, c'est aussi toucher
au Deck. La CI compile le moteur à chaque push (`ci.yml`, job `deck`). Le smoke test utilise
aussi ce moteur (commande `essai`, qui n'envoie rien au serveur).

## 9. Limites connues

Voir `docs/audit-2026-11.md` §3. En bref : sous Linux, un jeu qui se détache de son groupe de
processus termine la session trop tôt (L8) ; deux jeux PC du même nom partagent un préfixe
Proton (L14) ; des erreurs de l'interface sont encore avalées sans trace (L15).
