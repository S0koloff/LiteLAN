@echo off
powershell.exe -NoProfile -Command "Start-Process -FilePath (Join-Path '%~dp0' 'LiteLAN.exe') -Verb RunAs"
