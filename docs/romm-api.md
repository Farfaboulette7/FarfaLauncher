# API RomM 5.4.0 — ce dont FarfaLauncher a besoin

Source de vérité : le code de [rommapp/romm au tag `5.4.0`](https://github.com/rommapp/romm/tree/5.4.0)
(commit `68f71259`). Chaque endpoint ci-dessous a été lu dans le code, avec le fichier
de référence entre parenthèses. Le schéma OpenAPI complet est servi par chaque serveur
RomM sur `GET /openapi.json` (et la doc interactive sur `/api/docs`).

Toutes les routes sont préfixées par `/api` (`backend/main.py`).

---

## 1. Authentification

### 1.1 Client API Token (`rmm_…`)

- En-tête : `Authorization: Bearer rmm_xxxxxxxx`
  (`backend/handler/auth/hybrid_auth.py` : un bearer qui commence par `rmm_` est résolu
  comme Client Token ; sinon il est traité comme un JWT OAuth).
- Les scopes effectifs = **scopes du token ∩ scopes actuels de l'utilisateur**. Si tu
  retires un droit à un compte, ses tokens le perdent aussi.
- Token expiré, révoqué, ou compte désactivé/supprimé → la requête n'est pas
  authentifiée → **401** sur toute route protégée ; scope manquant → **403**.
  Le launcher traite les deux comme « session invalide » → retour à l'écran de connexion.
- Pas de CSRF pour les requêtes Bearer (`csrf_middleware.py` : bypass si
  `Authorization: Bearer` et pas de cookie de session) → le launcher n'envoie **aucun
  cookie**.
- Vérifier un token : `GET /api/users/me` (scope `me.read`).
- Vérifier qu'une URL est bien un serveur RomM (sans auth) : `GET /api/heartbeat`
  → `SYSTEM.VERSION` (ex. `"5.4.0"`).

### 1.2 Flux A — Appairage par code (`/api/client-tokens/…`)

(`backend/endpoints/client_tokens.py`, `backend/utils/client_tokens.py`)

1. L'ami se connecte à l'interface web RomM → *Paramètres → Client API Tokens* →
   crée un token (nom, **scopes cochés à la main**, expiration `30d|90d|1y|never`).
2. Il clique « Pair device » → `POST /api/client-tokens/{id}/pair` → un code est affiché.
3. Dans le launcher, il tape le code → `POST /api/client-tokens/exchange`
   `{"code": "ABCD-EFGH"}` (route ouverte, sans auth) → réponse `ClientTokenCreateSchema`
   avec `raw_token`.

Détails importants trouvés dans le code :
- ⚠️ Le code n'est **pas à 8 chiffres** : c'est **8 caractères alphanumériques**
  (alphabet `ABCDEFGHJKMNPQRSTUVWXYZ23456789`, sans 0/O/1/I/L), tirets et casse ignorés.
- ⚠️ Il **expire en 60 secondes** (`PAIR_CODE_TTL_SECONDS = 60`) et est à usage unique.
- L'échange **régénère** le secret du token : l'ancien `raw_token` (s'il avait été copié)
  devient invalide.
- Limite : 5 tentatives d'échange / minute / IP.
- Variante « deep link » : la page web `/pair?code=…&callback=farfalauncher://…`
  échange le code dans le navigateur et redirige vers le schéma d'URL custom avec
  `?token=…` (`frontend/src/views/Pair.vue`). Utile plus tard, pas nécessaire au MVP.

### 1.3 Flux B — Autorisation d'appareil (`/api/auth/device/…`)

(`backend/endpoints/device/auth.py`, `backend/utils/device_auth.py`,
`backend/endpoints/responses/device/auth.py`) — type RFC 8628.

1. Le launcher appelle (sans auth) `POST /api/auth/device/init` :
   ```json
   {
     "client_device_identifier": "<UUID stable généré au 1er lancement>",
     "name": "PC de Léo",
     "client": "FarfaLauncher",
     "platform": "windows",
     "client_version": "0.1.0",
     "requested_scopes": ["me.read", "platforms.read", "roms.read", "..."]
   }
   ```
   → `201` `{device_code, user_code, verification_path: "/pair/device",
   verification_path_complete: "/pair/device?user_code=XXXXXXXX", expires_in: 600, interval: 5}`
2. Le launcher affiche le `user_code` et un bouton **« Ouvrir dans le navigateur »**
   vers `<serveur>/pair/device?user_code=…` (+ QR code éventuel).
   Si l'ami n'est pas connecté, RomM l'envoie à la page de login puis le ramène
   sur la page d'approbation (`frontend/src/plugins/router.ts`, paramètre `next`).
   La page montre le nom de l'appareil et les scopes demandés ; il clique **Approuver**.
3. Pendant ce temps le launcher interroge `POST /api/auth/device/token`
   `{"device_code": "…"}` toutes les `interval` secondes :
   - `400 {"detail":"authorization_pending"}` → continuer ;
   - `400 slow_down` → augmenter l'intervalle ;
   - `400 access_denied` → refusé ; `400 expired_token` → recommencer ;
   - `200 {access_token: "rmm_…", device_id, scopes, expires_at}` → terminé.

Détails : code valable **10 minutes** ; 10 `init`/min/IP, 60 polls/min/IP. L'approbation
crée un **Device** RomM lié 1-pour-1 au token (visible et révocable par l'ami ou par toi
dans RomM). Les scopes accordés = demandés ∩ droits du compte (l'ami peut en décocher).

### 1.4 Recommandation

**Flux B (device) en principal**, flux A (code) + collage manuel du token en secours.

| | Flux A — code | Flux B — device |
|---|---|---|
| Étapes pour l'ami | aller dans les paramètres, créer un token, cocher les bons scopes, cliquer Pair, retaper le code en < 60 s | cliquer « Ouvrir dans le navigateur », se connecter, cliquer Approuver |
| Risque d'erreur | scopes oubliés, code expiré | quasi nul : c'est le launcher qui demande les scopes |
| Temps disponible | 60 s | 10 min |
| Traçabilité | token « nu » | appareil nommé, révocable |

### 1.5 Scopes minimaux

| Scope | Pourquoi | Obligatoire |
|---|---|---|
| `me.read` | `GET /api/users/me` : valider le token, afficher le nom du compte | oui |
| `platforms.read` | `GET /api/platforms` : trouver `farfa-portage` | oui |
| `roms.read` | lister les jeux, détails, liste des fichiers, **téléchargement** | oui |
| `roms.user.write` | `POST /api/play-sessions` : enregistrer le temps de jeu | oui (temps de jeu) |
| `roms.user.read` | `GET /api/play-sessions` : relire le temps de jeu | oui (temps de jeu) |
| `devices.read` | lire les sessions de **tous** les appareils de l'ami (PC + Steam Deck…). Sans lui, le launcher ne voit que les sessions de son propre appareil | recommandé |
| `collections.read` / `collections.write` | favoris = collection « favoris » de l'ami (§5). Un token créé avant les favoris reçoit 403 : le launcher propose de se reconnecter | oui (favoris) |

Pas besoin de `assets.read` : les jaquettes, captures et livrets scrapés sont des fichiers
statiques servis sans authentification (voir §4).

Lecture seule stricte impossible si on veut le temps de jeu : `roms.user.write` est le
seul scope d'écriture, et il ne permet de modifier que les propriétés *personnelles*
de l'ami sur un jeu (statut, note, temps de jeu), pas la bibliothèque. Ces 6 scopes font
tous partie du rôle « Viewer » par défaut de RomM
(`backend/handler/auth/permissions_map.py`).

---

## 2. Trouver la plateforme et lister ses jeux

1. `GET /api/platforms` (`platforms.read`) → `list[PlatformSchema]`
   (`backend/endpoints/platform.py`). On garde toutes les plateformes dont le `fs_slug`
   (nom du dossier sur disque) commence par `portage` : `portage-n64`, `portage-ngc`…
   Nom affiché : `custom_name` sinon `name` ; nombre de jeux : `rom_count`. Icône : les
   fichiers statiques `/assets/platforms/<fs_slug|slug>.svg|.ico`, essayés dans cet ordre
   comme le fait l'interface web (`PlatformIcon.vue`).
2. Pour chaque plateforme, `GET /api/roms?platform_ids=<id>&with_files=true&limit=500`
   `&with_char_index=false&with_filter_values=false&with_rom_id_index=false&with_total=false`
   (`roms.read`, `backend/endpoints/roms/__init__.py` `get_roms`) →
   `{items: list[SimpleRomSchema], total, limit, offset}`.
   - `with_files=true` inclut la liste des fichiers de chaque jeu dans la même requête.
   - `limit` va jusqu'à 10 000 ; on pagine avec `offset` par sécurité.
   - `updated_after=<ISO8601>` permet un rafraîchissement incrémental.
3. Détail d'un jeu : `GET /api/roms/{rom_id}` → `DetailedRomSchema` (même champs + données
   utilisateur).

---

## 3. Fichiers d'un jeu multi-fichiers

Chaque jeu a `files: list[RomFileSchema]` (`backend/endpoints/responses/rom.py`) :

| Champ | Contenu |
|---|---|
| `id` | identifiant du fichier (pour le téléchargement) |
| `file_name` | ex. `sm64coopdx_Windows.zip` |
| `file_path` | dossier relatif à la racine de la bibliothèque, ex. `roms/farfa-portage/Super Mario 64/RECOMP` |
| `full_path` | `file_path/file_name` |
| `file_size_bytes` | taille (vérification d'espace disque, barre de progression) |
| `is_top_level` | `true` pour `VERSION.txt` |
| `crc_hash`, `md5_hash`, `sha1_hash` | **peuvent être `null`** (selon l'option de hash du scan) |
| `category` | déduite du **premier dossier** sous le jeu : `MODS/` → `"mod"` ; `RECOMP/` et `ROM/` → `null` ; fichier racine → `"game"` |

**Chemin relatif dans le jeu** (`RECOMP/…`, `ROM/…`, `MODS/…`, `VERSION.txt`) :
`file.full_path` moins le préfixe `rom.full_path + "/"` (`rom.full_path` = dossier du jeu).
Le launcher se base sur ce chemin relatif, pas sur `category`.

**Télécharger un fichier précis** :
`GET /api/roms/{file_id}/files/content/{file_name}` (`roms.read`,
`backend/endpoints/roms/files.py`). ⚠️ Le `{id}` est celui du **fichier**, pas du jeu
(le nom de la route est trompeur). Le contenu est servi par nginx (`X-Accel-Redirect`) →
`Content-Length` présent et **requêtes `Range` supportées** → reprise de téléchargement
possible.

**Intégrité** : si `sha1_hash` (ou `md5_hash`) est non nul, on le vérifie après
téléchargement ; sinon on vérifie au moins la taille.

**VERSION.txt** : téléchargé par le même endpoint, contenu = version serveur.

Ne pas utiliser `GET /api/roms/{id}/content/{file_name}` (route du jeu entier, renvoie un
zip de tout le dossier).

---

## 4. Médias : jaquette, captures, résumé, livret

Tous ces chemins sont relatifs à l'origine du serveur et pointent vers
`/assets/romm/resources/…`, servi **en statique par nginx sans authentification**
(`docker/nginx/templates/default.conf.template`).

| Info | Champ (`SimpleRomSchema` / `DetailedRomSchema`) |
|---|---|
| Jaquette | `path_cover_large` (et `path_cover_small`) — déjà préfixés, avec `?ts=` (cache-busting) ; chaîne vide si aucune |
| Captures | `merged_screenshots: list[str]` — chemins déjà préfixés |
| Vidéo | `path_video` (optionnel) |
| Résumé | `summary` ; autres infos : `name`, `metadatum.genres`, `metadatum.developers`, `metadatum.first_release_date`… |
| Livret (scrapé/uploadé) | si `has_manual` : `/assets/romm/resources/` + `path_manual` (non préfixé ; ajouter `?v=<updated_at>`) — PDF |
| Livret (fichier dans le dossier du jeu) | fichiers dont `category == "manual"` (dossier `manual/` ou `manuals/`) → endpoint de téléchargement du §3 (servi `inline`) |
| Faces de la boîte | `ss_metadata` (ou `gamelist_metadata`) : `box2d_path` (avant), `box2d_side_path` (tranche), `box2d_back_path` (dos) — relatifs à `/assets/romm/resources/` |
| Boîte 3D détourée | `ss_metadata.box3d_path`, seulement si `box3d` est dans `scan.media` du `config.yml` |
| Date d'ajout (onglet Récent) | `created_at` |

**Boîte 3D** : RomM ne télécharge que les médias listés dans `scan.media` de son
`config.yml` ; pour les autres, seul le lien ScreenScraper (`box3d_url`…) est stocké, et
il contient des identifiants : le launcher ne l'utilise jamais. Comme l'interface web
(« `box2d_side` enables the interactive 3D box »), le launcher construit la boîte en CSS 3D
à partir de l'avant, de la tranche et du dos. Certaines tranches (N64) sont stockées
couchées (plus larges que hautes) : elles sont redressées d'un quart de tour.

---

## 5. Favoris

Les favoris sont la collection de l'utilisateur marquée `is_favorite` — le cœur de
l'interface web, donc partagés entre tous ses appareils
(`backend/endpoints/collections.py`, `frontend/src/composables/useFavoriteToggle.ts`).

- Lire : `GET /api/collections` (`collections.read`) → garder celle où
  `is_favorite == true` **et** `user_id ==` l'id de `GET /api/users/me` (une collection
  favoris publique d'un autre utilisateur apparaît aussi dans la liste). Jeux : `rom_ids`.
- Créer si absente : `POST /api/collections?is_favorite=true&is_public=false`, formulaire
  `name=Favorites` (`collections.write`).
- Ajouter / retirer : `POST` / `DELETE /api/collections/{id}/roms` avec
  `{"rom_ids": [42]}` → la collection mise à jour.

---

## 6. Temps de jeu — ✅ disponible, on l'implémente

RomM 5.4.0 a des **sessions de jeu par utilisateur**
(`backend/endpoints/play_sessions.py`, `backend/models/play_session.py`).

**Enregistrer** : `POST /api/play-sessions` (`roms.user.write`)
```json
{
  "device_id": null,
  "sessions": [{
    "rom_id": 42,
    "start_time": "2026-10-24T20:00:00Z",
    "end_time":   "2026-10-24T21:30:00Z",
    "duration_ms": 5400000
  }]
}
```
- `device_id` peut être omis : avec un token du flux device, RomM prend l'appareil lié.
- `end_time > start_time` obligatoire ; max 100 sessions par requête ; dates dans le futur
  de plus de 5 min refusées.
- Réponse : `{results: [{index, status: "created"|"duplicate"|"error", id, detail}],
  created_count, skipped_count}`. Les doublons sont détectés → on peut renvoyer sans
  risque une session restée en file d'attente (hors-ligne).
- Effet de bord côté RomM : met à jour `rom_user.last_played` et passe le jeu en
  « en cours ».

**Relire** : `GET /api/play-sessions?rom_id=42&limit=…&offset=…` (`roms.user.read`) →
`list[PlaySessionSchema]` (dont `duration_ms`). Temps total = somme des `duration_ms`.
- Sans `device_id` (toutes les sessions de l'ami, tous appareils) → nécessite
  `devices.read`. Avec `device_id=<son appareil>` → `roms.user.read` suffit.
- Il n'existe pas d'endpoint « temps total » : le launcher fait la somme.

---

## Points à vérifier sur ton serveur (pendant les tests)

1. `fs_slug` réel de la plateforme (devrait être `farfa-portage`).
2. Forme exacte de `rom.full_path` / `file.full_path` pour un jeu (préfixe `roms/…`).
3. Présence ou non des hashes sur les fichiers (option de hash au scan).
4. Que le reverse proxy du NAS laisse bien passer l'en-tête `Range` et les gros fichiers.

---

## Constats sur un serveur réel (tests d'octobre 2026)

- **Hash des archives** : pour un fichier compressé (`.zip`, `.tar`, `.gz`, `.7z`, `.bz2`),
  RomM stocke un hash **du contenu décompressé**, pas du fichier
  (`_calculate_rom_hashes`). FarfaLauncher ne compare donc le SHA-1/MD5 que pour les
  fichiers non compressés (ROM, AppImage) ; pour une archive, il vérifie la taille puis
  les CRC à l'extraction.
- **Jeux à très nombreux fichiers** : un dossier `RECOMP/` contenant des archives déjà
  extraites (135 000 fichiers) fait mettre ~40 s à RomM pour décrire le jeu, et bloque
  `GET /api/roms?with_files=true` sur une page entière. Le launcher charge donc la liste
  sans fichiers, puis le détail (`GET /api/roms/{id}`) jeu par jeu, deux à la fois.
- **VERSION.txt** : rapport multi-lignes écrit par le script de mise à jour ; la version
  affichée est la ligne `Version :` de la section `RECOMP`, et la détection de mise à jour
  compare le fichier entier (normalisé).
- **Erreurs 500 intermittentes** sur `GET /api/play-sessions` et `GET /api/users/me`,
  surtout quand deux requêtes identiques partent en même temps : les lectures sont
  retentées une fois.
- **Sessions de jeu** : `POST /api/play-sessions` sans `device_id` fonctionne avec un
  token issu du flux device (RomM prend l'appareil lié) ; réponse `created` puis
  `duplicate` en cas de renvoi.
