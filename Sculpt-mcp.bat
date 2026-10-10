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
if not exist "%SERVER%" (
    echo Sculpt has not been built yet. Run Sculpt.bat once first. 1>&2
    exit /b 1
)
"%SERVER%" --mcp --app "%APP%"
