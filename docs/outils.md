# Outils à double-cliquer (sans IA)

Des outils pour faire soi-même, sans Claude, les gestes courants du launcher. Ils sont dans
`scripts\`. Chacun est un `.bat` à **double-cliquer** : une ligne par étape (**OK** en vert,
**ATTENTION** en jaune, **ÉCHEC** en rouge), puis la fenêtre attend une touche. Rien ne part
sur GitHub ni sur Discord sans taper **`oui`**. Aucun secret n'est affiché.

Les outils du gestionnaire et du NAS (vérifier la santé, redémarrer, déployer, sauvegarder)
sont dans l'autre dépôt : FarfaRomManager, `outils\` et `docs/outils.md`.

## Tableau

| Outil | À quoi ça sert | Quand l'utiliser | Ce qu'il fait exactement | En cas d'échec |
|---|---|---|---|---|
| `scripts\smoke-test.bat` | Vérifier que toute la chaîne marche : RomM, gestionnaire, et le vrai code du launcher (connexion, liste, installation, lancement d'un petit jeu SNES) | Après une mise à jour de RomM, avant une publication, ou si les amis disent que rien ne marche | **Ne fait que lire** côté serveurs. Compile le moteur sans interface dans `src-tauri\target-test`, installe et lance un jeu 10 s dans `%TEMP%\farfa-smoke`, puis efface tout | Tableau des 15 étapes dans [`docs/smoke-test.md`](smoke-test.md) |
| `scripts\publier.bat` | Publier une nouvelle version du launcher pour tous les joueurs | Seulement **après avoir testé soi-même** la version de test | Voir [Publier](#publier-une-version) : main propre et à jour, type de version, `bump-version.mjs`, notes, tsc + cargo test, commit et tag, **« oui »**, push, annonce Discord | Voir [Publier](#en-cas-déchec) |

Options utiles (ouvrir une invite de commandes dans `scripts\`) :

- `smoke-test.bat -ApiSeulement` : seulement la partie réseau (30 s, sans compiler) ;
  autres options dans `docs/smoke-test.md` ;
- `publier.bat -Essai` : fait les vérifications (dont tsc et cargo test) et montre tout ce qui
  serait fait, **sans rien changer ni envoyer** ;
- `publier.bat -Type correctif` (ou `mineure`, `majeure`) : évite la question du type.

## Publier une version

Toujours depuis le dossier du launcher sur la branche `main` (le dossier principal
`FarfaLauncher`). Étapes de `publier.bat` :

1. **Branche `main`, sans modification en cours, à jour** (`git pull`). Le réglage local
   `[profile.dev]` de `src-tauri\Cargo.toml` est toléré : il n'est jamais commité. Le support des
   rapports de bug publie parfois lui-même des bêtas sur `main` : `git pull` les récupère.
2. **Type de version** : `1` correctif (+0.0.1, par défaut, Entrée), `2` mineure (+0.1.0),
   `3` majeure (+1.0.0). Depuis une bêta `X.Y.Z-beta.N`, « correctif » donne la stable `X.Y.Z`.
3. **`node scripts/bump-version.mjs X.Y.Z`** : le numéro dans `package.json`,
   `package-lock.json`, `tauri.conf.json`, `Cargo.toml` et `Cargo.lock`.
4. **Notes de version** : le bloc-notes s'ouvre sur `docs\changelog\vX.Y.Z.md` (un modèle
   « Nouveautés / Corrections » s'il n'existe pas). Écrire pour les joueurs, en français,
   enregistrer, **fermer le bloc-notes**. Tant qu'il reste « (à remplir…) », il se rouvre.
   Ces notes sont le texte de la release GitHub et de l'onglet « Nouvelles versions ».
5. **Vérifications** : `npx tsc --noEmit -p .` puis `cargo test` (dans `src-tauri\target-test`,
   1 à 10 minutes ; le cache `debug\incremental` est vidé après).
6. **Commit « Version X.Y.Z »** des fichiers de version et des notes seulement (jamais
   `git commit -am`), puis **tag `vX.Y.Z`**.
7. **Taper « oui »**, puis `git push origin main` et `git push origin vX.Y.Z`. GitHub compile la
   version (`release.yml`, 20 à 40 min) ; les launchers des amis se mettent à jour seuls.
8. **Annonce Discord** dans le salon farfalauncher (webhook `DISCORD_LAUNCHER` lu dans
   `Z:\FarfaRomManager\.env`, jamais affiché) : titre, notes, et « **Disponible dans un petit
   moment** ». Elle part tout de suite, sans attendre la compilation.
9. **Lien de suivi** : https://github.com/Farfaboulette7/FarfaLauncher/actions/workflows/release.yml

Les **bêtas** (`X.Y.Z-beta.N`, seulement pour ceux qui ont coché « Recevoir les versions de
test ») ne passent pas par cet outil : le support des rapports les publie lui-même, ou voir
le `CLAUDE.md` du launcher.

### En cas d'échec

Au premier ÉCHEC, tout s'arrête et **rien n'est parti** sur GitHub ni Discord. Si le numéro de
version avait déjà été changé, il est remis tout seul ; les notes déjà écrites sont gardées et
resservent au prochain essai.

| Ligne | Que faire |
|---|---|
| Branche main | Ouvrir le bon dossier (`FarfaLauncher`, branche `main`). Ne pas changer de branche dans un dossier où une autre session travaille. |
| Aucune modification en cours | Commiter (ou annuler) les fichiers listés, puis relancer. |
| À jour avec GitHub (`git pull`) | Conflit avec les commits de GitHub : à régler à la main (ou avec Claude). |
| Nouvelle version | Le tag existe déjà : choisir un autre type, ou vérifier sur GitHub que cette version n'est pas déjà publiée. |
| Dépendances de l'interface | Lancer `npm install` dans le dossier du launcher. |
| Types TypeScript (tsc) | Erreur dans `src\` : le message donne `fichier.tsx(ligne,colonne)`. Corriger, commiter, relancer. |
| Tests Rust (cargo test) | Un test échoue ou le code ne compile pas : voir `docs/tests.md`, « Lire un échec ». |
| Envoi de main | GitHub a reçu d'autres commits entre-temps : `git pull`, puis `git push origin main` et `git push origin vX.Y.Z`, puis l'annonce Discord à la main. |
| Envoi du tag | Relancer `git push origin vX.Y.Z`. |
| Annonce Discord (ATTENTION) | La version est publiée ; faire l'annonce à la main dans le salon farfalauncher. |

Si on répond autre chose que « oui » à l'envoi : le commit et le tag restent sur ce PC. Le
script affiche les commandes pour publier plus tard, ou pour tout annuler
(`git tag -d vX.Y.Z` puis `git reset --keep HEAD~1`).

Si la compilation sur GitHub échoue (croix rouge) : rien n'est publié aux joueurs. Voir
`docs/tests.md`, « Release ou version de test bloquée par les tests ».

## Comment c'est fait

- `scripts\publier.ps1` : tout le travail, commenté étape par étape (UTF-8 avec BOM pour
  Windows PowerShell 5.1).
- `scripts\indexer-version-cargo.mjs` : met dans le commit **seulement** la ligne `version` de
  `src-tauri\Cargo.toml`, à partir du dernier commit, sans toucher au fichier sur le disque (le
  réglage local `[profile.dev]` y reste).
- `scripts\bump-version.mjs` : change le numéro partout (le même outil remet l'ancien numéro en
  cas d'échec).
