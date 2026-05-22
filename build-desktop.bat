@echo off
setlocal

powershell -NoProfile -ExecutionPolicy Bypass -File "%~dp0scripts\package-desktop.ps1" %*
exit /b %ERRORLEVEL%
