<#
  Smoke test du Farfa project : vérifie, contre les VRAIS serveurs, que la chaîne marche.

    1. Contrôle de l'API RomM : les appels du launcher et les champs qu'il lit.
    2. Contrôle du gestionnaire (FarfaRomManager) : /health, catalogue, téléchargement.
    3. Le vrai code du launcher (moteur `farfalauncherdeck`, sans interface) :
       connexion, liste des jeux, installation d'un petit jeu, lancement de quelques secondes.

  RIEN n'est écrit sur RomM ni sur le gestionnaire : seulement des lectures (GET).
  Le lancement passe par la commande `essai` du moteur, qui n'envoie ni sauvegarde,
  ni temps de jeu, ni présence, ni statistiques.

  Le jeton RomM vient de la variable FARFA_SMOKE_TOKEN, sinon du trousseau Windows
  (là où le launcher le range). Il n'est jamais affiché ni écrit dans un fichier.

  Utilisation : double-cliquer sur smoke-test.bat. Voir docs/smoke-test.md.

  Options :
    -ApiSeulement      seulement les contrôles HTTP (1 et 2), sans compiler ni lancer (GitHub)
    -Discret           n'affiche ni noms de jeux ni nom de compte (journaux publics de GitHub)
    -Console snes      console du jeu installé et lancé (slug RomM ; il faut un émulateur)
    -RomId 1234        jeu précis à installer et lancer (sinon : le premier de moins de 5 Mo)
    -Secondes 10       durée pendant laquelle le jeu doit tenir
    -Dossier <chemin>  dossier où créer le dossier de travail `farfa-smoke` (supprimé à la fin)
                       et, en cas d'échec, `farfa-smoke-journal` ; par défaut %TEMP%
    -SansCompilation   utilise le moteur déjà compilé, sans lancer cargo
    -Serveur <url>     serveur RomM (sinon FARFA_SMOKE_URL, sinon celui du launcher)
    -Gestionnaire <url> gestionnaire (sinon FARFA_SMOKE_MANAGER, sinon manager.<domaine>)
#>
param(
    [switch]$ApiSeulement,
    [switch]$Discret,
    [string]$Console = 'snes',
    [long]$RomId = 0,
    [int]$Secondes = 10,
    [string]$Dossier = '',
    [switch]$SansCompilation,
    [string]$Serveur = '',
    [string]$Gestionnaire = ''
)

$ErrorActionPreference = 'Stop'
[Console]::OutputEncoding = [Text.Encoding]::UTF8
$SurWindows = [Environment]::OSVersion.Platform -eq 'Win32NT'
$TailleMax = 5MB   # taille maximale du jeu choisi automatiquement

# ---------------------------------------------------------------------------
# Affichage : une ligne par étape, puis un résumé
# ---------------------------------------------------------------------------

$script:Resultats = New-Object System.Collections.ArrayList
$script:Numero = 0

# Lance une étape. Le bloc renvoie un texte (le détail affiché) ; s'il commence par
# « ATTENTION », l'étape passe mais signale un risque. Une exception = ÉCHEC.
# -Si $false : l'étape est sautée (une étape dont elle dépend a échoué).
function Etape([string]$Nom, [scriptblock]$Action, [bool]$Si = $true) {
    $script:Numero++
    $debut = Get-Date
    if (-not $Si) {
        $etat = 'SAUTÉ'; $detail = 'une étape précédente a échoué'
    } else {
        try {
            $detail = (& $Action) -join ' '
            $etat = if ($detail -like 'ATTENTION*') { 'ATTENTION' } else { 'OK' }
        } catch {
            $etat = 'ÉCHEC'; $detail = $_.Exception.Message
        }
    }
    $duree = '{0:N1} s' -f ((Get-Date) - $debut).TotalSeconds
    $couleur = @{ 'OK' = 'Green'; 'ATTENTION' = 'Yellow'; 'ÉCHEC' = 'Red'; 'SAUTÉ' = 'DarkGray' }[$etat]
    Write-Host ('[{0,-9}] {1,2}. {2}' -f $etat, $script:Numero, $Nom) -ForegroundColor $couleur -NoNewline
    Write-Host "  $detail ($duree)"
    # Annotations GitHub Actions : visibles dans le résumé du workflow.
    if ($env:GITHUB_ACTIONS -eq 'true') {
        if ($etat -eq 'ÉCHEC') { Write-Host "::error title=Smoke test::$Nom : $detail" }
        if ($etat -eq 'ATTENTION') { Write-Host "::warning title=Smoke test::$Nom : $detail" }
    }
    [void]$script:Resultats.Add([pscustomobject]@{ Nom = $Nom; Etat = $etat; Detail = $detail })
    return ($etat -eq 'OK' -or $etat -eq 'ATTENTION')
}

