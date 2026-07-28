@ECHO OFF
SETLOCAL EnableDelayedExpansion

if not defined PONDUIN_NODE_DIR (
    SET "PONDUIN_NODE_DIR=%LOCALAPPDATA%\Ponduin\node"
)
SET "NODE_VERSION=22.14.0"

REM === Check for previously downloaded portable Node.js (matching version) ===
if exist "%PONDUIN_NODE_DIR%\node-v%NODE_VERSION%.installed" (
    SET "PATH=%PONDUIN_NODE_DIR%;!PATH!"
    "%PONDUIN_NODE_DIR%\npx.cmd" %*
    exit /b !errorlevel!
)

REM === Download portable Node.js ===
echo [Ponduin] Node.js not found. Downloading portable Node.js v%NODE_VERSION%... 1>&2

SET "NODE_ZIP=%TEMP%\ponduin-node-%NODE_VERSION%.zip"
SET "NODE_EXTRACT=%TEMP%\ponduin-node-extract"

powershell -NoProfile -Command "$ProgressPreference='SilentlyContinue'; try { [Net.ServicePointManager]::SecurityProtocol=[Net.SecurityProtocolType]::Tls12; Invoke-WebRequest -Uri 'https://nodejs.org/dist/v%NODE_VERSION%/node-v%NODE_VERSION%-win-x64.zip' -OutFile '%NODE_ZIP%' -UseBasicParsing; Expand-Archive -Path '%NODE_ZIP%' -DestinationPath '%NODE_EXTRACT%' -Force; exit 0 } catch { Write-Error $_.Exception.Message; exit 1 }"
if errorlevel 1 (
    echo [Ponduin] ERROR: Failed to download Node.js. Please install manually from https://nodejs.org/ 1>&2
    del "%NODE_ZIP%" >nul 2>&1
    exit /b 1
)

REM Clean previous version and install to Ponduin directory
rmdir /s /q "%PONDUIN_NODE_DIR%" >nul 2>&1
mkdir "%PONDUIN_NODE_DIR%" >nul 2>&1
xcopy /s /e /q /y "%NODE_EXTRACT%\node-v%NODE_VERSION%-win-x64\*" "%PONDUIN_NODE_DIR%\" >nul 2>&1

REM Clean up
del "%NODE_ZIP%" >nul 2>&1
rmdir /s /q "%NODE_EXTRACT%" >nul 2>&1

if exist "%PONDUIN_NODE_DIR%\npx.cmd" (
    echo.>"%PONDUIN_NODE_DIR%\node-v%NODE_VERSION%.installed"
    SET "PATH=%PONDUIN_NODE_DIR%;!PATH!"
    echo [Ponduin] Node.js v%NODE_VERSION% ready. 1>&2
    "%PONDUIN_NODE_DIR%\npx.cmd" %*
    exit /b !errorlevel!
)

echo [Ponduin] ERROR: Installation failed. Please install Node.js manually from https://nodejs.org/ 1>&2
exit /b 1
