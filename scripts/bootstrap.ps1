#Requires -Version 7

[CmdletBinding()]
param()

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

$RepoRoot = Split-Path -Parent $PSScriptRoot
Push-Location $RepoRoot
try {
    $script:Failed = $false

    function Step([string]$Name) {
        Write-Host "==> $Name" -ForegroundColor Cyan
    }
    function Ok([string]$Message) {
        Write-Host "    [ok] $Message" -ForegroundColor Green
    }
    function Fail([string]$Message) {
        Write-Host "    [FAIL] $Message" -ForegroundColor Red
        $script:Failed = $true
    }

    Step 'Rust toolchain'
    $ToolchainToml = Get-Content (Join-Path $RepoRoot 'rust-toolchain.toml') -Raw
    if ($ToolchainToml -notmatch '(?m)^\s*channel\s*=\s*"([^"]+)"') {
        throw 'Could not parse channel from rust-toolchain.toml'
    }
    $Channel = $Matches[1]

    $InstalledToolchains = (rustup toolchain list) -join "`n"
    if ($InstalledToolchains -match [regex]::Escape($Channel)) {
        Ok "toolchain installed: $Channel"
    }
    else {
        Write-Host "    installing $Channel ..."
        rustup toolchain install $Channel --no-self-update
        if ($LASTEXITCODE -ne 0) { Fail "could not install $Channel"; return }
        Ok "toolchain installed: $Channel"
    }

    foreach ($Component in @('rust-src', 'llvm-tools-preview', 'rustfmt', 'clippy')) {
        # rustup may report llvm-tools-preview under its real name llvm-tools.
        $Names = @($Component)
        if ($Component -eq 'llvm-tools-preview') { $Names += 'llvm-tools' }
        $Pattern = "^($($Names -join '|'))\b.*\(installed\)"
        if ((rustup component list --toolchain $Channel | Where-Object { $_ -match $Pattern })) {
            Ok "component installed: $Component"
        }
        else {
            Write-Host "    adding component $Component ..."
            rustup component add --toolchain $Channel $Component
            if ($LASTEXITCODE -ne 0) { Fail "could not add component $Component"; continue }
            Ok "component installed: $Component"
        }
    }

    Step 'MSYS2 packages'
    $Pacman = Get-Command pacman -ErrorAction SilentlyContinue
    if (-not $Pacman) {
        Fail 'pacman not on PATH. Install MSYS2 UCRT64 and put C:\msys64\ucrt64\bin + C:\msys64\usr\bin on PATH.'
    }
    else {
        foreach ($Pkg in @('mingw-w64-ucrt-x86_64-qemu', 'mingw-w64-ucrt-x86_64-mtools')) {
            pacman -Q $Pkg 2>$null
            if ($LASTEXITCODE -eq 0) {
                Ok "$Pkg installed"
            }
            else {
                Fail "$Pkg missing. Fix: pacman -S --needed $Pkg"
            }
        }
        foreach ($Tool in @('qemu-system-x86_64', 'qemu-system-aarch64', 'mformat', 'mcopy', 'mdir')) {
            if (Get-Command $Tool -ErrorAction SilentlyContinue) {
                Ok "$Tool on PATH"
            }
            else {
                Fail "$Tool not found on PATH even though its package may be installed"
            }
        }
    }

    Step 'Limine bootloader'

    # Bumping Limine means editing these three values deliberately.
    $LimineVersion = 'v12.6.0'
    $LimineUrl = 'https://github.com/Limine-Bootloader/Limine/releases/download/v12.6.0/limine-binary.zip'
    $LimineSha256 = 'cbbc0a68da766faf05c14fdde31710563c5e6a89b6f2b012a57540d0cfdce822'

    $LimineDir = Join-Path $RepoRoot 'third_party\limine'
    # Located by name anywhere in the archive so upstream reshuffles don't break us.
    $RequiredFiles = @(
        'limine.exe',           # host-side image installer (BIOS install step)
        'limine-bios.sys',      # BIOS stage payload staged into the image
        'limine-bios-cd.bin',   # BIOS El Torito boot image
        'limine-uefi-cd.bin',   # UEFI El Torito boot image
        'BOOTX64.EFI',          # x86_64 UEFI loader
        'BOOTIA32.EFI',         # ia32 UEFI loader
        'BOOTAA64.EFI'          # aarch64 UEFI loader
    )

    $VersionMarker = Join-Path $LimineDir 'LIMINE_VERSION'
    $HaveAll = (Test-Path $VersionMarker) -and
               ((Get-Content $VersionMarker -Raw).Trim() -eq $LimineVersion) -and
               @($RequiredFiles | Where-Object { -not (Test-Path (Join-Path $LimineDir $_)) }).Count -eq 0

    if ($HaveAll) {
        Ok "limine $LimineVersion already present in third_party/limine/"
    }
    else {
        New-Item -ItemType Directory -Force -Path $LimineDir | Out-Null

        $TmpZip = Join-Path ([IO.Path]::GetTempPath()) "ferric-k-limine-$LimineVersion.zip"
        Write-Host "    downloading $LimineUrl"
        Invoke-WebRequest -Uri $LimineUrl -OutFile $TmpZip

        $ActualHash = (Get-FileHash -Path $TmpZip -Algorithm SHA256).Hash.ToLowerInvariant()
        if ($ActualHash -ne $LimineSha256.ToLowerInvariant()) {
            Remove-Item $TmpZip -Force
            throw @"
Checksum mismatch for limine-binary.zip!
  expected: $LimineSha256
  actual:   $ActualHash
Refusing to extract. If upstream re-published the asset, update the pin
deliberately and record it in ARCHITECTURE.md.
"@
        }
        Ok 'sha256 checksum matches pinned value'

        $ExtractDir = Join-Path ([IO.Path]::GetTempPath()) "ferric-k-limine-$LimineVersion-extract"
        if (Test-Path $ExtractDir) { Remove-Item $ExtractDir -Recurse -Force }
        Expand-Archive -Path $TmpZip -DestinationPath $ExtractDir
        Remove-Item $TmpZip -Force

        foreach ($File in $RequiredFiles) {
            $Matches_ = @(Get-ChildItem $ExtractDir -Recurse -Filter $File -File)
            if ($Matches_.Count -ne 1) {
                throw "Expected exactly one '$File' in the archive, found $($Matches_.Count)."
            }
            Copy-Item $Matches_[0].FullName (Join-Path $LimineDir $File) -Force
        }
        Remove-Item $ExtractDir -Recurse -Force

        Set-Content -Path $VersionMarker -Value $LimineVersion -NoNewline
        Ok "limine $LimineVersion materialized into third_party/limine/"
    }

    Write-Host ''
    if ($script:Failed) {
        Write-Host 'BOOTSTRAP FAILED — fix the items marked [FAIL] above.' -ForegroundColor Red
        exit 1
    }
    Write-Host 'Bootstrap complete: environment ready.' -ForegroundColor Green
}
finally {
    Pop-Location
}