function Resume {
    $echecs = @($script:Resultats | Where-Object Etat -eq 'ÉCHEC').Count
    $attentions = @($script:Resultats | Where-Object Etat -eq 'ATTENTION').Count
    $sautes = @($script:Resultats | Where-Object Etat -eq 'SAUTÉ').Count
    $ok = @($script:Resultats | Where-Object Etat -eq 'OK').Count
    Write-Host ''
    Write-Host '================ Résumé ================'
    Write-Host "OK : $ok   Attention : $attentions   Échecs : $echecs   Sautées : $sautes"
    if ($echecs -eq 0 -and $sautes -eq 0) {
        Write-Host 'Tout marche.' -ForegroundColor Green
    } elseif ($echecs -eq 0) {
        Write-Host 'Pas d''échec, mais des étapes ont été sautées.' -ForegroundColor Yellow
    } else {
        Write-Host 'Étapes en échec :' -ForegroundColor Red
        $script:Resultats | Where-Object Etat -eq 'ÉCHEC' | ForEach-Object { Write-Host "  - $($_.Nom) : $($_.Detail)" -ForegroundColor Red }
        Write-Host 'Que faire : voir docs/smoke-test.md, section « Les étapes, et que faire en cas d''échec ».'
    }
    return $echecs
}

# ---------------------------------------------------------------------------
# HTTP : HttpClient marche pareil sous Windows PowerShell 5.1 et PowerShell 7 (GitHub)
# ---------------------------------------------------------------------------

Add-Type -AssemblyName System.Net.Http
[Net.ServicePointManager]::SecurityProtocol = [Net.ServicePointManager]::SecurityProtocol -bor [Net.SecurityProtocolType]::Tls12
$script:Http = New-Object System.Net.Http.HttpClient
$script:Http.Timeout = [TimeSpan]::FromSeconds(120)
$script:Http.DefaultRequestHeaders.UserAgent.ParseAdd('FarfaLauncher-smoke-test')

# GET d'une URL. -Jeton : en-tête Authorization (jamais affiché).
# -Octets N : demande seulement les N premiers octets (Range) et n'en lit pas plus.
function Get-Http([string]$Url, [string]$Jeton = '', [int]$Octets = 0) {
    $req = New-Object System.Net.Http.HttpRequestMessage ([System.Net.Http.HttpMethod]::Get), $Url
    if ($Jeton) { $req.Headers.Authorization = New-Object System.Net.Http.Headers.AuthenticationHeaderValue 'Bearer', $Jeton }
    if ($Octets -gt 0) { $req.Headers.Range = New-Object System.Net.Http.Headers.RangeHeaderValue ([long]0), ([long]($Octets - 1)) }
    try {
        $resp = $script:Http.SendAsync($req, [System.Net.Http.HttpCompletionOption]::ResponseHeadersRead).GetAwaiter().GetResult()
    } catch {
        $e = $_.Exception; while ($e.InnerException) { $e = $e.InnerException }
        throw "serveur injoignable ($($e.Message))"
    }
    try {
        $r = [pscustomobject]@{ Code = [int]$resp.StatusCode; Texte = ''; Lus = 0; Plage = $null; Total = $null }
        if ($Octets -gt 0) {
            # Lit au plus $Octets octets, puis coupe : pas de téléchargement complet par erreur.
            $flux = $resp.Content.ReadAsStreamAsync().GetAwaiter().GetResult()
            $tampon = New-Object byte[] 65536
            while ($r.Lus -lt $Octets) {
                $n = $flux.Read($tampon, 0, [Math]::Min($tampon.Length, $Octets - $r.Lus))
                if ($n -le 0) { break }
                $r.Lus += $n
            }
            $r.Plage = $resp.Content.Headers.ContentRange
            $r.Total = $resp.Content.Headers.ContentLength
        } else {
            $r.Texte = $resp.Content.ReadAsStringAsync().GetAwaiter().GetResult()
        }
        return $r
    } finally { $resp.Dispose() }
}

