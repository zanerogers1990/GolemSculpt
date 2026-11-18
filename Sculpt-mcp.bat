@echo off
rem The MCP server that lets Claude see and drive Sculpt. Claude starts this
rem itself (see .mcp.json); there is no need to run it by hand.
rem
rem It runs from a copy of the program, so a connected Claude session never
rem stops Sculpt from being rebuilt. Nothing here may print to the screen:
rem the output is the connection to Claude.
setlocal
set "APP=%~dp0target\release\sculpt.exe"
set "SERVER=%~dp0target\mcp\sculpt-mcp.exe"
if not exist "%~dp0target\mcp" mkdir "%~dp0target\mcp" >nul 2>&1
if exist "%APP%" copy /y "%APP%" "%SERVER%" >nul 2>&1
rem A server left running by another Claude session keeps its copy locked, and
rem the copy above then fails without a word, which would leave this one on the
rem old build. Start from a copy of its own instead, and sweep up old ones
rem (the ones still in use stay, as they cannot be deleted).
if exist "%APP%" if errorlevel 1 call :fresh
if not exist "%SERVER%" (
    echo Sculpt has not been built yet. Run Sculpt.bat once first. 1>&2
    exit /b 1
)
"%SERVER%" --mcp --app "%APP%"
exit /b

:fresh
del /q "%~dp0target\mcp\sculpt-mcp-*.exe" >nul 2>&1
set "SERVER=%~dp0target\mcp\sculpt-mcp-%RANDOM%%RANDOM%.exe"
copy /y "%APP%" "%SERVER%" >nul 2>&1
exit /b
