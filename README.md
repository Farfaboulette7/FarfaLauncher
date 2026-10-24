# FaRFa project

<img src="src/assets/logo.png" width="76" alt="Farfaboulette" style="image-rendering: pixelated">

Launcher desktop pour installer, mettre à jour et lancer en un clic les jeux recomp /
portages PC natifs hébergés sur un serveur [RomM](https://romm.app) (plateformes
`portage-*`), avec l'identité visuelle YoRHa de *NieR:Automata*.

Trois onglets : **Récent** (derniers jeux ajoutés), **Bibliothèque** (par console) et
**Favoris** (le cœur de RomM, partagé entre vos appareils). Les jeux s'affichent dans
leur boîte en 3D, que l'on fait tourner à la souris sur la fiche du jeu. Vos **sauvegardes**
sont copiées sur le serveur à la fin de chaque partie, pour les retrouver sur vos autres
appareils.

> Le programme garde son nom technique `FarfaLauncher` (fichiers d'installation, dossiers)
> pour que la mise à jour automatique reste transparente ; seul le nom affiché a changé.

## Pour mes amis : installation

Téléchargez le fichier de votre système dans la
[dernière release](https://github.com/Farfaboulette7/FarfaLauncher/releases/latest) :

| Système | Fichier |
|---|---|
| Windows | `FarfaLauncher_x.y.z_x64-setup.exe` |
| Linux / Steam Deck | `FarfaLauncher_x.y.z_amd64.AppImage` |
| macOS (Apple Silicon : M1, M2…) | `FarfaLauncher_x.y.z_aarch64.dmg` |
| macOS (Intel) | `FarfaLauncher_x.y.z_x64.dmg` |

Ensuite, FaRFa project **se met à jour tout seul** : il vérifie au démarrage et propose
la nouvelle version (aussi : *Paramètres → Vérifier les mises à jour*).

### Windows : avertissement SmartScreen / antivirus

Le launcher n'est pas signé par un certificat payant : Windows affiche
« Windows a protégé votre ordinateur ». C'est normal pour un petit projet entre amis.

1. Cliquez **Informations complémentaires**, puis **Exécuter quand même**.
2. L'installation se fait dans votre dossier utilisateur, sans droits administrateur.

Si votre antivirus bloque le fichier, autorisez-le (ou ajoutez une exception) : le code
source est public et les fichiers sont compilés par GitHub à partir de ce dépôt.

### Linux / Steam Deck

```bash
chmod +x FarfaLauncher_*.AppImage
./FarfaLauncher_*.AppImage
```

Sur Steam Deck, faites-le en mode Bureau. Vous pouvez ensuite ajouter l'AppImage à Steam
(*Ajouter un jeu non-Steam*) pour le lancer depuis le mode Jeu.

### macOS

L'application n'est pas notarisée par Apple. Après l'avoir glissée dans *Applications*,
si macOS dit qu'elle est « endommagée » ou « ne peut pas être ouverte », ouvrez le
Terminal et tapez :

```bash
xattr -cr /Applications/FarfaLauncher.app
```

### Bon à savoir

- Les jeux s'installent par défaut dans `~/FarfaLauncher/Jeux` (modifiable dans
  *Paramètres*). Vos sauvegardes (`save`/`saves`) ne sont jamais écrasées lors d'une mise
  à jour : elles sont même copiées dans `.farfalauncher/backups/` avant.
- **N'utilisez pas la mise à jour intégrée d'un jeu** (certains, comme gen1recomp, en
  proposent une) : c'est FaRFa project qui gère les versions, avec le badge
  « Mise à jour disponible ».
- Les mods se téléchargent dans `mods_downloads/` du jeu : à vous de les placer selon les
  instructions du mod (boutons « Ouvrir le dossier » dans l'onglet *Mods*).
- En cas de souci : *Paramètres → Ouvrir les logs*, et envoyez le fichier du jour.

### Sauvegardes sur le serveur

- **En lançant un jeu**, le launcher compare votre sauvegarde à celle du serveur. Si elles
  diffèrent, il vous demande laquelle garder (par défaut : celle de cet appareil). Prendre
  celle du serveur remplace la vôtre ; une copie de secours est gardée dans
  `.farfalauncher/backups/` du jeu.
- **En quittant le jeu**, votre sauvegarde est envoyée sur le serveur si elle a changé.
- Le launcher cherche seul le dossier des sauvegardes (dossier `save`/`saves`, recomps
  connues). Sinon il vous le demande au premier lancement : la documentation de la recomp
  indique en général où il se trouve. Vous pouvez le changer depuis la fiche du jeu
  (*Dossier des sauvegardes…*), ou choisir de jouer sans synchroniser.

## Se connecter

1. Au premier lancement, saisissez l'adresse de votre serveur RomM (celle que vous ouvrez
   dans votre navigateur).
2. FaRFa project affiche un code et un bouton **« Ouvrir RomM »**. Connectez-vous à RomM
   dans la page qui s'ouvre, vérifiez que le code est le même, puis cliquez **Approve**.
   C'est tout : le launcher se connecte seul, et l'appareil apparaît dans votre compte RomM
   (vous pouvez le révoquer à tout moment).

Le token est rangé dans le trousseau sécurisé du système (Gestionnaire d'identification
Windows, Trousseau macOS, Secret Service sous Linux), jamais en clair sur le disque.

**Méthodes de secours** (lien « Autres méthodes ») : dans RomM, *Paramètres → Client API
Tokens*, créez un token avec les droits ci-dessous, puis soit cliquez « Pair device » et
tapez le code de 8 caractères (valable 60 s), soit copiez le token et collez-le.

Droits (scopes) demandés : `me.read`, `platforms.read`, `roms.read`, `roms.user.read`,
`roms.user.write` (temps de jeu), `devices.read` (temps de jeu sur tous vos appareils),
`collections.read` et `collections.write` (favoris), `assets.read` et `assets.write`
(sauvegardes). Si vous vous êtes connecté avec une version plus ancienne, le launcher vous
demande de vous reconnecter une fois (*Paramètres → Se déconnecter*).
Détails : [docs/romm-api.md](docs/romm-api.md).

## Pour l'admin : convention côté serveur

Une plateforme RomM par console, dans un dossier qui commence par `portage` :
`portage-n64`, `portage-ngc`, `portage-gbc`… (les plateformes vides sont masquées).

```
roms/portage-<console>/<Nom du jeu>/
├── VERSION.txt   → version du jeu ; le launcher la compare à la version installée
├── ROM/          → ROM(s) copiées à côté de l'exécutable
├── MODS/         → une archive par mod (optionnel)
└── RECOMP/       → une archive par plateforme : <nom>_<Plateforme>.zip
                    Plateforme ∈ Windows, Linux, SteamOS, macOS_ARM, macOS_Intel
```

Les noms des releases d'origine sont aussi reconnus : le launcher repère le système et
l'architecture dans le nom (`windows`/`win32`/`win64`, `linux`, `mac`/`macos` +
`x86_64`/`x64`/`arm64`/`aarch64`/`universal`) et accepte `.zip`, `.AppImage` et `.tar.gz`.
Les archives Android, iOS, consoles, etc. sont ignorées. Si plusieurs archives conviennent,
la convention `_<Plateforme>` gagne, puis (Linux) AppImage > tar.gz > zip.