# GET JSON : échoue avec un message clair si le code n'est pas 200.
function Get-Json([string]$Url, [string]$Jeton = '') {
    $r = Get-Http $Url $Jeton
    if ($r.Code -eq 401 -or $r.Code -eq 403) { throw "HTTP $($r.Code) : jeton refusé (expiré, révoqué ou droits manquants)" }
    if ($r.Code -ne 200) { throw "HTTP $($r.Code)" }
    try { return ($r.Texte | ConvertFrom-Json) } catch { throw 'réponse qui n''est pas du JSON' }
}

# Vérifie qu'un objet JSON a les champs demandés. Renvoie la liste de ceux qui manquent.
function Champs-Manquants($Objet, [string[]]$Champs) {
    $presents = @($Objet.PSObject.Properties.Name)
    return @($Champs | Where-Object { $presents -notcontains $_ })
}

# ---------------------------------------------------------------------------
# Paramètres : serveur, gestionnaire, jeton
# ---------------------------------------------------------------------------

# Réglages du launcher sur ce PC (adresse du serveur), s'ils existent.
$reglages = $null
if ($SurWindows -and $env:APPDATA) {
    $fichier = Join-Path $env:APPDATA 'fr.farfaboulette.farfalauncher\settings.json'
    if (Test-Path $fichier) { try { $reglages = Get-Content $fichier -Raw -Encoding UTF8 | ConvertFrom-Json } catch {} }
}
if (-not $Serveur) { $Serveur = $env:FARFA_SMOKE_URL }
if (-not $Serveur -and $reglages) { $Serveur = $reglages.server_url }
if (-not $Gestionnaire) { $Gestionnaire = $env:FARFA_SMOKE_MANAGER }
if (-not $Gestionnaire -and $reglages) { $Gestionnaire = $reglages.manager_url }
if ($Serveur) { $Serveur = $Serveur.Trim().TrimEnd('/') }
if (-not $Gestionnaire -and $Serveur) {
    # Comme le launcher : manager.<domaine> (premier niveau du nom remplacé).
    $u = [Uri]$Serveur
    $parties = $u.Host.Split('.')
    $hote = if ($parties.Count -gt 2) { 'manager.' + (($parties | Select-Object -Skip 1) -join '.') } else { 'manager.' + $u.Host }
    $Gestionnaire = "$($u.Scheme)://$hote"
}
if ($Gestionnaire) { $Gestionnaire = $Gestionnaire.Trim().TrimEnd('/') }

