# Smoke test du Farfa project

Un contrôle rapide, de bout en bout, contre les **vrais** serveurs : RomM, le gestionnaire
(FarfaRomManager) et le code du launcher. Il répond à une seule question : « est-ce que la
chaîne marche encore ? » (connexion, liste des jeux, téléchargement, lancement).

Il ne fait **que lire** : aucune session de jeu, aucune sauvegarde, aucun favori, aucune
présence, aucune statistique n'est envoyée. Il marche sans Claude.

Deux façons de l'utiliser :

| | Où | Ce qui est vérifié |
|---|---|---|
| `scripts\smoke-test.bat` | ce PC, à la main | tout : API RomM, gestionnaire, puis le vrai code du launcher qui installe et lance un petit jeu |
| `.github/workflows/smoke.yml` | GitHub, chaque matin | la partie réseau seulement (pas de lancement) : prévient si une mise à jour de RomM casse un appel du launcher |

## Lancer le test sur ce PC

1. Avoir lancé le launcher au moins une fois et s'y être connecté : le test reprend son
   serveur (`settings.json`) et son jeton (trousseau Windows, « Gestionnaire
   d'identification »). Le jeton n'est jamais affiché ni écrit dans un fichier.
2. Double-cliquer sur `scripts\smoke-test.bat`.
3. Lire les lignes : une par étape, puis le résumé. Compter 1 à 3 minutes (la compilation
   du moteur est plus longue la première fois).

Le test compile le moteur sans interface du launcher (`farfalauncherdeck`, le même code que
le launcher et le Steam Deck) dans `src-tauri\target-test`, sans cache incrémental (le
disque ne se remplit pas). Il faut donc Rust (`cargo`). Il travaille dans
`%TEMP%\farfa-smoke`, supprimé à la fin : réglages, journaux, émulateur et jeu de test. Les
jeux et réglages du vrai launcher ne sont pas touchés.

Ce qui peut rester après un test :
- `%TEMP%\farfa-smoke-journal` : journal du moteur, seulement en cas d'échec ;
- `%TEMP%\farfa-smoke-cargo.log` : sortie de cargo, seulement si la compilation échoue.

Le test n'efface qu'un dossier marqué par un fichier `.farfa-smoke` (créé par lui) : un
dossier `farfa-smoke` qui existe déjà sans ce fichier n'est jamais effacé, le test s'arrête.

Options (à ajouter après `smoke-test.bat`, ou à `smoke-test.ps1`) :

| Option | Effet |
|---|---|
| `-Console nes` | console du jeu installé et lancé (slug RomM, `snes` par défaut : Snes9x pèse 5 Mo). Pas de Switch (voir plus bas) |
| `-RomId 1234` | jeu précis à installer et lancer |
| `-Secondes 20` | durée pendant laquelle le jeu doit tenir (10 par défaut) |
| `-SansCompilation` | utilise le moteur déjà compilé |
| `-Dossier D:\Essais` | crée `farfa-smoke` (et `farfa-smoke-journal`) dans ce dossier au lieu de `%TEMP%` |
| `-ApiSeulement` | seulement la partie réseau (étapes 1 à 10) |
| `-Discret` | n'affiche ni noms de jeux ni nom de compte |

Autre jeton ou autre serveur : variables `FARFA_SMOKE_TOKEN`, `FARFA_SMOKE_URL` et
`FARFA_SMOKE_MANAGER` (par défaut `manager.<domaine du serveur>`).

### Exemple de sortie

```
[OK       ]  2. RomM répond (GET /api/heartbeat)  RomM 5.4.1 (0,3 s)
...
[OK       ] 15. Launcher : lancement (10 s, sans rien envoyer au serveur)  le jeu a tenu 10 s, fenêtre affichée, puis a été fermé (10,7 s)

================ Résumé ================
OK : 15   Attention : 0   Échecs : 0   Sautées : 0
Tout marche.
```

États : **OK** ; **ATTENTION** (ça marche, mais quelque chose a changé : à regarder) ;
**ÉCHEC** ; **SAUTÉ** (une étape dont elle dépend a échoué : corriger d'abord celle-là).

## Les étapes, et que faire en cas d'échec

Toujours commencer par le **premier** échec : les suivants en découlent souvent.

| # | Étape | Si ÉCHEC ou ATTENTION |
|---|---|---|
| 1 | Paramètres | « serveur inconnu » ou « jeton introuvable » : ouvrir le launcher et s'y connecter (ou définir `FARFA_SMOKE_TOKEN`). |
| 2 | RomM répond (`/api/heartbeat`) | « serveur injoignable » : NAS éteint, conteneur RomM arrêté, reverse proxy ou certificat. Ouvrir RomM dans le navigateur. ATTENTION « RomM x.y » : RomM a changé de version mineure : lancer tout le test, relire les notes de version de RomM. |
| 3 | Connexion (`/api/users/me`) | HTTP 401/403 : jeton expiré ou révoqué (se reconnecter dans le launcher). « champs absents » : RomM a changé l'objet utilisateur, le launcher ne pourra plus se connecter : à corriger dans `romm/auth.rs`. |
| 4 | Plateformes (`/api/platforms`) | Champs absents : `romm/library.rs` (`RawPlatform`). « aucune plateforme snes » : choisir une autre console avec `-Console`. |
| 5 | Une page de jeux (`/api/roms`) | Champ obligatoire absent (`id`, `platform_id`, `fs_name`, `full_path`, `updated_at`) : **la bibliothèque du launcher ne se charge plus** ; corriger `RawRom` dans `romm/library.rs`. ATTENTION « champs disparus » : jaquettes, résumés, captures… vont manquer ; trouver le nouveau nom dans `<serveur>/api/docs`. |
| 6 | Détail du jeu (`/api/roms/{id}`) | Champs des fichiers absents : plus aucun téléchargement possible (`RawFile`). ATTENTION sur `sha1_hash`/`md5_hash` : l'intégrité n'est plus vérifiée. |
| 7 | Téléchargement partiel d'une ROM | HTTP 404 : la route `files/content` a changé (`romm/library.rs`, `file_content_path`). ATTENTION « Range ignoré » : les téléchargements marchent, mais ne reprennent plus après une coupure (souvent le reverse proxy). |
| 8 | Gestionnaire (`/health`) | Gestionnaire arrêté : `touch Z:/FarfaRomManager/redemarrer`, puis vérifier `started`. Sinon, ses journaux Docker. |
| 9 | Catalogue (`/api/catalog`) | 401 : le gestionnaire ne valide plus le jeton auprès de RomM (`/api/users/me`, voir étape 3). Champs absents : fiches ou format du catalogue changés (`manager.rs`). |
| 10 | Fichier du gestionnaire (`/files/{sha1}`) | 404 : fichier du catalogue absent du NAS (fiche à revalider). |
| 11 | Dossier de travail et compilation | « pas créé par le smoke test » : un dossier `farfa-smoke` existe déjà sans marqueur ; le vider ou le renommer à la main. Sinon, lire `%TEMP%\farfa-smoke-cargo.log`. Ou relancer avec `-SansCompilation`. |
| 12 | Launcher : connexion | Le code du launcher n'accepte plus le serveur alors que les étapes 2 et 3 passent : comparer avec `romm/auth.rs`. |
| 13 | Launcher : liste des jeux | Pas d'émulateur pour la console sur ce PC (fiche du gestionnaire), ou réponse de RomM que le launcher ne sait plus lire. |
| 14 | Launcher : installation | Téléchargement ou extraction en échec : le journal du moteur est gardé dans `%TEMP%\farfa-smoke-journal`. |
| 15 | Launcher : lancement | Erreur « not_available » : jeu refusé par `essai` (jeu PC, recomp ou Switch) : choisir une autre console. « fermé avant N s » : l'émulateur plante (nouvelle version ? fiche ?). ATTENTION « pas de fenêtre vue » : il tourne mais rien ne s'affiche. Journal dans `%TEMP%\farfa-smoke-journal`. |

En cas d'échec, le journal du moteur est copié dans `%TEMP%\farfa-smoke-journal` (sans
le jeton). Le supprimer une fois lu.

## Le test quotidien sur GitHub

`.github/workflows/smoke.yml` refait les étapes 1 à 10 chaque matin (et à la demande :
onglet **Actions → Smoke test RomM → Run workflow**). Un échec rend le workflow rouge, et
GitHub envoie un courriel au propriétaire du dépôt (réglage par défaut des notifications
« Actions »). Les journaux étant publics, le test y tourne en mode `-Discret`.

Sans les secrets ci-dessous, le workflow s'arrête en vert avec un avertissement « Secrets
absents ».

### Mettre en place (une fois)

1. **Créer un compte RomM dédié**, en lecture seule. Dans RomM, avec le compte admin :
   *Administration → Utilisateurs → Ajouter*, un nom comme `smoke-test`, rôle **Viewer**,
   mot de passe long (gardé dans le gestionnaire de mots de passe). Il apparaîtra dans la
   liste des joueurs des amis : c'est normal.
2. **Créer son jeton.** Se connecter à RomM avec ce compte, puis *Paramètres → Client API
   Tokens → Créer* :
   - nom : `GitHub smoke test` ;
   - scopes : seulement **`me.read`**, **`platforms.read`**, **`roms.read`** (rien en
     écriture) ;
   - expiration : 1 an (penser à le renouveler ; un jeton expiré fait échouer l'étape 3).
   Copier le jeton `rmm_…` affiché.
3. **Le ranger dans GitHub.** Sur le dépôt : *Settings → Secrets and variables → Actions →
   New repository secret* :
   - `ROMM_SMOKE_TOKEN` : le jeton ;
   - `ROMM_SMOKE_URL` : l'adresse publique de RomM (`https://…`) ;
   - `MANAGER_SMOKE_URL` (facultatif) : l'adresse du gestionnaire, si ce n'est pas
     `manager.<domaine>`.
   Les secrets ne sont jamais affichés dans les journaux (GitHub les masque).
4. Lancer une fois à la main (*Actions → Smoke test RomM → Run workflow*) et vérifier que
   tout est vert.

Révoquer le jeton : dans RomM, *Paramètres → Client API Tokens* du compte `smoke-test`, ou
désactiver le compte.

À savoir : GitHub met en pause les workflows planifiés d'un dépôt public sans activité
depuis 60 jours. Un courriel prévient ; il suffit de le réactiver dans l'onglet Actions.

## Comment c'est fait

- `scripts/smoke-test.ps1` : un seul script, commenté, qui marche avec Windows PowerShell
  5.1 (ce PC) et PowerShell 7 (GitHub). Les appels HTTP passent par `HttpClient` ; les
  téléchargements partiels demandent 1 Ko (`Range`) et n'en lisent jamais plus.
- Les champs contrôlés sont ceux que le launcher lit (`romm/library.rs`, `romm/auth.rs`,
  `manager.rs`) : un champ obligatoire absent est un ÉCHEC, un champ facultatif absent une
  ATTENTION.
- Les étapes 11 à 15 utilisent le vrai code du launcher, par le moteur
  `farfalauncherdeck` (`src-tauri/src/deck.rs`) :
  - le jeton lui est passé par la variable `FARFALAUNCHERDECK_JETON` (jamais écrit sur le
    disque) ;
  - au démarrage, le moteur retire cette variable de son environnement, et `launch::spawn`
    la retire aussi : les émulateurs et jeux n'en héritent pas ;
  - la commande `essai <id> <secondes>` lance le jeu comme `jouer`, mais sans rien envoyer :
    - seulement un jeu d'émulateur hors Switch : ainsi `play::start` ne prépare ni jeu PC
      (client Call of Duty), ni partage des shaders de Citron et Eden (qui peut envoyer au
      gestionnaire). Le reste est refusé (`not_available`) ;
    - `play::start` avec `SaveChoice::Unchanged` : ni téléchargement ni envoi de
      sauvegarde ;
    - ni `play::finish` (temps de jeu, sauvegarde, shaders), ni `stats::sync`, ni la
      présence. Le jeu est ensuite prié de se fermer, puis arrêté.
- Le jeu est choisi sur la première page de la console : le premier de moins de 5 Mo, de
  préférence fait d'un seul fichier.
