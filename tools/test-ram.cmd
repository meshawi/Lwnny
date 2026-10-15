@echo off
rem Write colors to the RAM sticks (needs administrator rights).
rem Right-click this file and choose "Run as administrator".
set LOG=%TEMP%\lwnny-test-ram.txt
set EXE=%~dp0..\src-tauri\target\debug\probe.exe
if not exist "%EXE%" set EXE=%~dp0..\src-tauri\target\release\probe.exe

:menu
echo.
echo  RAM test
echo   1  all sticks red
echo   2  all sticks green
echo   3  all sticks blue
echo   4  each stick its own color (1 red, 2 green, 3 blue, 4 white)
echo   5  all sticks white
echo   6  speed test (reads only, nothing changes)
echo   0  quit
set /p PICK=Pick a number: 
if "%PICK%"=="0" goto end
if "%PICK%"=="1" "%EXE%" set ram ff0000 > "%LOG%" 2>&1
if "%PICK%"=="2" "%EXE%" set ram 00ff00 > "%LOG%" 2>&1
if "%PICK%"=="3" "%EXE%" set ram 0000ff > "%LOG%" 2>&1
if "%PICK%"=="4" (
  "%EXE%" zone ram 0 ff0000 > "%LOG%" 2>&1
  "%EXE%" zone ram 1 00ff00 >> "%LOG%" 2>&1
  "%EXE%" zone ram 2 0000ff >> "%LOG%" 2>&1
  "%EXE%" zone ram 3 ffffff >> "%LOG%" 2>&1
)
if "%PICK%"=="5" "%EXE%" set ram ffffff > "%LOG%" 2>&1
if "%PICK%"=="6" "%EXE%" rambench > "%LOG%" 2>&1
findstr /B /C:"ram " "%LOG%"
findstr /C:"RAM write took" /C:"still busy" /C:"ms per byte" "%LOG%"
findstr /I /C:"error" /C:"failed" /C:"panic" "%LOG%"
goto menu

:end
echo Output saved to %LOG%