# Jeton : variable FARFA_SMOKE_TOKEN, sinon trousseau Windows (cible « <serveur>.FarfaLauncher »,
# rangée par la bibliothèque keyring du launcher, mot de passe en UTF-16).
function Lire-JetonTrousseau([string]$Cible) {
    if (-not ('FarfaSmoke.Trousseau' -as [type])) {
        Add-Type -TypeDefinition @'
using System;
using System.Runtime.InteropServices;
namespace FarfaSmoke {
    public static class Trousseau {
        [StructLayout(LayoutKind.Sequential, CharSet = CharSet.Unicode)]
        struct CREDENTIAL {
            public int Flags; public int Type; public string TargetName; public string Comment;
            public long LastWritten; public int CredentialBlobSize; public IntPtr CredentialBlob;
            public int Persist; public int AttributeCount; public IntPtr Attributes;
            public string TargetAlias; public string UserName;
        }
        [DllImport("advapi32.dll", CharSet = CharSet.Unicode, SetLastError = true)]
        static extern bool CredReadW(string target, int type, int flags, out IntPtr cred);
        [DllImport("advapi32.dll")]
        static extern void CredFree(IntPtr cred);
        public static string Lire(string cible) {
            IntPtr p;
            if (!CredReadW(cible, 1, 0, out p)) return null;   // 1 = CRED_TYPE_GENERIC
            try {
                CREDENTIAL c = (CREDENTIAL)Marshal.PtrToStructure(p, typeof(CREDENTIAL));
                if (c.CredentialBlobSize == 0) return null;
                byte[] octets = new byte[c.CredentialBlobSize];
                Marshal.Copy(c.CredentialBlob, octets, 0, octets.Length);
                return System.Text.Encoding.Unicode.GetString(octets);
            } finally { CredFree(p); }
        }
    }
}
'@
    }
    return [FarfaSmoke.Trousseau]::Lire($Cible)
}

$Jeton = $env:FARFA_SMOKE_TOKEN
$origineJeton = 'variable FARFA_SMOKE_TOKEN'
if (-not $Jeton -and $SurWindows -and $Serveur) {
    try { $Jeton = Lire-JetonTrousseau "$Serveur.FarfaLauncher"; $origineJeton = 'trousseau Windows (launcher)' } catch { $Jeton = $null }
}
if ($Jeton) { $Jeton = $Jeton.Trim() }

Write-Host '=== Smoke test du Farfa project ==='
Write-Host ("Date : {0:yyyy-MM-dd HH:mm}" -f (Get-Date))
if (-not $Discret) {
    Write-Host "Serveur RomM : $Serveur"
    Write-Host "Gestionnaire : $Gestionnaire"
}
Write-Host ''

# ---------------------------------------------------------------------------
# 1. API RomM : les appels du launcher (docs/romm-api.md) et les champs qu'il lit
# ---------------------------------------------------------------------------

$ok = Etape 'Paramètres (serveur, jeton)' {
    if (-not $Serveur) { throw 'serveur inconnu : lancer le launcher une fois, ou définir FARFA_SMOKE_URL' }
    if (-not $Jeton) { throw 'jeton introuvable : se connecter dans le launcher, ou définir FARFA_SMOKE_TOKEN' }
    "jeton lu dans : $origineJeton"
}
if (-not $ok) { [void](Resume); exit 1 }

$version = $null
$okHeartbeat = Etape 'RomM répond (GET /api/heartbeat)' {
    $hb = Get-Json "$Serveur/api/heartbeat"
    $script:version = $hb.SYSTEM.VERSION
    if (-not $script:version) { throw 'champ SYSTEM.VERSION absent : le launcher ne reconnaîtra plus le serveur' }
    if ($script:version -notlike '5.4*') { "ATTENTION : RomM $($script:version), le launcher est écrit pour 5.4 (docs/romm-api.md)" }
    else { "RomM $($script:version)" }
}

$okMoi = Etape 'Connexion avec le jeton (GET /api/users/me)' {
    $moi = Get-Json "$Serveur/api/users/me" $Jeton
    $manque = Champs-Manquants $moi @('id', 'username')
    if ($manque) { throw "champs absents : $($manque -join ', ')" }
    if ($Discret) { 'compte reconnu' } else { "compte : $($moi.username) (rôle $($moi.role))" }
} -Si $okHeartbeat

