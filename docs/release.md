# Publier une version du launcher

Pas à pas, pour publier seul. Toutes les commandes se tapent dans **Git Bash**, dans le dossier
`C:\Users\Farfa\Documents\CLAUDE\FarfaLauncher` (branche `main`).

Trois sortes de versions :

| Sorte | Tag | Qui la reçoit | Qui la publie |
|---|---|---|---|
| **stable** | `vX.Y.Z` | tous les launchers | vous (ce document) |
| **bêta** | `vX.Y.Z-beta.N` | ceux qui ont coché « Recevoir les versions de test » | vous, ou la surveillance des rapports (§9) |
| **test d'un rapport** | `test-<rapport>-<n>` | le joueur qui a signalé le problème | la surveillance des rapports (§9) |

## 0. Avant tout : tester

Ne publiez qu'une version que vous avez essayée vous-même. Construire la version de test :

```bash
npm run build && cd src-tauri && CARGO_TARGET_DIR=target-test cargo build --features tauri/custom-protocol
rm -rf target-test/debug/incremental
cd ..
```

Puis lancer `src-tauri\target-test\debug\farfalauncher.exe` et essayer ce qui a changé.
« Accès refusé » pendant la construction : l'exe de test est encore ouvert, fermez-le.

## 1. Récupérer ce que la surveillance a publié

La surveillance des rapports publie parfois des bêtas toute seule (§9). Votre `main` local
peut donc être en retard sur GitHub.

```bash
git fetch
git log --oneline main..origin/main
```

Si la deuxième commande affiche des lignes, récupérez-les :

```bash
git pull
```

Puis regardez la version actuelle :

```bash
node -p "require('./package.json').version"
```

## 2. Choisir le numéro

- Par défaut : **+0.0.1** sur la dernière version publiée (1.2.1 → 1.2.2).
- Beaucoup de nouveautés : +0.1.0 (1.2.1 → 1.3.0). Un changement énorme : +1.0.0. À décider
  vous-même ; un agent IA doit vous le demander.
- Après des bêtas : la stable reprend le numéro des bêtas sans le suffixe
  (1.2.2-beta.3 → 1.2.2).
- Une bêta : `X.Y.Z-beta.N` (1.2.2-beta.1, puis 1.2.2-beta.2…).

Les tags déjà pris : `git tag --list "v1.2*"`.

## 3. Écrire les notes de version

Créer `docs/changelog/vX.Y.Z.md` (ou `docs/changelog/vX.Y.Z-beta.N.md`). C'est le texte de la
release sur GitHub, et ce que les joueurs lisent dans le launcher (Messages → Nouvelles
versions, fenêtre de mise à jour). En français, pour un joueur, sans jargon :

```markdown
Téléchargez le fichier correspondant à votre système ci-dessous. Si FaRFa project est déjà installé, il se met à jour tout seul.

## Nouveautés
- **Titre court** : ce que le joueur peut faire maintenant.

## Corrections
- **Le problème** : ce qui est réglé.
```

Pour une bêta, première ligne : « Version de test : vous la recevez parce que « Recevoir les
versions de test » est coché dans Paramètres → À propos. ». Exemples : les fichiers existants
de `docs/changelog/`.

Sans ce fichier, la release a un texte générique.

## 4. Vérifier

```bash
npx tsc --noEmit -p .
cd src-tauri && CARGO_TARGET_DIR=target-test cargo test --lib
rm -rf target-test/debug/incremental
cd ..
```

Attendu : `tsc` n'écrit rien, `cargo test` finit par `test result: ok`. Sinon, voir
[tests.md](tests.md). GitHub refait ces tests (§6), mais un échec ici vous fait gagner 15 min.

## 5. Changer le numéro, commiter, taguer

Si vous avez des changements de code pas encore commités, **commitez-les d'abord**, dans un
commit à part (`git add <fichier>`, jamais `-am`) : le commit de version ne doit contenir que
le numéro et les notes.

```bash
node scripts/bump-version.mjs 1.2.2
```

Le script met le numéro dans `package.json`, `package-lock.json`, `src-tauri/tauri.conf.json`,
`src-tauri/Cargo.toml` et `src-tauri/Cargo.lock`, puis affiche les commandes suivantes.

**Ne jamais faire `git commit -am`** : `src-tauri/Cargo.toml` contient chez vous un réglage
local (`[profile.dev]`, builds plus légers) qui ne doit pas partir sur GitHub. On indexe
fichier par fichier :

