@echo off
rem Starts Sculpt, building it first if it has not been built yet.
cd /d "%~dp0"
if not exist "target\release\sculpt.exe" (
    echo Building Sculpt for the first time. This takes a minute or two...
    cargo build --release
    if errorlevel 1 (
        echo.
        echo The build failed. See the messages above.
        pause
        exit /b 1
    )
)
start "" "target\release\sculpt.exe" %*