À éviter : des archives **déjà extraites** dans `RECOMP/`. Seuls les fichiers posés
directement dans `RECOMP/` sont pris en compte, et des milliers de fichiers dans un jeu
ralentissent fortement RomM.

**Boîtes 3D** : le launcher construit la boîte à partir de la face avant, de la tranche et
du dos scrapés par ScreenScraper. Il faut donc `box2d_side` (et idéalement `box2d_back`)
dans `scan.media` du `config.yml` de RomM ; sans tranche, la jaquette s'affiche à plat.

**Bruitages et musiques** : ils ne sont pas dans ce dépôt (droits non redistribuables).
Ils sont servis par un petit conteneur nginx (`farfa-assets`) sur un sous-domaine dédié,
qui n'envoie un fichier qu'aux requêtes portant un token RomM valide (vérifié auprès de
`GET /api/users/me`, résultat gardé 5 min). Adresse par défaut : celle de RomM avec
`assets` à la place du premier niveau (`romm.exemple.fr` → `assets.exemple.fr`, sans
jamais sortir du domaine) ; modifiable dans *Paramètres → Sons*.

```
assets.json                musiques : fichiers et points de boucle
music/*.ogg
sfx/
├── OpeningMenu.wav        ouverture de l'application
├── Click.wav              clic sur un bouton
├── deplacementFocus.wav   survol d'un jeu
├── deplacementOption.wav  survol / focus d'un bouton
├── DownloadEnd.wav        téléchargement terminé
├── LancementGame.wav      lancement d'un jeu
├── retourSelection.wav    sortie de la fiche d'un jeu
└── Error.wav              erreur
```

**Musique** : à chaque lancement, un morceau d'`assets.json` est tiré au sort (jamais le
même deux fois de suite) ; seules ses deux versions sont téléchargées. Version normale dans
la bibliothèque, 8-bit sur la fiche d'un jeu : la version cible repart au même endroit de sa
boucle et les deux se croisent en fondu (0,8 s). Coupée en fondu pendant qu'un jeu tourne.
Réglages dans *Paramètres → Sons*.

Le launcher télécharge les sons une fois et les garde en cache (*Vider le cache* pour
reprendre une nouvelle version). Un son absent, ou un serveur d'assets injoignable, laisse
simplement le launcher muet.

**Sauvegardes** : une archive `farfa-save.zip` par jeu et par utilisateur, rangée par RomM
dans ses sauvegardes sous l'émulateur `farfa-project` (visible dans l'interface web).

Après chaque modification des fichiers, un scan RomM doit être lancé pour que l'API
expose les nouveaux fichiers (le script de mise à jour le fait).

## Choix techniques

**Tauri 2 (Rust) + React + TypeScript + Vite.**