```bash
git add package.json package-lock.json src-tauri/tauri.conf.json src-tauri/Cargo.lock docs/changelog/v1.2.2.md
git add -p src-tauri/Cargo.toml
```

`git add -p` montre chaque morceau modifié de `Cargo.toml` : répondez **`y`** pour la ligne
`version = "1.2.2"`, **`n`** pour le bloc `[profile.dev]`. Vérifiez :

```bash
git diff --cached --stat
git diff --cached src-tauri/Cargo.toml
```

Le second ne doit montrer **que** la ligne `version`. Puis :

```bash
git commit -m "Version 1.2.2"
git tag v1.2.2
git push origin main
git push origin v1.2.2
```

## 6. Ce que fait GitHub (`release.yml`)

Le push du tag lance le workflow « Release » (onglet **Actions** du dépôt). Compter 20 à
30 minutes.

1. **Tests** (`tests.yml`, Windows et Linux) : types, build de l'interface, tests Rust. S'ils
   échouent, **rien n'est créé ni publié**.
2. **Vérification** : le tag doit correspondre au numéro des fichiers (sinon : vous avez
   oublié `bump-version.mjs` ou le commit).
3. **Release en brouillon**, avec `docs/changelog/<tag>.md` comme texte. Un tag avec `-`
   (bêta) donne une pré-release. Le moteur du Steam Deck `FarfaLauncherDeck-linux-x86_64`
   est compilé en même temps (il n'a pas besoin de la clé).
4. **Attente de votre approbation** : le run passe en jaune, « Waiting », et GitHub vous
   envoie un e-mail. **Vous devez approuver** la signature, sinon rien ne se publie
   (§6 bis).
5. **Compilation signée** (job `build`) : installateur Windows (`_x64-setup.exe`), AppImage
   Linux, `.dmg` Mac. Chaque paquet est signé avec la clé (§10).
6. **Publication** : le brouillon devient public, avec `latest.json`. Ce fichier est aussi
   copié dans la release technique `canal-beta`.

Ensuite :

- **stable** : tous les launchers la voient au démarrage (ou dans les 30 min) et proposent
  la mise à jour ;
- **bêta** : seuls les launchers du canal bêta la voient ; les autres restent sur la
  dernière stable.

## 6 bis. Approuver la signature

La clé de signature est rangée dans l'environnement GitHub **`signature`**. Seul le job
`build` de `release.yml` et de `test.yml` y a accès, et seulement après **votre**
approbation. Pourquoi : les bêtas et les versions de test sont aussi lancées par la
surveillance des rapports, avec du code écrit par Claude d'après le message d'un joueur,
que personne n'a relu. Un code piégé pourrait voler la clé pendant la compilation et
signer de fausses mises à jour, que tous les launchers accepteraient. Votre approbation
est le moment où un humain relit.

**Approuver veut dire : « j'ai relu ce qui a changé et il n'y a que ce que j'attends ».**
Si vous n'avez pas le temps de relire, n'approuvez pas : le run peut attendre.

Pas à pas :

1. Onglet **Actions** du dépôt → le run en jaune (« Release » ou « Version de test »,
   statut *Waiting*). Les tests sont déjà passés (coche verte sur `tests`). S'ils sont
   rouges, il n'y a rien à approuver.
2. Regarder **ce qui a changé** :
   - en haut du run, cliquer le numéro du commit (7 caractères) : GitHub montre son diff ;
   - mieux, tout ce qui a changé depuis la version précédente :
     `https://github.com/Farfaboulette7/FarfaLauncher/compare/<version d'avant>...<ce tag>`
     (par exemple `.../compare/v1.2.2...v1.2.3-beta.1`). Pour une version de test :
     `.../compare/main...<tag test-…>`.
3. Ce qu'il faut regarder en priorité (un piège se cache surtout là) :
   - **tout fichier hors de `src/` et `src-tauri/src/`** : `build.rs`, `Cargo.toml`,
     `Cargo.lock`, `package.json`, `package-lock.json`, `vite.config.*`, `.github/`,
     `scripts/`, `.cargo/`, `tauri.conf.json`. Un correctif d'un rapport n'a presque jamais
     besoin d'y toucher : au moindre doute, **refuser** ;
   - une nouvelle dépendance (Cargo ou npm), surtout inconnue ;
   - une adresse internet, un `curl`, `Command::new`, `std::env::var`, du texte encodé
     (base64, longues suites de chiffres) qui n'ont rien à voir avec la correction ;
   - le reste : la correction doit correspondre au rapport (petite, à l'endroit attendu).
4. Si tout va bien : bouton **Review deployments** → cocher `signature` → **Approve and
   deploy**. Une seule approbation libère tous les systèmes du run.
5. Sinon : **Review deployments** → cocher `signature` → **Reject**. Le run échoue, le
   brouillon reste invisible des joueurs (le supprimer depuis la page Releases), et la
   surveillance des rapports passe le rapport « à décider ».

Le statut *Waiting* d'un run qui attend votre approbation est normal : la surveillance des
rapports le reconnaît et attend aussi.

## 7. Annonce Discord (stable)

**Juste après le push du tag**, sans attendre la compilation. Dans le salon `farfalauncher`,
en disant que la version sera **disponible dans un petit moment**. Pas d'annonce pour une
bêta, sauf si vous le voulez.

Le plus simple : écrire le message vous-même dans Discord. Pour l'envoyer comme les annonces
habituelles (encadré, par le webhook du salon), créez `annonce.json` (Bloc-notes, encodage
UTF-8) **hors du dépôt**, par exemple dans `C:\Users\Farfa\Documents` :

```json
{"embeds": [{
  "title": "FaRFa project 1.2.2",
  "url": "https://github.com/Farfaboulette7/FarfaLauncher/releases/tag/v1.2.2",
  "description": "Nouvelle version disponible dans un petit moment : le launcher vous la proposera tout seul.\n\n**Nouveautés**\n- …\n\n**Corrections**\n- …",
  "color": 5763719
}]}
```

Puis dans Git Bash (le webhook est lu dans une variable, jamais affiché) :

```bash
W=$(grep ^DISCORD_LAUNCHER= /z/FarfaRomManager/.env | cut -d= -f2- | tr -d '\r')
curl -s -H "Content-Type: application/json" --data-binary @/c/Users/Farfa/Documents/annonce.json "$W"
```

Rien ne s'affiche si c'est bon.

## 8. Après la publication

- **Rapports de bug corrigés** par cette version : le joueur doit le voir dans *Messages →
  Mes signalements* (`corrigeDans`, statut « corrigé », un message). **Ne jamais modifier
  `infos.json` à la main** : le gestionnaire l'écrit aussi, et une modification à la main peut
  l'abîmer ou effacer un message du joueur. Utiliser `infos.py` (`poser`, `statut`,
  `message`) comme expliqué dans la section « Gestes à la main sur une publication » de
  [`outils/surveille-rapports/INSTALLER.md`](https://github.com/Farfaboulette7/FarfaRomManager/blob/master/outils/surveille-rapports/INSTALLER.md)
  du dépôt FarfaRomManager.
- **Steam Deck** : NieRDeck télécharge seul le nouveau moteur. Si vous publiez aussi
  NieRDeck, mettez son `package.json` au même numéro que le launcher.
- Vérifier que la release est bien publique :
  https://github.com/Farfaboulette7/FarfaLauncher/releases

## 9. Versions de test et bêtas automatiques (surveillance des rapports)

La surveillance des rapports publie seule dans deux cas. Son code est dans le dépôt
FarfaRomManager (`outils/surveille-rapports/`) ; la copie en service sur ce PC est
`C:\Users\Farfa\Documents\CLAUDE\outils\surveille-rapports\surveille-rapports.ps1`, lancée par
la tâche Windows « FaRFa - surveille rapports » toutes les 3 min.
Fonctionnement complet côté serveur :
[runbook du gestionnaire](https://github.com/Farfaboulette7/FarfaRomManager/blob/master/docs/runbook.md).

**Version de test d'un rapport.** Quand le support a préparé une correction (copie de
travail `FarfaLauncher-correctifs\<rapport>`, branche `correctif/<rapport>` partie de
`origin/main`), le script pousse le tag `test-<rapport>-<n>`. `test.yml` lance les tests,
**attend votre approbation** (§6 bis : relire le diff du correctif), compile Windows et
AppImage et publie une pré-release, **sans toucher** à la dernière release ni au canal bêta.
Le launcher n'installe une version de test que si elle vient de sa release `test-…` du
dépôt officiel et n'est pas plus ancienne que la version en service. Le joueur voit « Installer la version de test » dans son
signalement. Si la compilation échoue, le rapport passe « à décider ».

**Bêta automatique.** Quand le joueur confirme que la version de test règle son problème,
la branche est mise en file. Puis, **une seule à la fois**, et jamais pendant qu'un
`release.yml` tourne :

1. copie de travail `FarfaLauncher-correctifs\_publication` partie de `origin/main` ;
2. fusion de `correctif/<rapport>` (conflit : rien n'est poussé, les admins sont prévenus) ;
3. numéro suivant : `X.Y.Z-beta.N` → `X.Y.Z-beta.(N+1)`, ou `X.Y.Z` → `X.Y.(Z+1)-beta.1` ;
4. `bump-version.mjs`, notes `docs/changelog/vX.Y.Z-beta.N.md` tirées du rapport ;
5. `tsc` et `cargo test` (échec : rien n'est poussé, les admins sont prévenus) ;
6. commit « Version … », push de `main` puis du tag ; la branche du correctif est supprimée ;
   `release.yml` attend alors **votre approbation** avant de compiler et signer (§6 bis) ;
7. le rapport passe « corrigé » avec `corrigeDans`, le joueur et les admins sont prévenus.

Conséquence : **faites toujours `git fetch` et `git pull` avant de publier** (§1).

Essayer une publication sans rien pousser (le résultat reste dans
`FarfaLauncher-correctifs\_publication`) :

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File C:\Users\Farfa\Documents\CLAUDE\outils\surveille-rapports\surveille-rapports.ps1 -EssaiPublication <rapport>
```

## 10. La clé de signature : ne JAMAIS la perdre

Les launchers installés n'acceptent une mise à jour que si elle est signée par **votre** clé
privée. La clé publique correspondante est dans `src-tauri/tauri.conf.json`
(`plugins.updater.pubkey`).

| Où | Quoi |
|---|---|
| `C:\Users\Farfa\.tauri\farfalauncher.key` | la clé privée (secrète) |
| votre mémoire / gestionnaire de mots de passe | son mot de passe |
| GitHub, *Settings → Environments → `signature` → Environment secrets* (**pas** dans *Secrets and variables → Actions*) | `TAURI_SIGNING_PRIVATE_KEY` (contenu du fichier) et `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` |

GitHub ne rend **jamais** un secret : si le fichier disparaît de ce PC, il est perdu. Gardez
donc deux copies hors du PC, dès aujourd'hui :

1. dans un **gestionnaire de mots de passe** (Bitwarden, KeePass…) : une entrée
   « FarfaLauncher signature » avec le fichier `farfalauncher.key` en pièce jointe et le mot de
   passe ;
2. sur une **clé USB** rangée chez vous (copier le fichier `farfalauncher.key`).

Jamais dans le dépôt (il est public ; `.gitignore` exclut `*.key` par sécurité), ni dans un
message, ni sur un partage du NAS accessible à d'autres.

**Si elle est perdue** : générer une nouvelle clé
(`npx tauri signer generate -w ~/.tauri/farfalauncher.key`), mettre la nouvelle clé publique
dans `tauri.conf.json`, remplacer les deux secrets de l'environnement `signature`, publier. Mais **aucun launcher déjà
installé ne pourra plus se mettre à jour** : chaque ami devra télécharger et installer la
nouvelle version à la main depuis la page des releases.

## 11. Si la release échoue

Onglet **Actions** → le run « Release » en rouge → le job avec une croix → l'étape en rouge.
Lecture d'une erreur : [tests.md §3](tests.md).

| Ce qui a échoué | État | Que faire |
|---|---|---|
| **tests** | rien n'a été créé | Corriger sur `main`, puis reposer le tag (ci-dessous). |
| **Vérifier que le tag correspond** | rien n'a été créé | Le numéro des fichiers n'est pas celui du tag : refaire §5, puis reposer le tag. |
| **build** en jaune, *Waiting* | brouillon, invisible des joueurs | Ce n'est pas un échec : il attend votre approbation (§6 bis). |
| **build** refusé (*Rejected*) | brouillon, invisible des joueurs | Supprimer le brouillon, corriger, reposer le tag. |
| **build** rouge dès le début, « TAURI_SIGNING_PRIVATE_KEY » vide ou clé introuvable | brouillon | Les secrets ne sont pas dans l'environnement `signature` (§12). |
| **build** ou **deck** (compilation) | une release **brouillon** existe, invisible des joueurs | Erreur passagère (réseau, `apt-get`, `npm ci`) : bouton **Re-run failed jobs**. Vraie erreur : supprimer le brouillon (page Releases → le brouillon → *Delete*), corriger, reposer le tag. |
| **publish-release** | brouillon complet | **Re-run failed jobs**. |

Reposer le tag sur le commit corrigé :

```bash
git tag -d v1.2.2
git push origin :refs/tags/v1.2.2
git tag v1.2.2
git push origin v1.2.2
```

Tant que la release n'est pas publique, aucun launcher ne voit rien : rien ne presse. Si
l'annonce Discord est déjà partie et que la correction prend du temps, un petit message dans
le salon suffit (« un peu de retard »).

**Une mise à jour publiée pose problème** chez les joueurs : publiez vite une version
corrective (+0.0.1). On ne « dépublie » pas une release : les launchers déjà mis à jour ne
reviennent pas en arrière seuls.

## 12. À faire par le propriétaire : l'environnement `signature` (une seule fois)

À faire au moment de fusionner la branche qui ajoute `environment: signature` aux workflows
(phase 2, faille SG3). Tout se fait sur github.com, connecté avec votre compte. **Les valeurs
des secrets ne s'affichent nulle part et ne doivent être collées que dans les cases de
GitHub.** Disponible pour un dépôt public avec un compte gratuit (vérifié dans la doc
GitHub : les « required reviewers » et les secrets d'environnement des comptes gratuits
sont réservés aux dépôts **publics**, ce qui est le cas ici).

**A. Créer l'environnement : AVANT la fusion (obligatoire, ne casse rien)**

Attention : si un workflow cite un environnement qui n'existe pas encore, GitHub le crée
tout seul, **sans aucune protection**. Il faut donc le créer avant que la branche arrive
dans `main`.

1. Dépôt FarfaLauncher → **Settings** → menu de gauche **Environments** → **New
   environment**. Nom : `signature` (exactement, en minuscules) → **Configure environment**.
2. **Deployment protection rules** :
   - cocher **Required reviewers**, et ajouter votre compte `Farfaboulette7` ;
   - **ne pas** cocher *Prevent self-review* : les tags sont poussés avec votre compte (par
     vous ou par la surveillance des rapports), cette option vous empêcherait d'approuver ;
   - décocher **Allow administrators to bypass configured protection rules** (si la case
     existe) : ainsi rien ne passe sans une vraie approbation ;
   - pas de *Wait timer* ;
   - **Save protection rules**.
3. **Deployment branches and tags** → choisir **Selected branches and tags** → **Add
   deployment branch or tag rule** :
   - *Ref type* : **Tag**, motif `v*.*.*` → **Add rule** ;
   - recommencer : *Ref type* : **Tag**, motif `test-*` → **Add rule**.
   Ainsi, même un workflow modifié sur une branche ne peut pas demander la clé.

**B. Ranger les deux secrets dans l'environnement (étape 4 avant ou après la fusion, étape 5
juste après la fusion)**

4. Toujours dans l'environnement `signature`, partie **Environment secrets** → **Add
   environment secret** :
   - nom `TAURI_SIGNING_PRIVATE_KEY`, valeur : le contenu du fichier
     `C:\Users\Farfa\.tauri\farfalauncher.key` (l'ouvrir avec le Bloc-notes, Ctrl+A, Ctrl+C,
     coller dans la case, fermer le Bloc-notes **sans enregistrer**) ;
   - nom `TAURI_SIGNING_PRIVATE_KEY_PASSWORD`, valeur : le mot de passe de la clé (depuis votre
     gestionnaire de mots de passe).
5. **Settings** → **Secrets and variables** → **Actions** → onglet *Secrets*, partie
   **Repository secrets** : **supprimer** `TAURI_SIGNING_PRIVATE_KEY` et
   `TAURI_SIGNING_PRIVATE_KEY_PASSWORD`. C'est l'étape qui protège vraiment : tant qu'ils
   restent ici, n'importe quel job peut les lire sans approbation.

**C. Vérifier**

6. Au prochain tag (`test-…` de la surveillance ou `v…`), le run doit s'arrêter en jaune
   (*Waiting*) sur `build`, avec le bouton **Review deployments**. Approuver (§6 bis) : la
   compilation doit réussir et la release se publier. Si `build` échoue tout de suite en
   parlant de la clé, refaire l'étape 4 (nom exact, collé en entier).
7. La CI d'une branche (`ci.yml`, job `mac-app`) doit rester verte : elle n'utilise plus la
   clé du tout.

Ordre conseillé : A, puis 4, puis la fusion dans `main`, puis 5 tout de suite, puis C.
Avant la fusion, les secrets de l'environnement ne servent à rien (les anciens workflows ne
le citent pas) et ne gênent pas. Si vous faites l'étape 5 **avant** la fusion, les
publications échouent (sans danger) jusqu'à la fusion.
