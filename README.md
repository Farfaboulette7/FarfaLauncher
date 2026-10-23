# FarfaLauncher

Launcher desktop pour installer, mettre à jour et lancer en un clic les jeux recomp /
portages PC natifs hébergés sur un serveur [RomM](https://romm.app) (plateforme
`farfa-portage`), avec l'identité visuelle YoRHa de *NieR:Automata*.

> 🚧 En développement. État actuel : connexion au serveur RomM.

## Pour mes amis : installation

*(Section complétée à la première release.)*

## Se connecter

1. Au premier lancement, saisissez l'adresse de votre serveur RomM (celle que vous ouvrez
   dans votre navigateur).
2. FarfaLauncher affiche un code et un bouton **« Ouvrir RomM »**. Connectez-vous à RomM
   dans la page qui s'ouvre, vérifiez que le code est le même, puis cliquez **Approve**.
   C'est tout : le launcher se connecte seul, et l'appareil apparaît dans votre compte RomM
   (vous pouvez le révoquer à tout moment).

Le token est rangé dans le trousseau sécurisé du système (Gestionnaire d'identification
Windows, Trousseau macOS, Secret Service sous Linux), jamais en clair sur le disque.

**Méthodes de secours** (lien « Autres méthodes ») : dans RomM, *Paramètres → Client API
Tokens*, créez un token avec les droits ci-dessous, puis soit cliquez « Pair device » et
tapez le code de 8 caractères (valable 60 s), soit copiez le token et collez-le.

Droits (scopes) demandés : `me.read`, `platforms.read`, `roms.read`, `roms.user.read`,
`roms.user.write` (temps de jeu), `devices.read` (temps de jeu sur tous vos appareils).
Détails : [docs/romm-api.md](docs/romm-api.md).

## Pour l'admin : convention côté serveur

```
roms/farfa-portage/<Nom du jeu>/
├── VERSION.txt   → version du jeu ; le launcher la compare à la version installée
├── ROM/          → ROM(s) copiées à côté de l'exécutable
├── MODS/         → une archive par mod (optionnel)
└── RECOMP/       → une archive par plateforme : <nom>_<Plateforme>.zip
                    Plateforme ∈ Windows, Linux, SteamOS, macOS_ARM, macOS_Intel
```

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
│       ├── romm/             Client API RomM (auth, jeux, fichiers)
│       ├── secrets.rs        Trousseau OS
│       ├── settings.rs       Paramètres (JSON, sans secret)
│       └── logging.rs        Logs fichier
└── third_party/              Licences des ressources tierces
```

## Développement

Prérequis : Node.js 20+, Rust stable, et les
[prérequis Tauri](https://tauri.app/start/prerequisites/) de votre OS.

```bash
npm install
npm run tauri dev
```

## Crédits et licences

- **Police** : [Be Vietnam Pro](https://github.com/bettergui/BeVietnamPro) — SIL Open Font
  License 1.1 (`src/theme/fonts/OFL.txt`).
- **[yorha](https://github.com/metakirby5/yorha)** de Ethan Chan — licence MIT
  (`third_party/yorha-LICENSE.txt`) : technique du fond quadrillé et du remplissage
  animé des boutons.
- **[YoRHA-UI-BetterDiscord](https://github.com/AccraZed/YoRHA-UI-BetterDiscord)** :
  palette de couleurs relevée sur ce thème. Ce projet n'ayant pas de licence, **aucun de
  ses fichiers (CSS, icônes, curseurs) n'est redistribué** ici ; les pictogrammes de
  FarfaLauncher sont des dessins originaux.
- *NieR:Automata* est une marque de Square Enix. Projet de fan non officiel, sans
  aucun asset du jeu.
