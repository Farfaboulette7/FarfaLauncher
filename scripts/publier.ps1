<#
  Publier une nouvelle version du launcher : double-cliquer sur publier.bat.

  ⚠ Seulement APRÈS avoir testé soi-même la version de test (src-tauri\target-test\debug\farfalauncher.exe).

  Étapes (une ligne OK / ÉCHEC par étape ; au premier ÉCHEC, tout s'arrête) :
    1. On est sur la branche main, sans modification en cours, et à jour avec GitHub (git pull).
       Le réglage local [profile.dev] de src-tauri/Cargo.toml est toléré : il n'est jamais commité.
    2. Choix du type de version : correctif (+0.0.1, par défaut), +0.1.0 ou +1.0.0.
    3. scripts/bump-version.mjs met le nouveau numéro partout.
    4. Le bloc-notes s'ouvre sur docs/changelog/vX.Y.Z.md : écrire les notes pour les joueurs,
       enregistrer, FERMER le bloc-notes. Le script attend.
    5. Vérifications : types TypeScript (npx tsc) et tests Rust (cargo test, dans src-tauri\target-test).
    6. Commit « Version X.Y.Z » (fichiers précis, jamais « git commit -am ») et tag vX.Y.Z.
    7. Confirmation « oui », puis envoi sur GitHub : main, puis le tag. GitHub compile alors la
       version (release.yml, 20 à 40 minutes) et les launchers des amis se mettent à jour seuls.
    8. Annonce Discord (salon farfalauncher, webhook DISCORD_LAUNCHER du .env du gestionnaire,
       jamais affiché) : « disponible dans un petit moment ».
    9. Lien pour suivre la compilation.

  Options :
    -Essai            fait les vérifications et montre tout ce qui serait fait, sans rien changer :
                      ni version, ni notes, ni commit, ni tag, ni envoi, ni Discord.
    -Type correctif|mineure|majeure   évite la question du type de version.
    -SansTests        saute tsc et cargo test (seulement avec -Essai).
    -DossierCompilation <dossier>     dossier de compilation de cargo (par défaut src-tauri\target-test).
#>
param(
    [switch]$Essai,
    [ValidateSet('', 'correctif', 'mineure', 'majeure')][string]$Type = '',
    [switch]$SansTests,
    [string]$DossierCompilation = ''
)

$ErrorActionPreference = 'Stop'
[Console]::OutputEncoding = [Text.Encoding]::UTF8
[Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12

# ---------------------------------------------------------------------------
# Réglages
# ---------------------------------------------------------------------------
$Depot = Split-Path -Parent $PSScriptRoot                     # le dossier du launcher (parent de scripts\)
$DepotGitHub = 'Farfaboulette7/FarfaLauncher'
$EnvGestionnaire = 'Z:\FarfaRomManager\.env'                  # contient DISCORD_LAUNCHER (jamais affiché)
$SuiviRelease = "https://github.com/$DepotGitHub/actions/workflows/release.yml"
if (-not $DossierCompilation) { $DossierCompilation = Join-Path $Depot 'src-tauri\target-test' }
$Utf8SansBom = New-Object System.Text.UTF8Encoding $false

if ($SansTests -and -not $Essai) { Write-Host '-SansTests ne marche qu''avec -Essai : une vraie publication lance toujours les tests.' -ForegroundColor Red; exit 1 }

# ---------------------------------------------------------------------------
# Affichage
# ---------------------------------------------------------------------------
$script:Echecs = 0
# Après bump-version et avant le commit, un échec remet l'ancien numéro (voir Echec).
$script:VersionARemettre = ''

# Ligne de résultat d'une étape. En vrai, un ÉCHEC arrête tout (rien d'autre n'est fait).
# En essai, on continue pour montrer la suite.
function Ok([string]$Nom, [string]$Detail = '') { Write-Host ('[OK       ] {0}' -f $Nom) -ForegroundColor Green -NoNewline; Write-Host "  $Detail" }
function Attention([string]$Nom, [string]$Detail) { Write-Host ('[ATTENTION] {0}' -f $Nom) -ForegroundColor Yellow -NoNewline; Write-Host "  $Detail" }
function Echec([string]$Nom, [string]$Detail) {
    Write-Host ('[ÉCHEC    ] {0}' -f $Nom) -ForegroundColor Red -NoNewline; Write-Host "  $Detail"
    $script:Echecs++
    if (-not $Essai) {
        Write-Host ''
        if ($script:VersionARemettre) {
            # On remet l'ancien numéro avec le même outil (il ne touche que les lignes de version :
            # le réglage local de Cargo.toml reste). Les notes de version écrites sont gardées.
            $r = Lancer 'node' @('scripts/bump-version.mjs', $script:VersionARemettre)
            if ($LASTEXITCODE -eq 0) { Write-Host "Numéro de version remis à $($script:VersionARemettre). Les notes déjà écrites sont gardées (docs\changelog) et resserviront." -ForegroundColor Yellow }
            else { Write-Host "Impossible de remettre le numéro $($script:VersionARemettre) : lancer « node scripts/bump-version.mjs $($script:VersionARemettre) »." -ForegroundColor Red }
        }
        Write-Host 'Publication ARRÊTÉE. Rien n''a été envoyé sur GitHub ni sur Discord. Corriger, puis relancer publier.bat.' -ForegroundColor Red
        exit 1
    }
}
function Essai([string]$Texte) { Write-Host "[ESSAI    ] $Texte (rien n'est fait)" -ForegroundColor Magenta }
function Info([string]$Texte) { Write-Host "            $Texte" -ForegroundColor Gray }

# Lance un programme et renvoie ses lignes de sortie ; code de sortie dans $LASTEXITCODE.
# (Windows PowerShell 5.1 prendrait sinon tout message d'erreur d'un programme pour une erreur fatale.)
function Lancer([string]$Exe, [string[]]$Arguments) {
    $avant = $ErrorActionPreference; $ErrorActionPreference = 'Continue'
    try { $sortie = & $Exe @Arguments 2>&1 | ForEach-Object { "$_" } } finally { $ErrorActionPreference = $avant }
    return $sortie
}
# (Nom « Git-Depot » et pas « Git » : une fonction Git cacherait le programme git.)
# core.safecrlf=false : pas d'avertissements « LF will be replaced by CRLF » mêlés aux réponses.
function Git-Depot([string[]]$Arguments) { Lancer 'git' (@('-C', $Depot, '-c', 'core.safecrlf=false') + $Arguments) }

# Lit une valeur NOM=valeur du .env du gestionnaire, sans jamais l'afficher.
function Lire-Env([string]$Nom) {
    if (-not (Test-Path -LiteralPath $EnvGestionnaire)) { return $null }
    foreach ($ligne in [System.IO.File]::ReadAllLines($EnvGestionnaire)) {
        if ($ligne.StartsWith("$Nom=")) { $v = $ligne.Substring($Nom.Length + 1).Trim().Trim('"').Trim("'"); if ($v) { return $v } }
    }
    return $null
}

Write-Host ''
Write-Host '=== Publier une nouvelle version du launcher ===' -ForegroundColor Cyan
Write-Host ''
if ($Essai) { Write-Host 'MODE ESSAI : rien ne sera changé, ni envoyé.' -ForegroundColor Magenta; Write-Host '' }
Write-Host 'Rappel : publier seulement une version que tu as testée toi-même.' -ForegroundColor Yellow
Write-Host ''

Set-Location -LiteralPath $Depot

# ---------------------------------------------------------------------------
# 1. Branche main, propre, à jour
# ---------------------------------------------------------------------------
$branche = ((Git-Depot @('rev-parse', '--abbrev-ref', 'HEAD')) -join '').Trim()
if ($branche -eq 'main') { Ok 'Branche main' }
else { Echec 'Branche main' "on est sur « $branche ». Publier se fait depuis main (ne pas changer de branche ici si une autre session y travaille)." }

# Modifications en cours. Seule exception : src-tauri/Cargo.toml, si sa ligne version est
# celle du dernier commit (le réglage local [profile.dev] du propriétaire, jamais commité).
$cargoLocal = $false
# « git diff --name-only HEAD » : fichiers suivis dont le CONTENU a changé (une simple différence
# de fins de ligne, après un retour en arrière de bump-version, ne compte pas).
$modifs = @(Git-Depot @('diff', '--name-only', 'HEAD') | Where-Object { $_ })
$autres = @()
foreach ($m in $modifs) {
    $f = $m.Trim('"')
    if ($f -eq 'src-tauri/Cargo.toml') {
        $versionHead = ((Git-Depot @('show', 'HEAD:src-tauri/Cargo.toml')) | Where-Object { $_ -match '^version\s*=' } | Select-Object -First 1)
        $versionIci = (Get-Content -LiteralPath (Join-Path $Depot 'src-tauri\Cargo.toml') -Encoding UTF8 | Where-Object { $_ -match '^version\s*=' } | Select-Object -First 1)
        if ($versionHead -eq $versionIci) { $cargoLocal = $true; continue }
    }
    $autres += $m
}
if ($autres.Count -gt 0) {
    Echec 'Aucune modification en cours' "à commiter ou annuler d'abord : $(($autres | Select-Object -First 6) -join ' ; ')"
} else {
    Ok 'Aucune modification en cours' $(if ($cargoLocal) { 'réglage local de src-tauri/Cargo.toml toléré (il ne sera pas commité)' } else { '' })
}
$nonSuivis = @(Git-Depot @('status', '--porcelain') | Where-Object { $_ -and $_.StartsWith('??') } | ForEach-Object { $_.Substring(3) })
if ($nonSuivis.Count -gt 0) { Info "fichiers non suivis par git, ignorés : $(($nonSuivis | Select-Object -First 6) -join ', ')" }

# À jour avec GitHub. Le support des rapports de bug publie parfois lui-même des bêtas sur main :
# il faut les récupérer avant de choisir le numéro.
if ($Essai) {
    $r = Git-Depot @('fetch', '-q', 'origin')
    if ($LASTEXITCODE -ne 0) { Echec 'À jour avec GitHub' "git fetch : $($r -join ' ')" }
    else {
        $enRetard = ((Git-Depot @('rev-list', '--count', 'HEAD..origin/main')) -join '').Trim()
        Essai "ferait « git pull » ($enRetard commit(s) de GitHub à récupérer)"
    }
} else {
    # --autostash : met de côté le réglage local de Cargo.toml le temps de la mise à jour, puis le remet.
    $r = Git-Depot @('pull', '--ff-only', '--autostash', 'origin', 'main')
    if ($LASTEXITCODE -ne 0) { Echec 'À jour avec GitHub (git pull)' "$(($r | Select-Object -Last 3) -join ' ')" }
    else { Ok 'À jour avec GitHub (git pull)' (($r | Select-Object -Last 1) -join '') }
}
$enAvance = ((Git-Depot @('rev-list', '--count', 'origin/main..HEAD')) -join '').Trim()
if ($enAvance -ne '0') { Info "$enAvance commit(s) local(aux) pas encore sur GitHub : ils partiront avec la version." }

# ---------------------------------------------------------------------------
# 2. Numéro de version
# ---------------------------------------------------------------------------
$actuelle = ((Lancer 'node' @('-p', "require('./package.json').version")) -join '').Trim()
if ($actuelle -notmatch '^(\d+)\.(\d+)\.(\d+)(-beta\.\d+)?$') { Echec 'Version actuelle' "illisible dans package.json : « $actuelle »" }
$maj = [int]$Matches[1]; $min = [int]$Matches[2]; $cor = [int]$Matches[3]; $beta = [bool]$Matches[4]
Ok 'Version actuelle' $actuelle

if (-not $Type) {
    Write-Host ''
    Write-Host 'Quel type de version ?' -ForegroundColor Yellow
    Write-Host '  1 = correctif   (+0.0.1, des corrections et petites nouveautés)  ← par défaut'
    Write-Host '  2 = mineure     (+0.1.0, une grosse nouveauté)'
    Write-Host '  3 = majeure     (+1.0.0, un grand changement)'
    $choix = (Read-Host 'Tape 1, 2 ou 3 (Entrée = 1)').Trim()
    $Type = switch ($choix) { '2' { 'mineure' } '3' { 'majeure' } default { 'correctif' } }
}
# Une bêta X.Y.Z-beta.N devient la stable X.Y.Z (son correctif), sinon on ajoute au bon chiffre.
$nouvelle = switch ($Type) {
    'correctif' { if ($beta) { "$maj.$min.$cor" } else { "$maj.$min.$($cor + 1)" } }
    'mineure'   { "$maj.$($min + 1).0" }
    'majeure'   { "$($maj + 1).0.0" }
}
$Tag = "v$nouvelle"
$tagLocal = ((Git-Depot @('tag', '--list', $Tag)) -join '').Trim()
$tagDistant = ((Git-Depot @('ls-remote', '--tags', 'origin', "refs/tags/$Tag")) -join '').Trim()
if ($tagLocal -or $tagDistant) { Echec 'Nouvelle version' "$Tag existe déjà (sur ce PC ou sur GitHub) : choisir un autre type" }
else { Ok 'Nouvelle version' "$actuelle → $nouvelle ($Type)" }

# ---------------------------------------------------------------------------
# 3. bump-version.mjs
# ---------------------------------------------------------------------------
if ($Essai) {
    Essai "lancerait « node scripts/bump-version.mjs $nouvelle » (package.json, package-lock.json, tauri.conf.json, Cargo.toml, Cargo.lock)"
} else {
    $r = Lancer 'node' @('scripts/bump-version.mjs', $nouvelle)
    $script:VersionARemettre = $actuelle
    if ($LASTEXITCODE -ne 0) { Echec 'Numéro de version dans les fichiers' ($r -join ' ') }
    else { Ok 'Numéro de version dans les fichiers' "$nouvelle (package.json, tauri.conf.json, Cargo.toml…)" }
}

# ---------------------------------------------------------------------------
# 4. Notes de version (lues par les joueurs)
# ---------------------------------------------------------------------------
$Notes = Join-Path $Depot "docs\changelog\$Tag.md"
$Modele = @"
Téléchargez le fichier correspondant à votre système ci-dessous. Si FaRFa project est déjà installé, il se met à jour tout seul.

## Nouveautés
- (à remplir : une ligne par nouveauté, pour les joueurs ; supprimer la section s'il n'y en a pas)

## Corrections
- (à remplir : une ligne par correction ; supprimer la section s'il n'y en a pas)
"@
if ($Essai) {
    Essai "créerait $Notes (modèle « Nouveautés / Corrections » s'il n'existe pas) et l'ouvrirait dans le bloc-notes"
} else {
    if (-not (Test-Path -LiteralPath $Notes)) { [System.IO.File]::WriteAllText($Notes, $Modele.Replace("`r`n", "`n") + "`n", $Utf8SansBom) }
    Write-Host ''
    Write-Host "Le bloc-notes s'ouvre : écris les notes de version (en français, pour les joueurs)," -ForegroundColor Yellow
    Write-Host 'enregistre (Ctrl+S), puis FERME le bloc-notes pour continuer.' -ForegroundColor Yellow
    # Tant que les notes ne sont pas remplies, on rouvre le bloc-notes (ou on arrête tout).
    while ($true) {
        Start-Process notepad.exe -ArgumentList "`"$Notes`"" -Wait
        $texte = [System.IO.File]::ReadAllText($Notes)
        $probleme = if ($texte -match '\(à remplir') { 'il reste des lignes « (à remplir…) »' } elseif ($texte.Trim().Length -lt 20) { 'le fichier est presque vide' } else { '' }
        if (-not $probleme) { break }
        Write-Host "Notes pas finies : $probleme." -ForegroundColor Yellow
        $r = Read-Host 'Entrée pour rouvrir le bloc-notes, ou tape « annuler » pour tout arrêter'
        if ($r.Trim().ToLower() -eq 'annuler') { Echec 'Notes de version' "pas finies ($probleme)" }
    }
    Ok 'Notes de version' $Notes
}

# ---------------------------------------------------------------------------
# 5. Vérifications : types TypeScript et tests Rust
# ---------------------------------------------------------------------------
if ($SansTests) {
    Essai 'lancerait npx tsc --noEmit -p . puis cargo test (sautés : -SansTests)'
} else {
    if (-not (Test-Path -LiteralPath (Join-Path $Depot 'node_modules'))) {
        Echec 'Dépendances de l''interface' 'node_modules absent : lancer « npm install » dans ce dossier, puis relancer'
    }
    # L'interface compilée (dist\) est embarquée dans le Rust : sans elle, cargo ne compile pas.
    if (-not (Test-Path -LiteralPath (Join-Path $Depot 'dist'))) {
        Info 'dist\ absent : compilation de l''interface (npm run build)…'
        $r = Lancer 'npm.cmd' @('run', 'build')
        if ($LASTEXITCODE -ne 0) { Echec 'Compilation de l''interface (npm run build)' (($r | Select-Object -Last 4) -join ' ') }
    }
    Info 'Types TypeScript (npx tsc)…'
    $r = Lancer 'npx.cmd' @('tsc', '--noEmit', '-p', '.')
    if ($LASTEXITCODE -ne 0) { Echec 'Types TypeScript (tsc)' (($r | Select-Object -First 4) -join ' ') }
    else { Ok 'Types TypeScript (tsc)' 'aucune erreur' }

    Info "Tests Rust (cargo test, dans $DossierCompilation) : 1 à 10 minutes…"
    $env:CARGO_TARGET_DIR = $DossierCompilation
    $r = Lancer 'cargo' @('test', '--manifest-path', (Join-Path $Depot 'src-tauri\Cargo.toml'))
    $codeTests = $LASTEXITCODE
    Remove-Item Env:\CARGO_TARGET_DIR
    # Le cache incrémental grossit vite (16 Go vus) : on le vide après chaque passage.
    $inc = Join-Path $DossierCompilation 'debug\incremental'
    if (Test-Path -LiteralPath $inc) { Remove-Item -LiteralPath $inc -Recurse -Force -ErrorAction SilentlyContinue }
    $bilan = @($r | Where-Object { $_ -match '^test result:' })
    if ($codeTests -ne 0) {
        $rates = @($r | Where-Object { $_ -match '^test .* FAILED$|^error(\[E\d+\])?:' } | Select-Object -First 5)
        Echec 'Tests Rust (cargo test)' "$($rates -join ' ; ') $(($bilan | Select-Object -Last 1)) (voir docs/tests.md, « Lire un échec »)"
    } else {
        $passes = ($bilan | ForEach-Object { if ($_ -match '(\d+) passed') { [int]$Matches[1] } else { 0 } } | Measure-Object -Sum).Sum
        Ok 'Tests Rust (cargo test)' "$passes test(s) réussi(s)"
    }
}

# ---------------------------------------------------------------------------
# 6. Commit et tag
# ---------------------------------------------------------------------------
# Fichiers précis : jamais « git commit -am » (le réglage local de Cargo.toml ne doit pas partir).
$fichiers = @('package.json', 'package-lock.json', 'src-tauri/tauri.conf.json', 'src-tauri/Cargo.lock', "docs/changelog/$Tag.md")
if ($Essai) {
    Essai "commiterait « Version $nouvelle » : $($fichiers -join ', '), src-tauri/Cargo.toml (seulement sa ligne version$(if ($cargoLocal) { ', le réglage local reste en dehors' }))"
    Essai "poserait le tag $Tag"
} else {
    $r = Git-Depot (@('add', '--') + $fichiers)
    if ($LASTEXITCODE -ne 0) { Echec 'Préparation du commit' ($r -join ' ') }
    if ($cargoLocal) { $r = Lancer 'node' @('scripts/indexer-version-cargo.mjs', $nouvelle) }
    else { $r = Git-Depot @('add', '--', 'src-tauri/Cargo.toml') }
    if ($LASTEXITCODE -ne 0) { Echec 'Préparation du commit (Cargo.toml)' ($r -join ' ') }
    $r = Git-Depot @('commit', '-q', '-m', "Version $nouvelle")
    if ($LASTEXITCODE -ne 0) { Echec 'Commit' ($r -join ' ') }
    $r = Git-Depot @('tag', $Tag)
    if ($LASTEXITCODE -ne 0) { Echec "Tag $Tag" ($r -join ' ') }
    $script:VersionARemettre = ''   # le commit est fait : plus de retour automatique en arrière
    Ok "Commit « Version $nouvelle » et tag $Tag" ((Git-Depot @('log', '-1', '--format=%h')) -join '')
}

# ---------------------------------------------------------------------------
# 7. Envoi sur GitHub (après « oui »)
# ---------------------------------------------------------------------------
if ($Essai) {
    Essai "demanderait « oui », puis : git push origin main, puis git push origin $Tag"
} else {
    Write-Host ''
    Write-Host "Prêt à publier $Tag pour TOUS les joueurs." -ForegroundColor Yellow
    $r = Read-Host 'Tape « oui » pour envoyer sur GitHub (autre chose pour s''arrêter là)'
    if ($r.Trim().ToLower() -ne 'oui') {
        Write-Host ''
        Write-Host 'Arrêté : rien n''est parti sur GitHub. Le commit et le tag restent sur ce PC.' -ForegroundColor Yellow
        Write-Host "  Pour publier plus tard : git push origin main  puis  git push origin $Tag  (puis l'annonce Discord à la main)"
        Write-Host "  Pour tout annuler      : git tag -d $Tag  puis  git reset --keep HEAD~1"
        exit 0
    }
    $r = Git-Depot @('push', 'origin', 'main')
    if ($LASTEXITCODE -ne 0) { Echec 'Envoi de main' "$(($r | Select-Object -Last 3) -join ' ') — souvent : GitHub a reçu d'autres commits entre-temps ; « git pull » puis « git push origin main » et « git push origin $Tag »." }
    else { Ok 'Envoi de main sur GitHub' }
    $r = Git-Depot @('push', 'origin', $Tag)
    if ($LASTEXITCODE -ne 0) { Echec "Envoi du tag $Tag" "$(($r | Select-Object -Last 3) -join ' ') — relancer « git push origin $Tag »." }
    else { Ok "Envoi du tag $Tag : GitHub compile la version" }
}

# ---------------------------------------------------------------------------
# 8. Annonce Discord (salon farfalauncher)
# ---------------------------------------------------------------------------
# Juste après le tag, sans attendre la compilation : l'annonce dit « disponible dans un petit moment ».
$lienRelease = "https://github.com/$DepotGitHub/releases/tag/$Tag"
if ($Essai) {
    $source = if (Test-Path -LiteralPath $Notes) { $Notes } else { $null }
    $corpsNotes = if ($source) { [System.IO.File]::ReadAllText($source) } else { '(les notes écrites à l''étape 4)' }
} else {
    $corpsNotes = [System.IO.File]::ReadAllText($Notes)
}
# Sans la première ligne « Téléchargez… », qui s'adresse aux visiteurs de la page GitHub.
$corpsNotes = ($corpsNotes -replace '(?s)^\s*Téléchargez[^\n]*\n', '').Trim()
if ($corpsNotes.Length -gt 3500) { $corpsNotes = $corpsNotes.Substring(0, 3500) + "…`n(suite sur GitHub)" }
$description = "$corpsNotes`n`n**Disponible dans un petit moment** : le launcher se mettra à jour tout seul."
$embed = @{ title = "FaRFa project $nouvelle"; url = $lienRelease; description = $description; color = 5814783 }
$corps = @{ embeds = @($embed) } | ConvertTo-Json -Depth 5

$webhook = Lire-Env 'DISCORD_LAUNCHER'
if ($Essai) {
    if ($webhook) { Essai 'enverrait l''annonce sur le salon farfalauncher (webhook DISCORD_LAUNCHER trouvé dans le .env)' }
    else { Echec 'Annonce Discord' "webhook DISCORD_LAUNCHER introuvable dans $EnvGestionnaire (Z: connecté ?)" }
    Info "Titre : FaRFa project $nouvelle  ($lienRelease)"
    ($description -split "`n") | ForEach-Object { Info $_ }
} elseif (-not $webhook) {
    Attention 'Annonce Discord' "webhook DISCORD_LAUNCHER introuvable dans $EnvGestionnaire (Z: connecté ?) : la version est publiée, faire l'annonce à la main"
} else {
    try {
        Invoke-RestMethod -Uri $webhook -Method Post -ContentType 'application/json; charset=utf-8' `
            -Body ([System.Text.Encoding]::UTF8.GetBytes($corps)) -UserAgent 'FarfaOutils' | Out-Null
        Ok 'Annonce Discord (salon farfalauncher)'
    } catch {
        # Le message d'erreur ne contient pas l'adresse du webhook.
        Attention 'Annonce Discord' "échec de l'envoi ($($_.Exception.Message)) : la version est publiée, faire l'annonce à la main"
    }
}
$webhook = $null

# ---------------------------------------------------------------------------
# 9. Suivi
# ---------------------------------------------------------------------------
Write-Host ''
if ($Essai) {
    if ($script:Echecs -gt 0) { Write-Host "Essai terminé : $($script:Echecs) échec(s) à corriger avant une vraie publication." -ForegroundColor Red; exit 1 }
    Write-Host 'Essai terminé : tout est prêt pour une vraie publication.' -ForegroundColor Green
    exit 0
}
Write-Host "Version $nouvelle envoyée." -ForegroundColor Green
Write-Host "Suivre la compilation (20 à 40 min) : $SuiviRelease"
Write-Host "Page de la version (remplie à la fin) : $lienRelease"
Write-Host 'Si la compilation échoue (croix rouge) : rien n''est publié aux joueurs ; voir docs/tests.md, « Lire un échec de CI ».'
exit 0