$plateforme = $null
$okPlateformes = Etape 'Plateformes (GET /api/platforms)' {
    $liste = @(Get-Json "$Serveur/api/platforms" $Jeton)
    if ($liste.Count -eq 0) { throw 'aucune plateforme' }
    foreach ($p in $liste) {
        $manque = Champs-Manquants $p @('id', 'slug', 'fs_slug', 'name')
        if ($manque) { throw "plateforme sans les champs : $($manque -join ', ')" }
    }
    $script:plateforme = $liste | Where-Object { $_.slug -eq $Console -and $_.rom_count -gt 0 } | Sort-Object rom_count -Descending | Select-Object -First 1
    if (-not $script:plateforme) { throw "aucune plateforme « $Console » avec des jeux" }
    $sansNombre = @($liste | Where-Object { $_.PSObject.Properties.Name -notcontains 'rom_count' }).Count
    if ($sansNombre) { "ATTENTION : $($liste.Count) plateformes, $sansNombre sans rom_count" }
    else { "$($liste.Count) plateformes, « $Console » : $($script:plateforme.rom_count) jeux" }
} -Si $okMoi

$rom = $null
$okPage = Etape 'Une page de jeux (GET /api/roms, requête du launcher)' {
    $url = "$Serveur/api/roms?with_char_index=false&with_filter_values=false&with_rom_id_index=false&with_total=false&order_by=name&order_dir=asc&limit=100&offset=0&platform_ids=$($plateforme.id)"
    $page = Get-Json $url $Jeton
    if ($page.PSObject.Properties.Name -notcontains 'items') { throw 'champ items absent' }
    $jeux = @($page.items)
    if ($jeux.Count -eq 0) { throw 'page vide' }
    # Champs obligatoires pour le launcher (sans eux, toute la bibliothèque échoue).
    foreach ($j in $jeux) {
        $manque = Champs-Manquants $j @('id', 'platform_id', 'fs_name', 'full_path', 'updated_at')
        if ($manque) { throw "jeu $($j.id) sans les champs : $($manque -join ', ')" }
    }
    # Champs lus s'ils existent (jaquette, résumé, captures…) : leur absence appauvrit l'affichage.
    $optionnels = @('name', 'summary', 'path_cover_large', 'merged_screenshots', 'path_video', 'has_manual',
        'path_manual', 'metadatum', 'ss_metadata', 'created_at', 'platform_slug', 'platform_fs_slug', 'fs_size_bytes')
    $absents = Champs-Manquants $jeux[0] $optionnels
    # Jeu choisi : celui demandé, sinon le premier de moins de 5 Mo.
    if ($RomId -gt 0) { $script:rom = [pscustomobject]@{ id = $RomId; name = "jeu $RomId"; fs_size_bytes = 0 } }
    else {
        $petits = @($jeux | Where-Object { $_.fs_size_bytes -gt 0 -and $_.fs_size_bytes -lt $TailleMax })
        # De préférence un jeu fait d'un seul fichier (le cas le plus simple).
        $script:rom = @($petits | Where-Object { $_.has_simple_single_file -eq $true }) + $petits | Select-Object -First 1
    }
    if (-not $script:rom) { throw "aucun jeu de moins de 5 Mo sur la première page de « $Console »" }
    $nom = if ($Discret) { "jeu $($script:rom.id)" } else { "« $($script:rom.name) » (id $($script:rom.id))" }
    if ($absents) { "ATTENTION : champs disparus : $($absents -join ', ') ; $($jeux.Count) jeux, choisi : $nom" }
    else { "$($jeux.Count) jeux, champs attendus présents ; choisi : $nom" }
} -Si $okPlateformes

$fichier = $null
$okDetail = Etape 'Détail du jeu et de ses fichiers (GET /api/roms/{id})' {
    $d = Get-Json "$Serveur/api/roms/$($rom.id)" $Jeton
    $manque = Champs-Manquants $d @('id', 'fs_name', 'full_path', 'updated_at', 'files')
    if ($manque) { throw "champs absents : $($manque -join ', ')" }
    $fichiers = @($d.files)
    if ($fichiers.Count -eq 0) { throw 'aucun fichier' }
    foreach ($f in $fichiers) {
        $manque = Champs-Manquants $f @('id', 'file_name', 'full_path', 'file_size_bytes')
        if ($manque) { throw "fichier sans les champs : $($manque -join ', ')" }
    }
    $script:fichier = $fichiers | Sort-Object file_size_bytes | Select-Object -First 1
    $absents = Champs-Manquants $script:fichier @('sha1_hash', 'md5_hash', 'category')
    $taille = '{0:N0} Ko' -f ($script:fichier.file_size_bytes / 1KB)
    if ($absents) { "ATTENTION : champs disparus : $($absents -join ', ') ; $($fichiers.Count) fichier(s)" }
    else { "$($fichiers.Count) fichier(s), le plus petit : $taille" }
} -Si $okPage

