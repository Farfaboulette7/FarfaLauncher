# Dépannage du launcher (runbook)

Problèmes que les joueurs rencontrent avec FaRFa project, et quoi faire. Pour ce qui se passe
côté serveur (NAS, RomM, gestionnaire arrêté, rapports de bug) :
[runbook du gestionnaire](https://github.com/Farfaboulette7/FarfaRomManager/blob/master/docs/runbook.md).

**Toujours commencer par :**

1. Le joueur fait **« Signaler un problème »** (onglet *Communauté* du jeu) : le rapport part
   avec ses journaux et arrive dans `Z:\FarfaRomManager\data\reports\`.
2. Sinon, il ouvre ses journaux : *Paramètres → Cache et dépannage → Ouvrir les logs*, et vous
   envoie le fichier du jour.
3. Si le problème touche tout le monde : lancer le smoke test (§3).

## 1. Où sont les journaux

Un fichier par jour : `farfalauncher.AAAA-MM-JJ.log`. Aucun jeton n'y est écrit.

| Système | Dossier |
|---|---|
| Windows | `%LOCALAPPDATA%\fr.farfaboulette.farfalauncher\logs\` |
| Linux (AppImage) | `~/.local/share/fr.farfaboulette.farfalauncher/logs/` |
| Steam Deck, launcher en mode Bureau | comme Linux |
| Steam Deck, onglet « Farfa project » de NieRDeck (moteur `farfalauncherdeck`) | `~/.config/farfalauncherdeck/logs/` |
| Steam Deck, plugin NieRDeck lui-même | `~/homebrew/logs/NieRDeck/` (dossier standard de Decky) |
| Mac | `~/Library/Logs/fr.farfaboulette.farfalauncher/` |

Les réglages et l'état des jeux sont à côté : `%APPDATA%\fr.farfaboulette.farfalauncher\`
(Windows), `~/.config/fr.farfaboulette.farfalauncher/` (Linux). Détail :
[architecture.md §6](architecture.md#6-stockage-local).

Lignes utiles à chercher dans un journal : `ERROR`, `WARN`, `illisible`, `lien`, `403`, `401`.

## 2. Problèmes et solutions

### Le joueur ne peut pas se connecter

| Message | Cause | Solution |
|---|---|---|
| « Aucun serveur RomM n'a répondu à cette adresse » | adresse fausse, ou NAS / RomM arrêté | Vérifier l'adresse (celle de RomM dans le navigateur, avec `https://`). L'ouvrir dans un navigateur. Si elle ne répond pas : runbook du gestionnaire. |
| « l'adresse du serveur doit commencer par https:// » | `http://` hors réseau local | Mettre l'adresse `https://`. |
| « Ce serveur ne semble pas être un serveur RomM » | adresse d'un autre service (gestionnaire…) | Mettre l'adresse de RomM. |
| « Le serveur refuse l'accès à votre compte (erreur 403) » | droits du compte, ou proxy qui bloque | Le jeton est gardé. Se reconnecter. Si ça continue : vérifier le compte dans RomM (*Administration → Utilisateurs*) et le reverse proxy du NAS. |
| « Votre session a expiré ou votre accès a été retiré » | jeton révoqué ou expiré dans RomM | Se reconnecter. |
| « Cette fonction demande une autorisation de plus » | jeton créé par une vieille version | *Paramètres → Compte → Se déconnecter*, puis se reconnecter. |
| « Ce code d'appairage est invalide ou a expiré » | code de 8 caractères plus vieux que 60 s | En générer un nouveau dans RomM. |
| « Impossible d'accéder au trousseau sécurisé du système » | voir « Linux sans trousseau » plus bas | |

Le plus simple reste le bouton **« Ouvrir RomM »** de l'écran de connexion : le joueur se
connecte à RomM dans son navigateur, vérifie que le code est le même et clique **Approve**.

### Linux sans trousseau

Le jeton est normalement rangé dans le Secret Service (KWallet sur Steam Deck et KDE,
GNOME Keyring sur GNOME). Certaines distributions n'en ont pas, ou il est verrouillé.

- **Depuis la phase 1** (version après 1.2.1) : le launcher range alors le jeton dans
  `~/.config/fr.farfaboulette.farfalauncher/token.json`, lisible par le seul utilisateur
  (droits 0600). Le journal le dit : `trousseau indisponible : token rangé dans token.json`.
  Rien à faire.
- **Version 1.2.1 ou plus ancienne** : la connexion échoue. Mettre à jour le launcher
  (télécharger la dernière AppImage), ou installer et ouvrir un trousseau (`gnome-keyring` ou
  `kwalletmanager`), puis se reconnecter.
- Se déconnecter (*Paramètres → Compte*) efface aussi `token.json`.

### Le launcher dit qu'il est hors ligne

Le launcher interroge RomM (`/api/heartbeat`) toutes les 60 s, toutes les 20 s quand il est
hors ligne, et se reconnecte seul au retour.

- Hors ligne, la bibliothèque vient du cache, les jeux installés restent jouables. Les
  sauvegardes et le temps de jeu sont mis en file et partent au retour du serveur.
- On ne peut pas installer : le gestionnaire et RomM doivent répondre.
- Tout le monde hors ligne : NAS, RomM ou reverse proxy en panne (runbook du gestionnaire).
  Un seul joueur : sa connexion, un antivirus ou un pare-feu.

### L'installation échoue

| Message | Solution |
|---|---|
| « Espace disque insuffisant » / « Le disque est plein » | Libérer de la place, ou changer le dossier des jeux (*Paramètres → Installation*). |
| « Téléchargement impossible » | Connexion coupée ou serveur injoignable. Réessayer : le téléchargement reprend où il s'était arrêté. |
| « Le fichier téléchargé est corrompu » | Réessayer. Si ça se répète pour tout le monde : le fichier est abîmé sur le serveur (fiche à revalider côté gestionnaire). |
| « L'archive du jeu est illisible ou endommagée » | Même chose : fichier abîmé sur le serveur. |
| « Impossible d'écrire dans le dossier d'installation » | Dossier des jeux supprimé, disque débranché, ou dossier protégé (Windows : éviter `Program Files`). |
| « Ce jeu n'est pas disponible pour votre système » | Pas de build pour ce système (souvent une recomp Windows sous Linux : pas de Proton pour les recomps). La fiche du jeu donne la raison si la fiche du gestionnaire en a une (`systemes-manquants:`). |
| « Une installation de ce jeu est déjà en cours » | Attendre, ou l'annuler dans les téléchargements. |
| « Ce jeu 3DS doit être déchiffré, mais les clés… manquent » / « est abîmé sur le serveur » | Problème côté serveur : voir le gestionnaire (clés 3DS, ROM à remplacer). |
| Installation d'un jeu GOG « échouée ou annulée dans la fenêtre de l'installateur » | Relancer et laisser l'installateur aller au bout. |

Le détail exact (adresse, code HTTP) est toujours dans le journal.

### Le jeu ne se lance pas

1. **« Impossible de lancer le jeu. Essayez de choisir un autre exécutable »** : sur la fiche
   du jeu, *Choisir l'exécutable*, et prendre le bon `.exe` (ou binaire Linux).
2. **« Aucun exécutable n'a été trouvé dans ce jeu »** : l'archive ne contient pas ce que le
   launcher attend ; désinstaller puis réinstaller. Si ça continue : la fiche du
   gestionnaire (`exe:`) est à corriger.
3. **Windows** : l'antivirus a pu mettre l'exe en quarantaine (ajouter une exception pour le
   dossier des jeux), ou un pilote graphique trop ancien.
4. **Linux, jeu PC** : il passe par Proton. « Impossible de préparer Proton » : connexion, ou
   choisir une autre version de Proton dans les réglages du jeu. Le premier lancement est
   long (préparation du préfixe).
5. **Linux, la partie s'arrête tout de suite** pour le launcher alors que le jeu tourne : limite
   connue L8 (un jeu qui se détache de son groupe de processus). Le temps de jeu et l'envoi de
   la sauvegarde partent trop tôt. À signaler avec le nom du jeu.
6. **« Ce jeu est déjà en cours d'exécution »** : un ancien processus du jeu tourne encore.
   Le fermer (Gestionnaire des tâches, ou redémarrer).
7. Émulateur qui plante ou réglages étranges : *Signaler un problème*, le rapport contient les
   journaux de l'émulateur.

### Sauvegarde en conflit

Au lancement, si la sauvegarde de cet appareil et celle du serveur sont différentes, le
launcher demande laquelle garder (cet appareil par défaut).

- **Garder celle de cet appareil** : elle sera envoyée sur le serveur à la fin de la partie.
- **Prendre celle du serveur** : la sauvegarde locale est d'abord copiée dans
  `<dossier du jeu>\.farfalauncher\backups\` (les 5 dernières copies sont gardées).
- **Jouer sans synchroniser** : rien n'est comparé ni envoyé pour cette partie.

Pour revenir en arrière :

- copie locale : recopier le contenu d'un dossier de `.farfalauncher\backups\` dans le dossier
  des sauvegardes du jeu, launcher fermé ;
- copie du serveur : sur la fiche du jeu, *Historique des sauvegardes…* puis *Restaurer*
  (seulement les jeux dont la sauvegarde est gardée par le gestionnaire ; RomM ne garde que la
  dernière). La sauvegarde actuelle passe d'abord dans l'historique.

Le dossier des sauvegardes se change sur la fiche du jeu (*Dossier des sauvegardes…*). Sur
le Steam Deck (NieRDeck), il n'y a pas de question : la plus récente gagne.

Côté serveur : [sauvegardes](https://github.com/Farfaboulette7/FarfaRomManager/blob/master/docs/sauvegardes.md).

### Un fichier `installed.json.illisible-…` ou `settings.json.illisible-…` est apparu

Dans le dossier des réglages (`%APPDATA%\fr.farfaboulette.farfalauncher\` sous Windows,
`~/.config/fr.farfaboulette.farfalauncher/` sous Linux). Le fichier était abîmé (arrêt brutal,
disque plein, coupure de courant). Le launcher ne l'a **pas** écrasé : il l'a renommé avec la
date, et il est reparti d'un fichier vide. Le journal le dit
(`illisible (…) : mis de côté sous …`).

- **`settings.json.illisible-…`** : les réglages sont revenus aux valeurs par défaut. Le
  joueur ressaisit l'adresse du serveur (et se reconnecte si besoin), puis refait ses
  réglages (dossier des jeux, sons…). Rien n'est perdu côté jeux.
- **`installed.json.illisible-…`** : le launcher ne sait plus quels jeux sont installés.
  Les fichiers des jeux et leurs sauvegardes sont **toujours sur le disque**.
  1. Fermer le launcher.
  2. Ouvrir le fichier `.illisible-…` dans un éditeur de texte. S'il est seulement coupé à la
     fin (JSON incomplet) et qu'on sait le réparer, le réparer, le renommer en
     `installed.json` (en remplaçant le nouveau), et rouvrir le launcher.
  3. Sinon : réinstaller les jeux. L'installation ne supprime rien dans le dossier du jeu et
     ne touche pas aux sauvegardes ; elles sont même copiées dans
     `.farfalauncher/backups/` avant.
- Une fois tout revenu, le fichier `.illisible-…` peut être supprimé.

Les `*.tmp` dans ce dossier sont des écritures interrompues : le launcher efface seul ceux de
plus d'une heure.

### « Le dossier … n'a pas pu être remis à sa place »

À la désinstallation. Certains jeux et émulateurs ont un dossier hors du jeu remplacé par un
lien (clé `lien:` de leur fiche, par exemple `%LOCALAPPDATA%\boiii`). Si ce dossier existait
avant le launcher, son contenu a été déplacé dans le jeu (repère `.farfa-migre`) et doit
retourner à sa place à la désinstallation. Ici, ça a échoué : **rien n'a été effacé** et la
désinstallation s'est arrêtée.

1. Fermer le programme qui utilise le dossier (le jeu, l'émulateur, une fenêtre de
   l'Explorateur ouverte dedans), ou libérer de la place si le disque est plein.
2. Relancer la désinstallation.
3. Si ça échoue encore : le journal donne le chemin (`non remis à son chemin d'origine`).
   Copier à la main le contenu du dossier du jeu indiqué vers ce chemin, puis désinstaller.

### La mise à jour du launcher ne s'installe pas

- **Elle n'est pas proposée** : *Paramètres → À propos → Vérifier les mises à jour*. Une bêta
  n'est proposée qu'avec « Recevoir les versions de test » coché. Une release qui compile
  encore sur GitHub n'est pas encore visible (attendre 30 min).
- **Windows** : la mise à jour lance l'installateur ; un antivirus peut le bloquer. SmartScreen
  peut demander « Exécuter quand même ».
- **Linux / Deck** : l'AppImage est remplacée à sa place : son dossier doit être modifiable
  par l'utilisateur.
- **Erreur de signature** : le paquet n'est pas signé par la bonne clé (voir
  [release.md §10](release.md#10-la-clé-de-signature--ne-jamais-la-perdre)).
- **Dans tous les cas** : télécharger la dernière version à la main sur
  https://github.com/Farfaboulette7/FarfaLauncher/releases/latest et l'installer par-dessus.
  Jeux, réglages et connexion sont gardés.
- **Version de test d'un rapport** : si « Installer la version de test » n'apparaît pas
  (launcher trop ancien), le joueur la télécharge depuis le lien donné dans son signalement.

### Plus de musique ni de bruitages

Le serveur des assets (`assets.<domaine>`, servi par le gestionnaire) ne répond pas, ou
l'adresse est fausse (*Paramètres → Sons*). Le launcher reste simplement muet. *Vider le
cache* reprend les fichiers à neuf.

## 3. Lancer le smoke test

Il vérifie toute la chaîne (RomM, gestionnaire, puis le vrai code du launcher qui installe et
lance un petit jeu), sans rien écrire sur les serveurs. Détails :
[smoke-test.md](smoke-test.md).

Sur ce PC (le launcher doit y être connecté au moins une fois) : double-cliquer sur
`scripts\smoke-test.bat` dans le dossier du dépôt, ou dans PowerShell :

```powershell
C:\Users\Farfa\Documents\CLAUDE\FarfaLauncher\scripts\smoke-test.bat
C:\Users\Farfa\Documents\CLAUDE\FarfaLauncher\scripts\smoke-test.bat -ApiSeulement
```

La première vérifie tout (1 à 3 minutes), la seconde seulement la partie réseau. Lire le
**premier** échec : le tableau de `smoke-test.md` dit quoi faire pour chaque étape.

Sur GitHub, il tourne chaque matin (*Actions → Smoke test RomM*). Un échec envoie un courriel.
