# Voice Keyboard Setup Script for Windows
# Run in PowerShell as Administrator:
#   Set-ExecutionPolicy Bypass -Scope Process -Force
#   .\setup-windows.ps1

param(
    [switch]$SkipBuildTools,
    [switch]$SkipRust,
    [switch]$SkipModel
)

$ErrorActionPreference = "Stop"

# Configuration
$ModelName = "ggml-large-v3-turbo.bin"
$ModelUrl = "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/$ModelName"
$ModelsDir = "$env:LOCALAPPDATA\voice-keyboard\models"
$InstallDir = "$env:USERPROFILE\voice-keyboard"

# Colors
function Write-ColorOutput($ForegroundColor) {
    $fc = $host.UI.RawUI.ForegroundColor
    $host.UI.RawUI.ForegroundColor = $ForegroundColor
    if ($args) {
        Write-Output $args
    }
    $host.UI.RawUI.ForegroundColor = $fc
}

function Write-Step($step, $total, $message) {
    Write-Host "`n[$step/$total] " -ForegroundColor Cyan -NoNewline
    Write-Host $message -ForegroundColor White
}

function Write-Success($message) {
    Write-Host "  ✓ $message" -ForegroundColor Green
}

function Write-Warning($message) {
    Write-Host "  ! $message" -ForegroundColor Yellow
}

function Write-Error($message) {
    Write-Host "  ✗ $message" -ForegroundColor Red
}

# Header
Write-Host ""
Write-Host "╔══════════════════════════════════════════╗" -ForegroundColor Cyan
Write-Host "║    Voice Keyboard Setup for Windows      ║" -ForegroundColor Cyan
Write-Host "╚══════════════════════════════════════════╝" -ForegroundColor Cyan
Write-Host ""

# Check Administrator
$isAdmin = ([Security.Principal.WindowsPrincipal] [Security.Principal.WindowsIdentity]::GetCurrent()).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)
if (-not $isAdmin) {
    Write-Warning "Not running as Administrator. Some features may not work."
    Write-Host "  Consider running: Start-Process powershell -Verb RunAs" -ForegroundColor Gray
}

# Step 1: Check/Install Visual Studio Build Tools
Write-Step 1 6 "Checking Visual Studio Build Tools..."

$vsWhere = "${env:ProgramFiles(x86)}\Microsoft Visual Studio\Installer\vswhere.exe"
$hasBuildTools = $false

if (Test-Path $vsWhere) {
    $vsPath = & $vsWhere -latest -products * -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath
    if ($vsPath) {
        $hasBuildTools = $true
        Write-Success "Visual Studio Build Tools found"
    }
}

if (-not $hasBuildTools -and -not $SkipBuildTools) {
    Write-Warning "Visual Studio Build Tools not found"
    Write-Host "  Please install from: https://visualstudio.microsoft.com/visual-cpp-build-tools/" -ForegroundColor Gray
    Write-Host "  Select: 'Desktop development with C++'" -ForegroundColor Gray
    Write-Host ""

    $response = Read-Host "  Open download page? (y/n)"
    if ($response -eq 'y') {
        Start-Process "https://visualstudio.microsoft.com/visual-cpp-build-tools/"
        Write-Host ""
        Write-Host "  After installation, run this script again." -ForegroundColor Yellow
        exit 0
    }
}

# Step 2: Check/Install CMake
Write-Step 2 6 "Checking CMake..."

if (Get-Command cmake -ErrorAction SilentlyContinue) {
    Write-Success "CMake is installed"
} else {
    Write-Warning "CMake not found. Installing via winget..."
    try {
        winget install Kitware.CMake --silent --accept-package-agreements --accept-source-agreements
        # Refresh PATH
        $env:Path = [System.Environment]::GetEnvironmentVariable("Path","Machine") + ";" + [System.Environment]::GetEnvironmentVariable("Path","User")
        Write-Success "CMake installed"
    } catch {
        Write-Error "Failed to install CMake"
        Write-Host "  Please install manually from: https://cmake.org/download/" -ForegroundColor Gray
    }
}

# Step 3: Check/Install Rust
Write-Step 3 6 "Checking Rust..."