$null = Etape 'Téléchargement partiel d''une ROM (Range, /api/roms/{fichier}/files/content/…)' {
    $nomUrl = [Uri]::EscapeDataString($fichier.file_name)
    $r = Get-Http "$Serveur/api/roms/$($fichier.id)/files/content/$nomUrl" $Jeton 1024
    $attendu = [Math]::Min(1024, [long]$fichier.file_size_bytes)
    if ($r.Code -eq 401 -or $r.Code -eq 403) { throw "HTTP $($r.Code) : jeton refusé" }
    if ($r.Code -eq 200) { "ATTENTION : Range ignoré (HTTP 200) : la reprise des téléchargements ne marchera plus ; $($r.Lus) octets lus" }
    elseif ($r.Code -ne 206) { throw "HTTP $($r.Code)" }
    elseif ($r.Lus -ne $attendu) { throw "$($r.Lus) octets reçus au lieu de $attendu" }
    else { "HTTP 206, $($r.Lus) octets, plage $($r.Plage)" }
} -Si $okDetail

# ---------------------------------------------------------------------------
# 2. Gestionnaire (FarfaRomManager)
# ---------------------------------------------------------------------------

$null = Etape 'Gestionnaire en service (GET /health)' {
    $h = Get-Json "$Gestionnaire/health"
    if ($h.status -ne 'ok') { throw "status = $($h.status)" }
    "démarré le $($h.started)"
}

$emulateur = $null
$okCatalogue = Etape 'Catalogue du gestionnaire (GET /api/catalog)' {
    $c = Get-Json "$Gestionnaire/api/catalog" $Jeton
    $manque = Champs-Manquants $c @('recomps', 'emulators')
    if ($manque) { throw "champs absents : $($manque -join ', ')" }
    foreach ($e in @($c.emulators)) {
        foreach ($f in @($e.files)) {
            $manque = Champs-Manquants $f @('sha1', 'size', 'system', 'path')
            if ($manque) { throw "émulateur $($e.id) : fichier sans les champs $($manque -join ', ')" }
        }
    }
    # Émulateur de la console testée, sinon le premier.
    $script:emulateur = @($c.emulators | Where-Object { @($_.consoles) -contains $Console }) + @($c.emulators) | Where-Object { @($_.files).Count -gt 0 } | Select-Object -First 1
    if (-not $script:emulateur) { throw 'aucun émulateur avec des fichiers' }
    "$(@($c.recomps).Count) recomps, $(@($c.emulators).Count) émulateurs"
}

$null = Etape 'Téléchargement partiel d''un fichier du gestionnaire (Range, /files/{sha1})' {
    $f = @($emulateur.files) | Sort-Object size | Select-Object -First 1
    $r = Get-Http "$Gestionnaire/files/$($f.sha1)" $Jeton 1024
    $attendu = [Math]::Min(1024, [long]$f.size)
    if ($r.Code -eq 401 -or $r.Code -eq 403) { throw "HTTP $($r.Code) : jeton refusé par le gestionnaire" }
    if ($r.Code -eq 200) { "ATTENTION : Range ignoré (HTTP 200), $($r.Lus) octets lus" }
    elseif ($r.Code -ne 206) { throw "HTTP $($r.Code)" }
    elseif ($r.Lus -ne $attendu) { throw "$($r.Lus) octets reçus au lieu de $attendu" }
    else { "HTTP 206, $($r.Lus) octets ($($emulateur.id))" }
} -Si $okCatalogue

if ($ApiSeulement) {
    $echecs = Resume
    exit ([int]($echecs -gt 0))
}

# ---------------------------------------------------------------------------
# 3. Le vrai code du launcher : moteur farfalauncherdeck (sans interface)
# ---------------------------------------------------------------------------

