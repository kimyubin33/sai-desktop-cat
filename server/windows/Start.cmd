@echo off
setlocal
pushd "%~dp0"
"Sai-Team-Server.exe" setup
set "rc=%errorlevel%"
popd
if not "%rc%"=="0" pause
exit /b %rc%
