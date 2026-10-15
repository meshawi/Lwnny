@echo off
rem Detect the RAM sticks over SMBus (needs administrator rights).
rem Right-click this file and choose "Run as administrator".
set LOG=%TEMP%\lwnny-probe-ram.txt
set EXE=%~dp0..\src-tauri\target\debug\probe.exe
if not exist "%EXE%" set EXE=%~dp0..\src-tauri\target\release\probe.exe
echo Running %EXE% list
"%EXE%" list > "%LOG%" 2>&1
type "%LOG%"
echo.
echo Output saved to %LOG%
pause
