# Audit sécurité — FarfaLauncher — tout le dépôt (launcher 1.2.2) — 2026-11-24

Phase 2 du chantier « Farfa project ». Lecture seule : aucun code n'a été corrigé.
Les points sont numérotés **SL1, SL2…** (« sécurité launcher »).

Résumé : **1 critique, 5 élevées, 4 moyennes, 8 faibles.**

Comment lire ce rapport :
- **Où** : le fichier et la ligne à regarder.
- **Qui** : qui peut s'en servir. « Un ami connecté » = quelqu'un qui a un compte RomM.
  « Le gestionnaire compromis » = quelqu'un qui a pris la main sur le NAS, le conteneur
  FarfaRomManager ou ses fiches.
- **Effort** : petit (une heure ou deux), moyen (une journée), gros (plusieurs jours).

Outils passés :
- `npm audit --omit=dev` : 0 vulnérabilité.
- `cargo audit` : pas installé sur ce PC (non installé, comme demandé). À mettre en CI.

---

## Critique

### SL1 — Un rapport de bug peut faire exécuter du code sur le PC du propriétaire, puis signer et publier une mise à jour

- Où :
  - `outils/surveille-rapports/surveille-rapports.ps1:785-792` (dépôt de l'espace de travail) :
    Claude reçoit `Edit`, `Write`, `WebFetch`, `WebSearch` et `cargo test` ;
  - `.github/workflows/test.yml:84-89` : tout tag `test-*` est compilé **avec la clé de signature** ;
  - publication automatique des bêtas (fusion dans `main` puis tag `vX.Y.Z-beta.N`, voir
    `CLAUDE.md` de l'espace de travail, « Publication automatique »).
- Problème :
  - Le texte d'un rapport (description, journaux joints, réponses du joueur) est lu par
    `claude -p`. Un texte piégé peut lui donner des consignes (« injection de consignes »).
  - Claude peut écrire du code puis lancer `cargo test`. Ce code tourne sur le PC du
    propriétaire, avec son compte. Il peut lire `~/.tauri/farfalauncher.key` et
    `Z:/FarfaRomManager/.env`. `WebFetch` peut ensuite envoyer ces données dehors.
  - Le script commite et pousse le tag `test-*` sans relecture humaine du code. La CI
    compile ce code avec la clé de signature dans son environnement.
  - Après le « oui » du joueur, le code part dans `main` et dans une bêta signée, toujours
    sans relecture.
  - L'accord du propriétaire (`valider.bat`) porte sur le texte du rapport, pas sur le code
    produit ni sur le contenu du `rapport.zip`.
- Qui peut l'exploiter, et pour obtenir quoi : un ami connecté (il écrit le rapport et ses
  journaux). Il peut obtenir la clé de signature, le jeton admin RomM, le `GITHUB_TOKEN` et
  les webhooks Discord. Ou une mise à jour signée, envoyée à tous les testeurs bêta.
- Correction proposée :
  1. Relecture humaine du diff avant chaque tag `test-*` et avant chaque fusion dans `main`
     (le script prépare, le propriétaire dit oui).
  2. Retirer `WebFetch` et `WebSearch` des outils de l'analyse.
  3. Faire tourner `claude -p` et `cargo test` sous un compte Windows à part, sans accès à
     `~/.tauri` ni au `.env` (ou dans une machine virtuelle / un conteneur).
  4. Mettre les secrets de signature dans un « Environment » GitHub protégé, avec
     approbation obligatoire (voir SL6).
  - Effort : moyen. Risque pour les joueurs : aucun ; les corrections arrivent un peu moins vite.

---

## Élevée

### SL2 — Le gestionnaire et les recomps suivies sur GitHub peuvent lancer n'importe quel programme chez tous les joueurs

- Où :
  - `src-tauri/src/install/prepare.rs:131-150` (`preparer:`) ;
  - `src-tauri/src/launch/before.rs:52-75` (`avant:`) ;
  - `src-tauri/src/launch/mod.rs:118-149` (`exe:`, `argument:`, variables `env`) ;
  - `src-tauri/src/install/download.rs:229-233` (zip : seule la taille est vérifiée).
- Problème :
  - Le launcher exécute ce que le catalogue lui dit d'exécuter : c'est son rôle.
  - Rien ne prouve que le catalogue et les archives viennent bien du propriétaire. Le SHA-1
    vient du même serveur que l'archive : il prouve que le fichier est entier, pas qu'il est
    honnête.
  - FarfaRomManager prend tout seul la dernière version GitHub de chaque recomp et émulateur.
    Une release piégée par un auteur tiers (ou son compte volé) part donc chez tous les amis
    sans que personne ne la regarde.
- Qui : le gestionnaire compromis, ou l'auteur (ou le compte volé) d'une recomp ou d'un
  émulateur suivi. Il obtient l'exécution de code sur tous les PC et Steam Deck des joueurs.
- Correction proposée :
  - Délai ou validation avant de diffuser une nouvelle version GitHub (par exemple 48 h, ou
    un « oui » du propriétaire sur Discord).
  - Signer le catalogue (fiches + empreintes) avec une clé gardée hors du NAS (minisign).
    Le launcher refuse un catalogue mal signé.
  - Effort : gros. Risque pour les joueurs : faible si le catalogue en cache reste utilisable.

### SL3 — Le gestionnaire reçoit le jeton RomM de chaque joueur

- Où : `src-tauri/src/romm/client.rs:82-90` (`with_base`) ; appels dans `manager.rs:939`,
  `commands/report.rs:35`, `presence.rs:57`, `play.rs:267`, `install/download.rs:72`…
- Problème :
  - Le même jeton RomM sert d'identifiant auprès du gestionnaire et du serveur des assets.
  - La règle L16 l'envoie seulement au même domaine : c'est bien. Mais un conteneur
    FarfaRomManager compromis voit passer le jeton de chaque joueur à chaque requête.
- Qui : le gestionnaire compromis. Il obtient les jetons de tous les joueurs (lecture des
  jeux, écriture des sauvegardes, des collections et des assets de chacun).
- Correction proposée : un jeton propre au gestionnaire. Le launcher l'obtient une fois
  (échange signé par RomM), puis n'envoie plus le jeton RomM qu'à RomM.
  - Effort : moyen (launcher + gestionnaire). Risque : une reconnexion pour chaque joueur.

### SL4 — Les chemins `{appdata}`, `{documents}`… peuvent sortir de leur dossier sous Windows

- Où : `src-tauri/src/language.rs:185-201` (`expand`), utilisé par :
  - `links.rs:33-37` (`lien:`) ;
  - `language.rs:164` (`langue:`, écrit un fichier) ;
  - `report.rs:79-85` (`journaux:`, fichiers joints au rapport) ;
  - `controllers.rs:131` et `:1206` (configuration des manettes) ;
  - `install/emulator.rs:406`, `commands/install.rs:452`.
- Problème :
  - Le contrôle refuse `..` seulement entre des `/`. Sous Windows, `\` sépare aussi les
    dossiers, et un chemin absolu remplace tout.
  - Vérifié avec un petit programme : `{appdata}/..\..\Desktop\a.txt` passe le contrôle et
    donne `…\AppData\Roaming\..\..\Desktop\a.txt`. `{appdata}/C:\Windows\x.ini` donne
    `C:\Windows\x.ini`.
  - Même sans ce contournement, `lien:` accepte n'importe quel dossier sous `{appdata}`, par
    exemple `{appdata}/Microsoft/Windows/Start Menu/Programs/Startup` (dossier Démarrage).
- Qui : une fiche malveillante (gestionnaire compromis). Elle peut :
  - écrire ou remplacer un fichier n'importe où (`langue:`) ;
  - poser un lien sur le dossier Démarrage, donc lancer un programme à chaque ouverture de
    session ;
  - joindre au rapport n'importe quel fichier personnel (`journaux:`).
- Correction proposée :
  - Dans `expand`, refuser `\`, `:` et un début `/` dans la partie après le joker. Après le
    `join`, vérifier que le chemin ne contient que des composants « normaux » et commence
    par la base.
  - Pour `lien:`, une liste de dossiers autorisés (par exemple refuser `Microsoft`,
    `Start Menu`, `.ssh`, `autostart`).
  - Effort : petit. Risque pour les joueurs : aucun pour les fiches actuelles.

### SL5 — Un lien symbolique dans un zip permet d'écrire hors du dossier du jeu (Linux, Steam Deck, Mac)

- Où : `src-tauri/src/install/extract.rs:189-198` et `:272-279` (`make_symlink`).
- Problème :
  - `enclosed_name` bloque `..` et les chemins absolus dans les **noms**. Mais la **cible**
    d'un lien symbolique n'est pas contrôlée.
  - Une archive peut contenir `x -> /home/deck/.config/autostart`, puis un fichier
    `x/piege.desktop`. Le second est écrit à travers le lien, donc hors du dossier.
  - Les liens laissés dans le jeu sont aussi suivis plus tard (`langue:`, manettes, réglages
    optimisés, qui écrivent dans `{exedir}`).
- Qui : toute personne qui contrôle une archive : gestionnaire compromis, recomp GitHub
  piégée, jeu ou mod déposé sur RomM (`*.extract.zip`, mods). Elle obtient l'écriture de
  fichiers dans le dossier personnel (démarrage automatique, `.bashrc`…).
- Correction proposée : refuser un lien dont la cible est absolue ou contient `..`, ou
  vérifier avant chaque écriture que le dossier parent (résolu) reste dans `staging`.
  `tar` le fait déjà (`unpack_in`).
  - Effort : petit. Risque : aucun (les bundles macOS utilisent des liens relatifs internes).

### SL6 — La clé de signature est donnée à des builds qui n'en ont pas besoin, et à des actions non épinglées

- Où :
  - `.github/workflows/ci.yml:99-104` (job `mac-app`, à **chaque push sur n'importe quelle
    branche**) ;
  - `.github/workflows/test.yml:84-89` (chaque tag `test-*`, depuis n'importe quel commit) ;
  - `.github/workflows/release.yml:121-128` ;
  - actions tierces citées par une étiquette mobile, pas par une empreinte :
    `tauri-apps/tauri-action@v0` (reçoit la clé), `Swatinem/rust-cache@v2`,
    `dtolnay/rust-toolchain@stable`, `actions/*@v4`, `actions/github-script@v7`.
- Problème :
  - Tout code poussé sur une branche (y compris `correctif/<rapport>` écrit par Claude, SL1)
    tourne avec `TAURI_SIGNING_PRIVATE_KEY` dans son environnement : `build.rs`, scripts
    `npm`, plugins Vite. Il peut la lire et l'envoyer.
  - Une nouvelle version piégée d'une action reçoit la clé sans changement du dépôt.
  - Les PR venant d'un fork ne reçoivent pas les secrets (comportement GitHub) : ce chemin
    est fermé.
- Qui : toute personne ou tout script qui peut pousser sur le dépôt ; le mainteneur
  (ou le compte volé) d'une action. Il obtient la clé, donc des mises à jour signées
  acceptées par tous les launchers.
- Correction proposée :
  - `mac-app` : construire sans clé (`npx tauri build --bundles app,dmg --config
    '{"bundle":{"createUpdaterArtifacts":false}}'`).
  - Secrets de signature dans un Environment GitHub `signature`, avec approbation du
    propriétaire, limité aux tags `v*` et `test-*`.
  - Épingler les actions par empreinte de commit (Dependabot les tient à jour).
  - Effort : petit. Risque : aucun.

---

## Moyenne

### SL7 — « Installer la version de test » accepte n'importe quel tag `test-*`, même plus ancien

- Où : `src-tauri/src/commands/update.rs:104-127` (`version_comparator(|_, _| true)`, ligne 119) ;
  `.github/workflows/test.yml:13-15`.
- Problème :
  - Le tag vient du gestionnaire (`versionTest.tag` du rapport). Le launcher installe sans
    comparer les versions.
  - `test.yml` compile un tag `test-*` posé sur n'importe quel commit, pas seulement sur une
    branche `correctif/*`.
- Qui : le gestionnaire compromis (il propose une vieille version de test, retour vers une
  faille corrigée) ; quiconque peut pousser un tag (version signée à partir de n'importe
  quel code).
- Correction proposée :
  - Refuser une version de test dont la base (`X.Y.Z`) est plus ancienne que la version
    installée.
  - Vérifier que le tag contient l'identifiant du rapport affiché.
  - Dans `test.yml`, vérifier que le commit du tag appartient à `correctif/*`.
  - Effort : petit.

### SL8 — Le moteur du Steam Deck n'est pas signé

- Où : `.github/workflows/release.yml:150-165` (envoi de `FarfaLauncherDeck-linux-x86_64`) ;
  `NieRDeck/catalogue.py:22-32` (téléchargé depuis la dernière release puis exécuté).
- Problème : le launcher de bureau refuse une mise à jour non signée. Le moteur du Deck
  n'a aucune signature ni empreinte : NieRDeck l'exécute tel quel.
- Qui : quiconque peut écrire dans une release GitHub (compte volé, jeton GitHub avec ce
  droit, par exemple celui du `.env` du gestionnaire s'il l'a). Il obtient l'exécution de
  code sur tous les Steam Deck.
- Correction proposée : signer le binaire avec la clé minisign (fichier `.sig` joint), et
  le vérifier dans NieRDeck avec la clé publique déjà dans `tauri.conf.json`.
  - Effort : moyen (deux dépôts).

### SL9 — Les installateurs GOG sont lancés avec les droits administrateur

- Où : `src-tauri/src/pc/gog.rs:197-237` (`run_elevated`, verbe `runas`) ;
  `src-tauri/src/pc/install.rs:280-293`.
- Problème : un `setup_*.exe` venu de RomM demande l'UAC. Le joueur a l'habitude de cliquer
  « Oui ».
- Qui : quiconque peut déposer un jeu « Ordinateur » sur RomM (ou un RomM compromis). Il
  obtient les droits administrateur sur le PC du joueur.
- Correction proposée : lancer d'abord sans élévation, avec `/CURRENTUSER` (Inno Setup) dans
  le dossier « Jeux » ; ne demander l'UAC qu'en cas d'échec, avec un message clair.
  - Effort : petit.

### SL10 — Le rapport de bug peut joindre des fichiers personnels, et l'anonymisation est partielle

- Où : `src-tauri/src/report.rs:55-63` et `:79-92` (`journaux:` de la fiche) ;
  `report.rs:122-136` (`anonymize`).
- Problème :
  - La fiche choisit les fichiers joints : jusqu'à 5 par motif, 2 Mo chacun, sous
    `{documents}`, `{appdata}`, `{config}`… Sous Linux sans trousseau, le jeton est dans
    `{config}/fr.farfaboulette.farfalauncher/token.json` : une fiche peut le joindre.
  - L'anonymisation remplace le dossier personnel et le nom du compte. Le nom du PC, un
    dossier « OneDrive - Prénom Nom » ou un autre disque (`D:\…`) restent.
  - La liste des fichiers est bien montrée au joueur avant l'envoi (`reportFiles`), mais
    seulement leur nom.
- Qui : le gestionnaire compromis (fuite de fichiers et du jeton) ; sinon, simple fuite
  d'informations personnelles vers le NAS et vers l'analyse.
- Correction proposée : n'accepter dans `journaux:` que le dossier du jeu et des dossiers
  connus de l'émulateur ; refuser tout fichier du dossier de réglages du launcher ;
  anonymiser aussi `COMPUTERNAME` / `hostname`.
  - Effort : petit.

---

## Faible

### SL11 — Le jeton est lisible par les jeux et émulateurs

- Où : `src-tauri/src/secrets.rs:17-41` (trousseau), `secrets.rs:84-120` (repli Linux),
  `src-tauri/src/deck.rs:159-201` (fichier `token` du Deck).
- Problème : les jeux tournent avec le compte du joueur. Le Gestionnaire d'identification
  Windows et les fichiers 0600 se lisent par n'importe quel programme de ce compte. Les
  scopes du flux normal sont limités (`romm/auth.rs:11-22`), mais un jeton **collé** à la
  main peut avoir plus de droits (admin).
- Qui : un jeu ou émulateur piégé. Il obtient le jeton du joueur (celui d'un admin s'il a
  collé un jeton admin).
- Correction proposée : à la connexion par jeton collé, refuser ou signaler un jeton qui a
  des scopes d'écriture admin. Effort : petit.

### SL12 — `open_url` ouvre toute adresse « dépôt » du catalogue, sans exiger https

- Où : `src-tauri/src/commands/mod.rs:40-50`, `src-tauri/src/manager.rs:922-924`.
- Problème : un `repo:` de fiche en `file://…` ou `ms-…:` serait passé au système au clic.
- Qui : le gestionnaire compromis (il faut un clic du joueur).
- Correction : n'ouvrir que `https://`. Effort : petit.

### SL13 — Proton GE, umu et les clients CoD sont pris sans empreinte fixée

- Où : `src-tauri/src/pc/proton.rs:196-202` et `:228-245` ; `src-tauri/src/pc/cod.rs:33`
  (branche `main` de CBServers, empreintes dans le même manifeste).
- Problème : seul HTTPS protège ces fichiers. GE publie un `.sha512sum` qui n'est pas vérifié.
- Qui : un compte GitHub tiers compromis.
- Correction : vérifier le `.sha512sum` de GE ; épingler une version d'umu. Effort : petit.

### SL14 — Les archives du gestionnaire ne sont pas vérifiées par leur SHA-1

- Où : `src-tauri/src/install/download.rs:229-233`.
- Problème : pour un zip, seule la taille est contrôlée (RomM hache le contenu décompressé).
  Mais le gestionnaire sert `files/<sha1>` du fichier lui-même : la vérification serait possible.
- Correction : vérifier le SHA-1 pour les fichiers du gestionnaire. Effort : petit.

### SL15 — HTTP en clair accepté sur le réseau local

- Où : `src-tauri/src/romm/client.rs:36-58` (`normalize_server_url`, `is_local_host`).
- Problème : sur un Wi-Fi partagé ou une LAN, le jeton circule en clair.
- Correction : afficher un avertissement quand l'adresse est en `http://`. Effort : petit.

### SL16 — Le protocole `asset:` voit tout le dossier de cache de l'application

- Où : `src-tauri/tauri.conf.json:28-33` (`$APPCACHE/**`).
- Problème : sous Windows, ce dossier contient aussi les journaux et les données du WebView.
  Ce n'est utile qu'en cas de faille dans l'interface (aucune trouvée).
- Correction : `$APPCACHE/cache/**`. Effort : petit.

### SL17 — Le vrai domaine et l'IP du NAS sont dans le dépôt public

- Où : `src-tauri/src/assets.rs:12-13`, `:55-56`, `:64-65` ; `src-tauri/src/romm/client.rs:388-393`.
- Problème : contraire à la règle du dépôt (« jamais d'URL de serveur »). Le domaine indique
  où attaquer RomM et le gestionnaire.
- Correction : `romm.exemple.synology.me` et `192.0.2.x` dans les tests. Le domaine reste dans
  l'historique : c'est peu grave, il est déjà public par le DNS. Effort : petit.

### SL18 — Droits du jeton GitHub trop larges dans les workflows

- Où : `.github/workflows/ci.yml` (pas de `permissions:` : droits par défaut du dépôt) ;
  `release.yml:23-24` et `test.yml:17-18` (`contents: write` pour tous les jobs, tests compris).
- Correction : `permissions: contents: read` en tête ; `contents: write` seulement sur les jobs
  qui créent et publient la release. Vérifier dans GitHub : Settings → Actions → « Read
  repository contents permission » par défaut. Effort : petit.

---

## Info (vérifié, sain)

- **Dépendances** : `npm audit --omit=dev` = 0 vulnérabilité. Tauri 2.12.5, plugin-updater
  2.14.0, opener 2.7.2, reqwest 0.12.28 (rustls), zip 9.0.5, tar 0.4.46, keyring 3.6.3,
  pdfjs-dist 6.4.299 (faille CVE-2024-4367 corrigée depuis 4.2.67).
- **Secrets** : rien dans les fichiers suivis ni dans l'historique (238 commits) : recherche
  de `rmm_…`, webhooks Discord, clés privées, `ghp_`/`github_pat_`, mots de passe, `.env`,
  `.key`. Seuls des exemples factices (`rmm_x`, `rmm_xxxxxxxx`).
- **Mise à jour du bureau** : paquets signés (clé publique dans `tauri.conf.json`). Le
  `latest.json` n'est pas signé, mais un paquet non signé est refusé. Pas de retour à une
  version plus ancienne sur les canaux stable et bêta (comparaison de versions par défaut).
- **Jeton (L16)** : envoyé seulement au même domaine enregistrable que RomM (liste publique
  des suffixes, `synology.me` compris, test `token_only_goes_to_the_same_site`). Un 401 du
  gestionnaire ne déconnecte pas. reqwest retire `Authorization` sur une redirection vers un
  autre hôte. Pas de `danger_accept_invalid_certs`.
- **Jeton hors des journaux, URL et environnements** : aucune trace du jeton dans `tracing` ;
  les erreurs n'affichent que le chemin de l'URL ; `FARFALAUNCHERDECK_JETON` est retiré de
  l'environnement des jeux (`launch/mod.rs:131`, `deck.rs:170-173`) ; `token.json` et `token`
  en 0600.
- **Archives** : zip par `enclosed_name` (pas de `..` ni de chemin absolu dans les noms),
  tar par `unpack_in`/`unpack` (contrôle du dossier), sauvegardes par `enclosed_name`.
  `rel_to_path` retire `..` et les lettres de lecteur ; `effacer:` refuse `..`.
- **Interface** : CSP stricte (`script-src 'self'`, pas d'`eval`, `object-src 'none'`), pas
  de `dangerouslySetInnerHTML`. Capabilities minimales : ni `fs`, ni `shell`, ni `opener`
  côté JavaScript. `open_url` et `image_path` filtrent les adresses.
- **Commandes** : tout est lancé par `Command` avec des arguments séparés, sans `cmd /c` ni
  PowerShell. Rust 1.99 (faille « BatBadBut » des `.bat` corrigée depuis 1.77.2).
- **Workflows** : pas de `pull_request_target` ; secrets non donnés aux PR de forks ;
  `smoke.yml` en `contents: read` avec `-Discret` (ni compte ni noms de jeux).

---

## Contrôles à ajouter en CI

- **`cargo audit`** (ou `cargo deny check advisories sources`) : failles connues des crates
  Rust, crates retirées, sources non prévues.
- **`npm audit --omit=dev --audit-level=high`** : failles des paquets de l'interface.
- **gitleaks** (ou trufflehog) sur chaque push : un jeton, un webhook ou une clé commité
  par erreur.
- **zizmor** + **actionlint** : workflows trop permissifs, actions non épinglées, secrets
  exposés, injections dans `run:` (SL6, SL18).
- **Dependabot** pour les actions GitHub (épinglées par empreinte) et les dépendances.
- **Test « pas d'adresse réelle »** : échec si `poupounas`, `192.168.` ou `synology.me` (hors
  exemple) apparaît dans un fichier suivi (SL17).
- **Tests unitaires à ajouter** :
  - `language::expand` refuse `\`, `C:\…`, `\…` et `..` sous toutes leurs formes (SL4) ;
  - extraction d'un zip contenant un lien vers l'extérieur suivi d'un fichier dessous : rien
    n'est écrit hors du dossier (SL5) ;
  - `open_url` refuse une adresse non `https` (SL12).
- **Garde dans `test.yml`** : le commit du tag `test-*` doit appartenir à une branche
  `correctif/*` (SL7).