$depot = Split-Path -Parent $PSScriptRoot
$srcTauri = Join-Path $depot 'src-tauri'
# Dossier de build : CARGO_TARGET_DIR s'il est défini, sinon celui des versions de test.
$cible = if ($env:CARGO_TARGET_DIR) { $env:CARGO_TARGET_DIR } else { Join-Path $srcTauri 'target-test' }
$moteur = Join-Path $cible 'debug\farfalauncherdeck.exe'
# Le test travaille TOUJOURS dans un sous-dossier `farfa-smoke` du chemin donné, marqué par un
# fichier `.farfa-smoke` : `-Dossier C:\Users\moi\Documents` ne peut rien effacer d'autre.
if (-not $Dossier) { $Dossier = $env:TEMP }
$travail = Join-Path $Dossier 'farfa-smoke'
$garde = Join-Path $Dossier 'farfa-smoke-journal'
$config = Join-Path $travail 'config'
$jeux = Join-Path $travail 'Jeux'
$Marqueur = '.farfa-smoke'

# Crée un dossier du smoke test, avec son marqueur.
function Creer-DossierSmoke([string]$Chemin) {
    New-Item -ItemType Directory -Force $Chemin | Out-Null
    Set-Content -Path (Join-Path $Chemin $Marqueur) -Value 'Dossier du smoke test du Farfa project (effaçable).'
}

# Efface un dossier du smoke test, seulement s'il porte le marqueur (sinon : refus).
function Effacer-DossierSmoke([string]$Chemin) {
    if (-not (Test-Path $Chemin)) { return }
    if (-not (Test-Path (Join-Path $Chemin $Marqueur))) {
        throw "$Chemin existe mais n'a pas été créé par le smoke test (pas de fichier $Marqueur) : rien n'est effacé"
    }
    Remove-Item $Chemin -Recurse -Force
}

# Lance une commande du moteur et renvoie son objet `data` (ou lève l'erreur qu'il donne).
# Le moteur écrit une ligne JSON par message : progression, puis « result » ou « error ».
function Moteur([string[]]$Arguments) {
    $lignes = @(& $moteur @Arguments)
    $fin = $lignes | Where-Object { $_ -like '{*' } | ForEach-Object { try { $_ | ConvertFrom-Json } catch {} } |
        Where-Object { $_.type -eq 'result' -or $_.type -eq 'error' } | Select-Object -Last 1
    if (-not $fin) { throw "pas de réponse du moteur (code $LASTEXITCODE)" }
    if ($fin.type -eq 'error') { throw "erreur « $($fin.code) » : $($fin.message)" }
    return $fin.data
}

$env:FARFALAUNCHERDECK_CONFIG = $config
$env:FARFALAUNCHERDECK_JETON = $Jeton   # lu par le moteur, jamais écrit sur le disque
$okTravail = $false

$okMoteur = Etape 'Dossier de travail et compilation du moteur (cargo)' {
    # Dossier de travail neuf, préparé AVANT la première commande du moteur : ses réglages,
    # journaux et jeux vont là, nulle part ailleurs.
    Effacer-DossierSmoke $travail
    Creer-DossierSmoke $travail
    New-Item -ItemType Directory -Force $config, $jeux | Out-Null
    $script:okTravail = $true
    if (-not $SansCompilation) {
        if (-not (Get-Command cargo -ErrorAction SilentlyContinue)) { throw 'cargo introuvable (installer Rust), ou relancer avec -SansCompilation' }
        $env:CARGO_TARGET_DIR = $cible
        $env:CARGO_INCREMENTAL = '0'   # pas de cache incrémental : le disque ne se remplit pas
        Push-Location $srcTauri
        try {
            # La sortie de cargo va dans un fichier : l'écran reste lisible.
            $journal = Join-Path $env:TEMP 'farfa-smoke-cargo.log'
            $p = Start-Process cargo -ArgumentList 'build', '--no-default-features', '--bin', 'farfalauncherdeck' -NoNewWindow -Wait -PassThru -RedirectStandardError $journal
            if ($p.ExitCode -ne 0) { throw "la compilation a échoué (voir $journal)" }
            Remove-Item $journal -ErrorAction SilentlyContinue   # réussie : journal inutile
        } finally { Pop-Location }
    }
    if (-not (Test-Path $moteur)) { throw "moteur introuvable : $moteur" }
    "farfalauncherdeck $(Moteur @('version'))"
}