if (Get-Command cargo -ErrorAction SilentlyContinue) {
    Write-Success "Rust is installed"
    # Update to latest
    rustup update stable 2>$null
} elseif (-not $SkipRust) {
    Write-Warning "Rust not found. Installing..."
    try {
        # Download and run rustup-init
        $rustupInit = "$env:TEMP\rustup-init.exe"
        Invoke-WebRequest -Uri "https://win.rustup.rs/x86_64" -OutFile $rustupInit
        Start-Process -FilePath $rustupInit -ArgumentList "-y" -Wait

        # Refresh PATH
        $env:Path = [System.Environment]::GetEnvironmentVariable("Path","Machine") + ";" + [System.Environment]::GetEnvironmentVariable("Path","User")
        $env:Path += ";$env:USERPROFILE\.cargo\bin"

        Write-Success "Rust installed"
    } catch {
        Write-Error "Failed to install Rust"
        Write-Host "  Please install manually from: https://rustup.rs/" -ForegroundColor Gray
        exit 1
    }
}

# Step 4: Clone repository
Write-Step 4 6 "Cloning repository..."

if (Test-Path $InstallDir) {
    Write-Warning "Directory exists, updating..."
    Push-Location $InstallDir
    git pull
    Pop-Location
} else {
    git clone https://github.com/alexmak/voice-keyboard.git $InstallDir
}
Write-Success "Repository ready"

# Step 5: Build
Write-Step 5 6 "Building (this may take several minutes)..."

Push-Location $InstallDir
try {
    cargo build --release --features "whisper,opus"
    Write-Success "Build complete"
} catch {
    Write-Error "Build failed: $_"
    Pop-Location
    exit 1
}
Pop-Location

# Step 6: Download model
Write-Step 6 6 "Downloading Whisper model..."

if (-not (Test-Path $ModelsDir)) {
    New-Item -ItemType Directory -Force -Path $ModelsDir | Out-Null
}

$modelPath = Join-Path $ModelsDir $ModelName

if ((Test-Path $modelPath) -and -not $SkipModel) {
    Write-Warning "Model already exists, skipping download"
} elseif (-not $SkipModel) {
    Write-Host "  Downloading $ModelName (1.6 GB)..." -ForegroundColor Gray
    Write-Host "  This may take a few minutes..." -ForegroundColor Gray

    try {
        # Use BITS for better download experience
        Start-BitsTransfer -Source $ModelUrl -Destination $modelPath -Description "Downloading Whisper model"
        Write-Success "Model downloaded"
    } catch {
        # Fallback to Invoke-WebRequest
        Write-Warning "BITS failed, using alternative download..."
        Invoke-WebRequest -Uri $ModelUrl -OutFile $modelPath
        Write-Success "Model downloaded"
    }
}

# Create shortcut
$shortcutPath = "$env:USERPROFILE\Desktop\Voice Typer.lnk"
$WshShell = New-Object -comObject WScript.Shell
$Shortcut = $WshShell.CreateShortcut($shortcutPath)
$Shortcut.TargetPath = "$InstallDir\target\release\voice-typer.exe"
$Shortcut.WorkingDirectory = $InstallDir
$Shortcut.Save()

# Add to PATH
$userPath = [Environment]::GetEnvironmentVariable("Path", "User")
$binPath = "$InstallDir\target\release"
if ($userPath -notlike "*$binPath*") {
    [Environment]::SetEnvironmentVariable("Path", "$userPath;$binPath", "User")
    Write-Success "Added to PATH"
}

# Final instructions
Write-Host ""
Write-Host "╔══════════════════════════════════════════════════════════════╗" -ForegroundColor Green
Write-Host "║                    Installation Complete!                     ║" -ForegroundColor Green
Write-Host "╚══════════════════════════════════════════════════════════════╝" -ForegroundColor Green
Write-Host ""

Write-Host "To run Voice Keyboard:" -ForegroundColor Cyan
Write-Host "  - Double-click 'Voice Typer' shortcut on Desktop"
Write-Host "  - Or run: $InstallDir\target\release\voice-typer.exe"
Write-Host "  - Or in new terminal: voice-typer"
Write-Host ""

Write-Host "Usage:" -ForegroundColor Cyan
Write-Host "  1. Press and hold Fn key (or configured hotkey)"
Write-Host "  2. Speak"
Write-Host "  3. Release key - text appears in focused app"
Write-Host ""

Write-Host "Notes:" -ForegroundColor Yellow
Write-Host "  - Windows support is experimental"
Write-Host "  - If hotkey doesn't work, try running as Administrator"
Write-Host "  - Some antivirus may flag keyboard simulation - add exception if needed"
Write-Host ""

Write-Host "Configuration file: $env:APPDATA\voice-keyboard\config.json" -ForegroundColor Gray
Write-Host "Models directory: $ModelsDir" -ForegroundColor Gray
Write-Host ""
