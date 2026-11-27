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
| `lib.rs`, `main.rs` | Démarrage : dossiers, journaux, état partagé, liste des commandes. Fenêtre de 1440 × 880 par défaut (onglets d'un jeu sur une ligne), réduite à l'écran s'il est plus petit (`fit_to_screen`). |
| `commands/` | Commandes appelées par l'interface : `session` (connexion), `library`, `install`, `launch`, `saves`, `mods`, `update`, `report`, `requests`, `fiches` (écran « Fiches » des administrateurs), `service` (son onglet « Service »), `ajouts` (son onglet « Ajouter un jeu »), `community`, `stats`, `presence`, `music`, `sounds`, `steam`, `system`, `window`. |
| `romm/` | Client de l'API RomM : `auth` (connexion), `client` (HTTP, délais, règle du jeton), `library` (consoles, jeux), `saves`, `favorites`, `playtime`. Voir `docs/romm-api.md`. |
| `manager.rs` | Catalogue de FarfaRomManager : recomps, émulateurs, choix de la recomp par jeu, et toutes les clés des fiches. |
| `assets.rs` | Adresse du serveur des musiques et bruitages. |
| `install/` | Installation : `download` (reprise, contrôle SHA-1/MD5), `extract` (fusion sans casse), `emulator` (jeux d'émulateur), `prepare` (outils `preparer:`), `detect` (trouver l'exe), `state` (registre `installed.json`). |
| `pc/` | Jeux PC de la plateforme « Ordinateur » : `gog`, `cod` (Call of Duty et ses clients), `proton` (Linux), `steam`, `dosbox`, `net`, `install`. |
| `play.rs` | Tout ce qui se fait avant et après une partie, sans Tauri (partagé avec le Deck). |
| `launch/` | Lancement du processus, suivi de la durée, `kill_tree` (arrêt de tout l'arbre). |
| `links.rs` | Clé `lien:` : un dossier hors du jeu remplacé par un lien vers un dossier du jeu. |
| `language.rs` | Clé `langue:` et jokers `{exedir}`, `{appdata}`, `{localappdata}`, `{config}`, `{documents}`, suivis seulement de `/nom/nom` (ni `..`, ni `.`, ni `\`, ni `:`, ni chemin absolu), jamais vers un dossier protégé (liste `SENSITIVE` : `Microsoft`, Démarrage, `autostart`, `.ssh`…), pour toutes les clés. |
| `controllers.rs`, `gamepad.rs` | Manettes (SDL 3) et leur écriture dans la configuration de l'émulateur ou de la recomp. |
| `optimize.rs`, `hardware.rs` | Réglages optimisés selon le PC (clé `optimiser:`). |
| `shaders.rs`, `switch_content.rs`, `switch_saves.rs`, `n3ds.rs` | Cas propres à Switch et 3DS, pilotés par les clés des fiches. |
| `save_sync.rs`, `save_store.rs`, `saves.rs` | Sauvegardes : comparaison, envoi, restauration. |
| `stats.rs`, `presence.rs`, `report.rs` | Statistiques entre amis, présence, rapports de bug (vers le gestionnaire). |
| `achievements.rs` | Succès RetroAchievements d'un jeu : liste et progression du joueur lues dans RomM (`/api/roms/<id>/simple`, `/api/users/me`), progression des amis lue au gestionnaire (`/api/succes/jeux/<raId>`). Voir `docs/romm-api.md` §8. |
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
| `screens/GameDetail.tsx` | Fiche d'un jeu, onglets Informations, Médias, Contrôles, Succès, Communauté. L'onglet Succès (`components/AchievementsTab.tsx`, succès RetroAchievements) n'apparaît que si le jeu en a : `GameDetail` les lit à l'ouverture, après chaque partie et à chaque « Rafraîchir ». |
| `screens/InstallPanel.tsx` | Installer, Jouer, Mettre à jour, Désinstaller, dossier des sauvegardes. |
| `screens/ControlsTab.tsx` | Une manette par joueur. |
| `screens/Settings.tsx`, `PcPanel.tsx` | Paramètres : Compte, RetroAchievements (pseudo lu dans RomM, bouton qui ouvre le profil RomM `<serveur>/user/<id>` où le joueur l'indique ; l'onglet Succès y mène par « Lier mon compte »), Installation, Affichage, Sons, Émulateurs, Cache et dépannage, À propos (mises à jour, canal bêta), puis « Soutenir le projet » (`components/SupportSection.tsx`, infos de don lues dans `soutien.json` du gestionnaire par `commands/support.rs`, cachée si absent ; runbook §4). Informations du PC. |
| `components/` | Éléments communs : `Choice` (listes YoRHa, jamais de `<select>`), `MusicMenu`, `ReportsInbox` (Messages), `Fiches` (fiches du gestionnaire, administrateurs) et son onglet `ServicePanel`, `UpdateDialog`, `Downloads`, `BoxArt` (boîte 3D)… |
| `api/backend.ts` | Toutes les commandes Rust, typées. |
| `offline.ts`, `refresh.ts`, `music.ts`, `sounds.ts`, `audioGuard.ts` | Hors ligne, rafraîchissement, musique, bruitages, filet audio (Linux). |
| `ingame/Overlay.tsx` | Menu en jeu (Select + Start maintenus). |
| `theme/` | Thème NieR (CSS, police). |

L'apparence est voulue par le propriétaire : voir la section « Interface » de `CLAUDE.md`.

### Écran « Fiches » (administrateurs)

`components/Fiches.tsx` + `commands/fiches.rs` : les fiches de recomps et d'émulateurs du
gestionnaire (`recomps/<Nom>.txt`, `emulateurs/<id>.txt`), lues et modifiées sans le partage du NAS.

- Entrée : une icône dans `lib-actions`, à côté des Messages, **seulement** si `fiches_list`
  répond (compte `admin` selon `GET /api/me` du gestionnaire, et gestionnaire qui a
  `/api/fiches`). Pour les autres comptes et avec un ancien gestionnaire (404), rien ne s'affiche.
  La liste est demandée une fois par compte (sans son d'erreur ; gardée par compte, pour qu'un
  autre compte après une déconnexion ne voie jamais celle du précédent), puis à chaque ouverture.
- Liste : état de chaque fiche (valide, invalide avec l'erreur, retirée = `_<nom>.txt`).
- « + Nouvelle recomp » : formulaire des clés principales (`recomp:`, `exe:`, `argument:`,
  `remarque:`, `dossier-mods:`, `jeux:` avec la console prise dans la liste de RomM + « Autre »),
  qui produit le texte (`recompText`) puis l'ouvre dans l'éditeur.
- Éditeur : numéros de ligne, « Vérifier » (`POST /api/fiches/verifier`, rien n'est écrit, la
  ligne en cause est marquée et cliquable), « Enregistrer » (`PUT`, actif seulement si le texte
  affiché a été vérifié valide), « Retirer » (en deux clics) / « Remettre en service ».
  L'enregistrement renvoie l'empreinte de la version ouverte : une fiche changée entre-temps est
  refusée par le gestionnaire plutôt qu'écrasée.
- Hors de la maison (`modification: false`), le message « Modification possible seulement
  depuis la maison » s'affiche, avec l'adresse vue par le gestionnaire, et les boutons d'écriture
  sont désactivés. L'écriture ne marche **que depuis la maison**, mais depuis **n'importe quel
  appareil de la maison** : le DNS local de la maison mène le nom du gestionnaire à l'adresse du
  NAS sur le réseau local (sinon la box renvoie la connexion avec son adresse publique). Pas par
  le VPN du propriétaire (choix du propriétaire). Réglage et vérification :
  `docs/ajouter-un-jeu.md` du gestionnaire.

#### Onglet « Ajouter un jeu »

`components/AddGamePanel.tsx` + `commands/ajouts.rs` (onglet de la même fenêtre, à côté de
« Fiches ») : un fichier ou un dossier de Download Station devient un jeu de RomM.

- `GET /api/ajouts` du gestionnaire : la liste de Download Station (premier et deuxième niveau,
  films et séries masqués, avec ce qui est déjà lié dans RomM), les dossiers de console de
  `RomM/roms` (avec le nom de la plateforme RomM) et les demandes récentes. Cette liste est
  publiée par le **PC du propriétaire** (`outils/surveille-rapports/ajouts-romm.ps1` du
  gestionnaire), pas par le gestionnaire, qui ne voit pas ce dossier.
- Recherche (sans accents ni majuscules, 200 lignes au plus), console choisie avec `Choice`
  (proposée d'après l'extension), sous-dossier facultatif (jeu PC : un dossier au nom du jeu),
  cible affichée (`roms/<console>/…`), rappels de `docs/ajouter-un-jeu.md` (jeu PC, R8…).
- « Ajouter à RomM » (`POST /api/ajouts/demandes`, depuis n'importe quel appareil de la maison, pas par le VPN, comme les fiches) dépose une
  demande. Le PC la traite à son prochain passage (3 min) : lien physique par SSH, scan RomM de
  cette console, puis « fait » ou « échec » avec la raison. Le suivi (`GET /api/ajouts/demandes`)
  est relu toutes les 15 s tant qu'une demande attend. PC éteint (`pcEnLigne`, même signal que
  les trois petits points des rapports) : « En attente du PC (éteint) ».
- Ancien gestionnaire (404) : l'onglet le dit, sans erreur. Aucun nouveau droit RomM pour le
  launcher : seul le PC du propriétaire parle à RomM (scan).
- Les refus expliqués du gestionnaire (`{"detail": …}` en 400/403/404/409/413/422) deviennent
  `AppError::Refused` : leur message s'affiche tel quel.
- Tous les contrôles de sécurité sont dans le gestionnaire (son `docs/securite.md`, SG18) : le
  launcher ne fait que relayer, avec des noms de fichiers vérifiés et encodés dans l'adresse.
- Pas sur le Steam Deck : `commands/` n'est compilé qu'avec l'interface (`gui`), NieRDeck n'a pas
  cet écran. Linux (AppImage) et Mac : même écran que Windows.

#### Onglets de la fenêtre « Fiches »

La fenêtre a des onglets (`ONGLETS` dans `Fiches.tsx`, style `inbox-tabs` de la fenêtre Messages) :
« Fiches » (ci-dessus) et « Service ». Ajouter un onglet : une ligne dans `ONGLETS`, une ligne
`{onglet === "…" && <Composant />}` dans le rendu, et le composant dans son propre fichier. Le
bandeau « Modification possible seulement depuis la maison » ne concerne que l'onglet « Fiches ».

#### Onglet « Service » (tableau de bord du gestionnaire)

`components/ServicePanel.tsx` (+ `.css`) + `commands/service.rs` : le propriétaire voit d'un coup
d'œil si le service va bien, sans SSH ni journaux. Lecture seule, aussi hors de la maison.

- Source : `GET /api/service` du gestionnaire (administrateur revérifié toutes les 60 s ; détail
  des champs dans le `README.md` du gestionnaire, mode d'emploi dans son `docs/runbook.md`,
  « Lire le tableau de bord »). `service_status` relaie le JSON tel quel (`serde_json::Value`) :
  un bloc que ce launcher ne connaît pas est ignoré, un bloc absent n'est pas affiché.
- Blocs : Santé (`started`, dernier cycle réussi, état, version), Disque du NAS (place libre,
  seuil de 200 Go), Rapports de bug en attente (par statut, plus ancien, PC de support), Demandes
  et joueurs (nombres), En échec (recomps, émulateurs et fiches invalides, avec la raison),
  Dernières alertes (20), Recomps et émulateurs (version, date de la release, dernier passage).
- Couleurs : **rouge** = à faire maintenant (cycle arrêté, disque sous le seuil, un échec) ;
  **orange** = à regarder bientôt (disque sous deux fois le seuil, cycle sans succès depuis
  15 min, rapports `nouveau`/`réponse`/`à décider`, demandes nouvelles, PC de support éteint avec
  des rapports en attente). Le reste est neutre.
- Heures de la santé : `startedAt` et `cycleAt` (secondes Unix) ; `started` et `cycle` de `/health` sont
  des heures locales du conteneur sans fuseau, qui fausseraient « il y a … » et l'orange du cycle.
- « Actualiser » relit tout et affiche « Actualisé ✓ <heure> ».
- Gestionnaire d'avant cette route (404) : `null`, l'onglet dit qu'il faut déployer le
  gestionnaire, sans erreur. 403 (compte plus administrateur) : message d'erreur habituel.
- Pas sur le Steam Deck (comme l'écran « Fiches »). Linux et Mac : même onglet.

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

**Historique** (*Historique des sauvegardes…* sur la fiche du jeu, `historique` / `restaurer`
du moteur du Deck) : les 10 versions précédentes de chaque sauvegarde (date, appareil,
taille), gardées par FarfaRomManager, pour tous les jeux.

- **Recomp** : le gestionnaire garde la sauvegarde et son historique
  (`api/saves/<recomp>/<jeu>`, `…/history`, `…/history/<version>/restore`).
- **Jeu gardé par RomM** (émulateur, jeu PC), depuis la phase 3 : RomM reste la référence
  (lancement, comparaison, anciens launchers inchangés). Après chaque envoi réussi à RomM, le
  launcher envoie **une copie** au gestionnaire, sous `api/saves/jeux.romm/<id RomM>` (avec
  l'en-tête `X-Farfa-Device`). Le gestionnaire la traite comme une sauvegarde de recomp :
  l'ancienne copie passe dans l'historique (10 gardées, les plus anciennes effacées), quota de
  10 Go par compte, à part de celui des recomps qu'il ne bloque jamais, et seuil de disque (SG9). Une copie qui échoue n'empêche jamais l'envoi.
  - Lire l'historique : le launcher compare d'abord la sauvegarde actuelle de RomM au SHA-1 de
    la copie ; si elles diffèrent (envoi d'un ancien launcher, copie ratée), il la recopie sans
    nom d'appareil, et l'ancienne copie passe dans l'historique. Pour ne pas retélécharger à
    chaque ouverture, `romm-saves-copied.json` (dossier de réglages) note, par jeu, la date RomM
    (`updated_at`) de la dernière sauvegarde dont la copie est à jour : même date, rien n'est
    téléchargé. Fichier perdu ou abîmé : tout est simplement recomparé.
  - Restaurer : même contrôle, complet (sans le mémo) et **obligatoire** (en cas d'erreur, rien
    n'est changé), puis
    restauration sur le gestionnaire (l'actuelle passe dans l'historique), téléchargement de la
    version et envoi à RomM, enfin sur l'appareil (copie de la locale dans
    `.farfalauncher/backups/`, comme « Prendre celle du serveur »). Si l'envoi à RomM échoue,
    RomM garde l'actuelle, qui est déjà dans l'historique : on peut recommencer.
  - Gestionnaire d'avant (404 sur la copie, ou réponse de `…/history` sans l'en-tête
    `X-Farfa-Historique-Romm`) ou aucun gestionnaire : pas d'historique, message « Pas
    d'historique pour … ». Le reste marche comme avant.
- Pourquoi pas dans RomM ? RomM 5.4.1 sait garder des versions (« slots », `romm-api.md` §7),
  mais l'appareil n'y est connu qu'avec un nouveau scope (`devices.write`, reconnexion de tous
  les joueurs), les anciens launchers ne verraient plus les nouvelles sauvegardes, et c'est
  RomM qui effacerait les anciennes. La copie sur le gestionnaire ne change rien à RomM, réutilise
  le code, les quotas et l'écran des recomps, et ne demande aucun scope.
- Le moteur du Deck répond pareil pour les deux (`{"versions": [...] | null}`) : NieRDeck n'a
  rien à changer.

Routes du gestionnaire : [README du gestionnaire, « Pour le launcher »](https://github.com/Farfaboulette7/FarfaRomManager/blob/master/README.md#pour-le-launcher).

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
- Conflit de sauvegarde sur le Deck : la plus récente gagne. Avant chaque envoi (PC et Deck), le
  serveur est relu (`save_sync::after_play`) : une sauvegarde reçue d'un autre appareil n'est jamais
  écrasée à l'aveugle. `veille`, `reveil` et `resultat` servent à NieRDeck (mise en veille, réveil,
  notification à la fermeture d'un jeu).
- Les résumés de jeux (`jeux`, `recherche`) donnent `boxFaces` et `box3dUrl` : NieRDeck dessine la
  même boîte 3D que `BoxArt` du launcher.
- `succes <id>` donne les succès RetroAchievements d'un jeu (même contenu que l'onglet Succès
  du launcher, plus `friends`) ; les badges se téléchargent avec `image`. L'écran de NieRDeck qui
  les montre reste à faire.

Conséquence : toucher à `play.rs`, `save_sync.rs`, `install/` ou `pc/`, c'est aussi toucher
au Deck. La CI compile le moteur à chaque push (`ci.yml`, job `deck`). Le smoke test utilise
aussi ce moteur (commande `essai`, qui n'envoie rien au serveur).

## 9. Limites connues

Voir `docs/audit-2026-11.md` §3. En bref : sous Linux, un jeu qui se détache de son groupe de
processus termine la session trop tôt (L8) ; deux jeux PC du même nom partagent un préfixe
Proton (L14) ; des erreurs de l'interface sont encore avalées sans trace (L15).
