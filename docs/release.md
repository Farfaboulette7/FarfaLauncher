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
   (bêta) donne une pré-release.
4. **Compilation** : installateur Windows (`_x64-setup.exe`), AppImage Linux, `.dmg` Mac, et
   le moteur du Steam Deck `FarfaLauncherDeck-linux-x86_64`. Chaque paquet est signé avec la
   clé (§10).
5. **Publication** : le brouillon devient public, avec `latest.json`. Ce fichier est aussi
   copié dans la release technique `canal-beta`.

Ensuite :

- **stable** : tous les launchers la voient au démarrage (ou dans les 30 min) et proposent
  la mise à jour ;
- **bêta** : seuls les launchers du canal bêta la voient ; les autres restent sur la
  dernière stable.

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
compile Windows et AppImage et publie une pré-release, **sans toucher** à la dernière
release ni au canal bêta. Le joueur voit « Installer la version de test » dans son
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
| GitHub, *Settings → Secrets and variables → Actions* | `TAURI_SIGNING_PRIVATE_KEY` (contenu du fichier) et `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` |

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
dans `tauri.conf.json`, remplacer les deux secrets GitHub, publier. Mais **aucun launcher déjà
installé ne pourra plus se mettre à jour** : chaque ami devra télécharger et installer la
nouvelle version à la main depuis la page des releases.

## 11. Si la release échoue

Onglet **Actions** → le run « Release » en rouge → le job avec une croix → l'étape en rouge.
Lecture d'une erreur : [tests.md §3](tests.md).

| Ce qui a échoué | État | Que faire |
|---|---|---|
| **tests** | rien n'a été créé | Corriger sur `main`, puis reposer le tag (ci-dessous). |
| **Vérifier que le tag correspond** | rien n'a été créé | Le numéro des fichiers n'est pas celui du tag : refaire §5, puis reposer le tag. |
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