- **Léger** : Tauri utilise le moteur web du système (WebView2 / WebKit) ; l'exécutable
  fait quelques Mo, contre ~100 Mo pour Electron.
- **Sûr** : tout ce qui est sensible (token, réseau, fichiers, lancement de processus)
  vit côté Rust. L'interface web ne voit jamais le token et n'a pas accès au disque.
- **Réutilisation** : l'interface de NieRDeck est en React ; son style est repris et
  adapté à la souris, au clavier et aux grands écrans.
- **Mises à jour signées** intégrées (plugin updater de Tauri) et compilation
  Windows / Linux / macOS depuis GitHub Actions.
- Rust est bien outillé pour le reste du cahier des charges : téléchargement avec reprise,
  extraction zip, trousseau OS (`keyring`), suivi des processus enfants (temps de jeu).

### Arborescence

```
FarfaLauncher/
├── docs/romm-api.md          Étude de l'API RomM 5.4.0
├── src/                      Interface React (TypeScript)
│   ├── api/                  Appels typés vers les commandes Rust
│   ├── screens/              Écrans (connexion, bibliothèque, fiche jeu, paramètres)
│   └── theme/                Thème NieR (CSS, police)
├── src-tauri/                Cœur Rust
│   └── src/
│       ├── commands/         Commandes exposées à l'interface
│       ├── romm/             Client API RomM (auth, jeux, fichiers, temps de jeu)
│       ├── install/          Téléchargement, extraction sans casse, exécutable, état
│       ├── launch/           Lancement et suivi des processus du jeu
│       ├── platform.rs       Détection du système et des archives RECOMP
│       ├── cache.rs          Cache local (métadonnées, images, livrets)
│       ├── secrets.rs        Trousseau OS
│       ├── settings.rs       Paramètres (JSON, sans secret)
│       └── logging.rs        Logs fichier
├── scripts/bump-version.mjs  Changement de version
├── .github/workflows/        CI et publication des releases
└── third_party/              Licences des ressources tierces
```

## Développement

Prérequis : Node.js 20+, Rust stable, et les
[prérequis Tauri](https://tauri.app/start/prerequisites/) de votre OS.

```bash
npm install
npm run tauri dev
```

## Publier une nouvelle version (admin)

```bash
node scripts/bump-version.mjs 0.2.0
git commit -am "chore: version 0.2.0"
git tag v0.2.0
git push && git push origin v0.2.0
```

Le workflow [`release.yml`](.github/workflows/release.yml) vérifie que le tag correspond
aux versions des fichiers, compile Windows / Linux / macOS (Apple Silicon et Intel),
signe les paquets de mise à jour et publie la release avec `latest.json`, que les
launchers installés consultent au démarrage.

### Clé de signature des mises à jour

Les mises à jour sont signées : un launcher n'installe une nouvelle version que si elle
est signée par **votre** clé privée (la clé publique est dans `src-tauri/tauri.conf.json`).

1. **Générer la clé** (une seule fois, sur votre PC) :
   ```bash
   npx tauri signer generate -w ~/.tauri/farfalauncher.key
   ```
   Choisissez un mot de passe. Deux fichiers sont créés hors du dépôt :
   `farfalauncher.key` (privée, **secrète**) et `farfalauncher.key.pub` (publique).
2. **Clé publique** : son contenu va dans `plugins.updater.pubkey` de
   `src-tauri/tauri.conf.json` (elle peut être publique sans risque).
3. **Secrets GitHub** : *Settings → Secrets and variables → Actions → New repository
   secret* :
   - `TAURI_SIGNING_PRIVATE_KEY` : tout le contenu de `farfalauncher.key` ;
   - `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` : le mot de passe.
4. **Ne jamais commiter la clé privée** : elle est hors du dépôt, et le `.gitignore`
   exclut `*.key` par sécurité. Sauvegardez-la (gestionnaire de mots de passe, clé USB) :
   **si vous la perdez, les launchers déjà installés ne pourront plus se mettre à jour**
   (il faudrait que chacun réinstalle manuellement une version avec une nouvelle clé).

## Crédits et licences

- **Police** : [Be Vietnam Pro](https://github.com/bettergui/BeVietnamPro) — SIL Open Font
  License 1.1 (`src/theme/fonts/OFL.txt`).
- **Logo** : Farfaboulette, dessin de l'auteur du projet.
- **[yorha](https://github.com/metakirby5/yorha)** de Ethan Chan — licence MIT
  (`third_party/yorha-LICENSE.txt`) : technique du fond quadrillé et du remplissage
  animé des boutons.
- **[YoRHA-UI-BetterDiscord](https://github.com/AccraZed/YoRHA-UI-BetterDiscord)** :
  palette de couleurs relevée sur ce thème. Ce projet n'ayant pas de licence, **aucun de
  ses fichiers (CSS, icônes, curseurs) n'est redistribué** ici ; les pictogrammes de
  FaRFa project sont des dessins originaux.
- *NieR:Automata* est une marque de Square Enix. Projet de fan non officiel, sans
  aucun asset du jeu.
