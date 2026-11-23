@echo off
powershell.exe -NoProfile -ExecutionPolicy Bypass -File "%~dp0Start-TV.ps1" -Mirror -Silent -Name "Windows TV Mirror" %*
