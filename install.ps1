$ErrorActionPreference = "Stop"

$Repo = "nixuuu/ralph-wiggum-rs"
$BinaryName = "ralph-wiggum"
$InstallDir = Join-Path $env:USERPROFILE ".local\bin"
$ExeName = "$BinaryName.exe"
$InstallPath = Join-Path $InstallDir $ExeName

# --- Helpers ---

function Write-Info($Message) {
    Write-Host "  ->  " -ForegroundColor Cyan -NoNewline
    Write-Host $Message
}

function Write-Success($Message) {
    Write-Host "  OK  " -ForegroundColor Green -NoNewline
    Write-Host $Message
}

function Write-Error2($Message) {
    Write-Host "  ERR " -ForegroundColor Red -NoNewline
    Write-Host $Message
}

# --- Platform detection ---

function Assert-Platform {
    if (-not [System.Environment]::Is64BitOperatingSystem) {
        Write-Error2 "ralph-wiggum requires a 64-bit operating system."
        exit 1
    }
    Write-Info "Detected platform: x86_64-pc-windows-msvc"
}

# --- Download latest release binary ---

function Get-LatestBinary {
    $AssetName = "$BinaryName-x86_64-pc-windows-msvc.exe"
    $ApiUrl = "https://api.github.com/repos/$Repo/releases/latest"

    Write-Info "Fetching latest release info from GitHub..."

    try {
        $Release = Invoke-RestMethod -Uri $ApiUrl -Headers @{ "User-Agent" = "ralph-wiggum-installer" }
    }
    catch {
        Write-Error2 "Failed to fetch release info: $_"
        return $false
    }

    $Asset = $Release.assets | Where-Object { $_.name -eq $AssetName } | Select-Object -First 1

    if (-not $Asset) {
        Write-Error2 "Could not find asset '$AssetName' in the latest release."
        return $false
    }

    $DownloadUrl = $Asset.browser_download_url
    Write-Info "Downloading $AssetName from:"
    Write-Info "  $DownloadUrl"

    if (-not (Test-Path $InstallDir)) {
        New-Item -ItemType Directory -Path $InstallDir -Force | Out-Null
    }

    try {
        Invoke-WebRequest -Uri $DownloadUrl -OutFile $InstallPath -UseBasicParsing
    }
    catch {
        Write-Error2 "Download failed: $_"
        return $false
    }

    Write-Success "Binary installed to $InstallPath"
    return $true
}

# --- Fallback: cargo build from source ---

function Build-FromSource {
    Write-Info "Attempting to build from source as fallback..."

    $CargoPath = Get-Command cargo -ErrorAction SilentlyContinue
    if (-not $CargoPath) {
        Write-Error2 "cargo is not installed. Install Rust from https://rustup.rs/ and try again."
        exit 1
    }

    $GitPath = Get-Command git -ErrorAction SilentlyContinue
    if (-not $GitPath) {
        Write-Error2 "git is required to build from source."
        exit 1
    }

    $TmpDir = Join-Path ([System.IO.Path]::GetTempPath()) "ralph-wiggum-build-$(Get-Random)"
    New-Item -ItemType Directory -Path $TmpDir -Force | Out-Null

    try {
        Write-Info "Cloning repository..."
        git clone --depth 1 "https://github.com/$Repo.git" $TmpDir
        if ($LASTEXITCODE -ne 0) { throw "git clone failed" }

        Write-Info "Building with cargo (release mode)..."
        cargo build --release --manifest-path (Join-Path $TmpDir "Cargo.toml")
        if ($LASTEXITCODE -ne 0) { throw "cargo build failed" }

        if (-not (Test-Path $InstallDir)) {
            New-Item -ItemType Directory -Path $InstallDir -Force | Out-Null
        }

        $BuiltBinary = Join-Path $TmpDir "target\release\$ExeName"
        Copy-Item -Path $BuiltBinary -Destination $InstallPath -Force

        Write-Success "Binary built and installed to $InstallPath"
    }
    finally {
        Remove-Item -Path $TmpDir -Recurse -Force -ErrorAction SilentlyContinue
    }
}

# --- PATH configuration ---

function Set-UserPath {
    $CurrentPath = [Environment]::GetEnvironmentVariable("Path", [EnvironmentVariableTarget]::User)

    if ($CurrentPath -split ";" | Where-Object { $_ -eq $InstallDir }) {
        Write-Success "$InstallDir is already in user PATH"
        return
    }

    # Also check if a similar entry exists (case-insensitive, trailing slash variants)
    $Normalized = $InstallDir.TrimEnd('\', '/')
    $AlreadyPresent = $CurrentPath -split ";" | Where-Object {
        $_.TrimEnd('\', '/') -ieq $Normalized
    }

    if ($AlreadyPresent) {
        Write-Success "$InstallDir is already in user PATH"
        return
    }

    Write-Info "Adding $InstallDir to user PATH..."

    $NewPath = "$CurrentPath;$InstallDir"
    [Environment]::SetEnvironmentVariable("Path", $NewPath, [EnvironmentVariableTarget]::User)

    # Also update current session
    $env:Path = "$env:Path;$InstallDir"

    Write-Success "PATH updated. Restart your terminal for changes to take full effect."
}

# --- Verification ---

function Test-Installation {
    # Ensure install dir is in current session PATH
    if (-not ($env:Path -split ";" | Where-Object { $_.TrimEnd('\', '/') -ieq $InstallDir.TrimEnd('\', '/') })) {
        $env:Path = "$env:Path;$InstallDir"
    }

    $Cmd = Get-Command $BinaryName -ErrorAction SilentlyContinue
    if ($Cmd) {
        try {
            $Version = & $BinaryName --version 2>&1
            Write-Success "Installed: $Version"
        }
        catch {
            Write-Success "Binary is installed at $InstallPath"
        }
    }
    else {
        Write-Error2 "Installation could not be verified."
        exit 1
    }
}

# --- Main ---

function Main {
    Write-Host ""
    Write-Host "  ralph-wiggum installer" -ForegroundColor White
    Write-Host "  ======================" -ForegroundColor DarkGray
    Write-Host ""

    Assert-Platform

    $Downloaded = Get-LatestBinary
    if (-not $Downloaded) {
        Write-Info "Binary download failed, falling back to source build..."
        Build-FromSource
    }

    Set-UserPath
    Test-Installation

    Write-Host ""
    Write-Success "ralph-wiggum is ready to use!"
    Write-Host ""
}

Main