$okConnexion = Etape 'Launcher : connexion à RomM' {
    $info = Moteur @('config', '--serveur', $Serveur, '--gestionnaire', $Gestionnaire, '--dossier', $jeux)
    if ($info.connected -ne $true) { throw "non connecté (connected = $($info.connected))" }
    if ($Discret) { "launcher $($info.version)" } else { "launcher $($info.version), compte $($info.username)" }
} -Si $okMoteur

$idJeu = $null
$okListe = Etape 'Launcher : liste des jeux' {
    $consoles = @((Moteur @('consoles')).consoles)
    $c = $consoles | Where-Object { $_.slug -eq $Console -and $_.hasEmulator } | Select-Object -First 1
    if (-not $c) { throw "console « $Console » absente ou sans émulateur pour ce PC ($($consoles.Count) consoles)" }
    $liste = @((Moteur @('jeux', "$($c.id)")).games)
    if ($liste.Count -eq 0) { throw "aucun jeu dans « $Console »" }
    $script:idJeu = if ($rom) { [long]$rom.id } else { [long]$liste[0].id }
    "$($consoles.Count) consoles jouables, « $Console » : $($liste.Count) jeux sur la première page"
} -Si $okConnexion

$okInstall = Etape 'Launcher : téléchargement et installation du jeu' {
    $r = Moteur @('installer', "$idJeu")
    if ($r.status -ne 'installed') { throw "état « $($r.status) » (choix d'exécutable demandé ?)" }
    $octets = (Get-ChildItem $jeux -Recurse -File | Measure-Object Length -Sum).Sum
    $nom = if ($Discret) { "jeu $idJeu" } else { "« $($r.game.name) »" }
    "$nom installé ({0:N1} Mo avec l'émulateur)" -f ($octets / 1MB)
} -Si $okListe

$null = Etape "Launcher : lancement ($Secondes s, sans rien envoyer au serveur)" {
    $r = Moteur @('essai', "$idJeu", "$Secondes")
    if ($r.alive -ne $true) { throw "le jeu s'est fermé avant $Secondes s" }
    $fenetre = if ($r.window) { 'fenêtre affichée' } else { 'pas de fenêtre vue' }
    if (-not $r.window -and $SurWindows) { "ATTENTION : le jeu a tenu $Secondes s mais $fenetre" }
    else { "le jeu a tenu $Secondes s, $fenetre, puis a été fermé" }
} -Si $okInstall

# ---------------------------------------------------------------------------
# Ménage : le dossier de travail est supprimé (le journal du moteur est gardé en cas d'échec)
# ---------------------------------------------------------------------------

$echecs = Resume
Remove-Item Env:FARFALAUNCHERDECK_JETON -ErrorAction SilentlyContinue
$journaux = Join-Path $config 'logs'
if ($okTravail -and $echecs -gt 0 -and (Test-Path $journaux)) {
    try {
        Effacer-DossierSmoke $garde
        Creer-DossierSmoke $garde
        Copy-Item (Join-Path $journaux '*') $garde -Recurse
        Write-Host "Journal du moteur gardé dans : $garde"
    } catch { Write-Host "Journal non gardé : $($_.Exception.Message)" -ForegroundColor Yellow }
}
if ($okTravail) {
    Start-Sleep -Seconds 2   # le jeu fermé libère ses fichiers
    try { Effacer-DossierSmoke $travail; Write-Host "Dossier de travail supprimé : $travail" }
    catch { Write-Host "Dossier de travail à supprimer à la main : $travail" -ForegroundColor Yellow }
}
exit ([int]($echecs -gt 0))
