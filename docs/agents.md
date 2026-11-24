# Les agents Claude du launcher

Des sous-agents Claude Code prêts à l'emploi, rangés dans `.claude/agents/`. Chacun a un rôle,
des outils limités, des interdits et un format de rapport. Ils servent aux phases 2 (sécurité)
et 3 (nouvelles fonctions) du chantier « Farfa project », et à tout travail de code ensuite.

Les mêmes rôles existent dans FarfaRomManager (`docs/agents.md` de ce dépôt), adaptés au
gestionnaire.

## Les agents

| Agent | À quoi il sert | Peut modifier ? |
|---|---|---|
| `developpeur` | Implémente une tâche dans sa copie de travail et sa branche : un commit par point, tests ajoutés, code commenté en français. | oui, sa copie seulement, sans pousser |
| `relecteur` | Relit une branche avant fusion : diff, tests, CI, pertes de données, compatibilité avec les launchers des amis, parité Windows/Linux/Deck. Verdict **OK**, **OK avec réserves** ou **À corriger**. | non |
| `testeur` | Tests de bout en bout contre les vrais serveurs, sans rien y écrire : smoke test, essai réel d'un jeu (`farfalauncherdeck essai`), puis nettoyage du disque. | non |
| `securite` | Audit de sécurité (dépendances, secrets, jeton RomM, mise à jour signée, HTTPS, téléchargements, commandes lancées…), rapport classé par gravité. | non |
| `product` | Propose des fonctions (utilité, difficulté, risques) et un lot conseillé de 3. Ne code pas. | non |

## Comment les utiliser

Dans une session Claude Code ouverte dans le dépôt (ou une de ses copies de travail), il
suffit de le demander en clair :

- « Utilise l'agent **relecteur** sur la branche `phase2-jeton` (copie
  `FarfaLauncher-jeton`). »
- « Utilise l'agent **developpeur** pour corriger S3 de `docs/securite.md`, dans la copie
  `FarfaLauncher-s3`, branche `phase2-s3`. »
- « Utilise l'agent **testeur** sur la branche `phase3-favoris`, avec un jeu NES. »
- « Utilise l'agent **securite** pour auditer tout le launcher. »
- « Utilise l'agent **product** pour proposer le prochain lot de fonctions. »

La commande `/agents` de Claude Code liste les agents disponibles.

## L'organisation

```
            propriétaire  (choisit, teste, autorise fusion et publication)
                 │
           orchestrateur  (la session Claude principale)
     ┌───────────┼───────────┐
 developpeur  developpeur  developpeur     ← 3 au plus en même temps,
 (copie A,    (copie B,    (copie C,          chacun sa copie et sa branche
  branche A)   branche B)   branche C)
     └───────────┼───────────┘
             relecteur      ← un agent distinct, avant chaque fusion
                 │
              testeur       ← bout en bout, avant de faire tester le propriétaire
```

1. **L'orchestrateur** découpe le travail en tâches qui ne touchent pas les mêmes fichiers.
2. Pour chaque tâche, il crée une copie de travail sur une nouvelle branche, partie de
   `origin/main`, sans jamais changer de branche dans `FarfaLauncher` :
   ```bash
   cd C:/Users/Farfa/Documents/CLAUDE/FarfaLauncher
   git fetch
   git worktree add ../FarfaLauncher-<tache> -b <branche> origin/main
   cmd //c mklink /J "C:\Users\Farfa\Documents\CLAUDE\FarfaLauncher-<tache>\node_modules" "C:\Users\Farfa\Documents\CLAUDE\FarfaLauncher\node_modules"
   ```
3. Il lance **au plus 3** agents `developpeur` en parallèle, un par copie.
4. Chaque branche terminée passe par un agent **`relecteur` distinct** : un agent ne valide
   jamais son propre travail. « À corriger » → retour au développeur, puis nouvelle relecture.
5. Les branches relues « OK » sont fusionnées par l'orchestrateur dans la branche de la
   phase, puis l'agent `testeur` fait le bout en bout.
6. **Le propriétaire** teste lui-même la version de test (`target-test/debug/farfalauncher.exe`).
   Fusion dans `main`, tag, release et annonce Discord : seulement avec son accord, et
   seulement si tous les tests passent.
7. Les copies de travail inutiles sont retirées (`git worktree remove ../FarfaLauncher-<tache>`,
   après avoir vérifié qu'il ne reste rien à garder).

### Phase 2 : sécurité

- `securite` audite tout le dépôt. L'orchestrateur écrit son rapport dans `docs/securite.md`
  (classé par gravité).
- Des `developpeur` corrigent, gravité la plus haute d'abord, 3 à la fois.
- `relecteur` relit chaque correction.
- Les contrôles de sécurité proposés par l'audit (`npm audit`, `cargo audit`, recherche de
  secrets…) sont ajoutés à la CI (`.github/workflows/`), eux aussi relus.

### Phase 3 : nouvelles fonctions

- `product` propose des fonctions et un lot de 3 ; le propriétaire choisit.
- 3 `developpeur` en parallèle, un par fonction.
- Pour chaque branche : `relecteur`, puis `securite` en revue ciblée de la branche.
- Enfin `testeur`, puis le propriétaire teste.

## Règles communes à tous les agents

- Tout en français : code commenté, commits, rapports, messages.
- Jamais afficher un secret : `.env` du gestionnaire, `~/.tauri/farfalauncher.key` (jamais
  lue), jeton RomM. Le dépôt est public : ni jeton ni URL de serveur dedans.
- Rien de destructif sur le NAS ni sur RomM.
- Pas de push, de fusion, de tag ni de release sans l'accord du propriétaire.
- Jamais de changement de branche dans un dépôt principal.
- Un seul dossier de build : `FarfaLauncher/src-tauri/target-test`, et vider
  `target-test/debug/incremental` après.
- `python` n'existe pas : `py -3`.
- Ne pas changer l'apparence d'un écran que la tâche ne demande pas de changer.
- Parité Windows / Linux (AppImage) / Steam Deck, ou prévenir le propriétaire.
- Commits terminés par `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`.
