@echo off
rem Launcher: dispensa associacao de .ps1 e politica de execucao. Aceita -From/-Only/-List/-MaxFix.
powershell -NoProfile -ExecutionPolicy Bypass -File "%~dp0run.ps1" %*
