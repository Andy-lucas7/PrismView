@echo off
rem Uso: gate.cmd ts|rust|both   (saida condensada, so erros novos)
powershell -NoProfile -ExecutionPolicy Bypass -File "%~dp0gate.ps1" -Gate %1
exit /b %ERRORLEVEL%
