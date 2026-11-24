@echo off
rem Publier une nouvelle version du launcher (apres l'avoir testee soi-meme).
rem Double-cliquer. Options possibles : publier.bat -Essai   (montre tout sans rien changer)
rem Le travail est fait par publier.ps1 (meme dossier). Voir docs\outils.md.
chcp 65001 >nul
powershell -NoProfile -ExecutionPolicy Bypass -File "%~dp0publier.ps1" %*
echo.
echo Appuie sur une touche pour fermer cette fenetre...
pause >nul
