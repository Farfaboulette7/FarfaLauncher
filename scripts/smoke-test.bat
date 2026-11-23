@echo off
rem Smoke test du Farfa project : double-cliquer pour verifier RomM, le gestionnaire,
rem et le launcher (connexion, liste, telechargement, lancement). Voir docs\smoke-test.md.
rem Les options de smoke-test.ps1 peuvent suivre : smoke-test.bat -Console nes -Secondes 20
chcp 65001 >nul
powershell -NoProfile -ExecutionPolicy Bypass -File "%~dp0smoke-test.ps1" %*
echo.
pause
