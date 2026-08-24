#Requires -Version 7

[CmdletBinding()]
param(
    [ValidateSet('x64', 'arm64')]
    [string]$Arch = 'x64',
    [switch]$Smoke,
    [string]$ImagePath,
    [ValidateRange(10, 600)]
    [int]$SmokeTimeoutSec = 120
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

$RepoRoot = Split-Path -Parent $PSScriptRoot
Push-Location $RepoRoot
try {
    function Fail([string]$Message) {
        Write-Host "SMOKE FAILED: $Message" -ForegroundColor Red
        exit 1
    }

    if ($Arch -ne 'x64') { throw "Arch '$Arch' arrives later." }
    $Qemu = Get-Command qemu-system-x86_64 -ErrorAction SilentlyContinue
    if (-not $Qemu) {
        throw 'qemu-system-x86_64 not found on PATH. Run: pwsh scripts/bootstrap.ps1'
    }

    if (-not $ImagePath) { $ImagePath = Join-Path $RepoRoot 'build\ferric.img' }
    if (-not (Test-Path $ImagePath)) {
        Write-Host '==> image missing, building it' -ForegroundColor Cyan
        & (Join-Path $PSScriptRoot 'build-image.ps1')
        if ($LASTEXITCODE -ne 0) { throw 'image build failed' }
    }

    # Mirrors crates/ferric-unsafe-core/src/qemu.rs; keep in sync.
    $DebugExitDeviceArgs = @(
        '-device', 'isa-debug-exit,iobase=0x501,iosize=0x2'
    )
    $ExpectedExitCode = (0x10 -shl 1) -bor 1   # = 33

    $BaseArgs = @('-M', 'q35', '-m', '2G', '-hda', $ImagePath, '-serial', 'stdio')

    if (-not $Smoke) {
        Write-Host '==> booting QEMU (interactive; close window or Ctrl-C to stop)' `
            -ForegroundColor Cyan
        & $Qemu.Source @BaseArgs @DebugExitDeviceArgs
        Write-Host ("QEMU exited with code {0}" -f $LASTEXITCODE)
        return
    }

    Write-Host '==> smoke boot (headless)' -ForegroundColor Cyan
    $LogDir = Join-Path $RepoRoot 'build'
    if (-not (Test-Path $LogDir)) {
        New-Item -ItemType Directory -Force -Path $LogDir | Out-Null
    }
    $StdoutLog = Join-Path $LogDir 'last-smoke-stdout.log'
    $StderrLog = Join-Path $LogDir 'last-smoke-stderr.log'

    # Headless + no-reboot: a guest reset (e.g. triple fault) exits instead of
    # looping forever, and its exit code can never match the assertion.
    $Proc = Start-Process -FilePath $Qemu.Source `
        -ArgumentList ($BaseArgs + $DebugExitDeviceArgs +
                       @('-display', 'none', '-no-reboot')) `
        -NoNewWindow -PassThru `
        -RedirectStandardOutput $StdoutLog -RedirectStandardError $StderrLog

    if (-not $Proc.WaitForExit($SmokeTimeoutSec * 1000)) {
        Start-Process -FilePath taskkill.exe `
            -ArgumentList "/PID $($Proc.Id) /T /F" -Wait -WindowStyle Hidden
        Fail "no isa-debug-exit signal within ${SmokeTimeoutSec}s (killed QEMU)"
    }

    $SerialTail = if ((Test-Path $StdoutLog) -and (Get-Item $StdoutLog).Length -gt 0) {
        (Get-Content $StdoutLog | Select-Object -Last 10) -join "`n"
    } else { '<serial empty>' }

    if ($Proc.ExitCode -ne $ExpectedExitCode) {
        Fail ("QEMU exit code {0}, expected {1}. Serial tail:`n{2}" -f `
                $Proc.ExitCode, $ExpectedExitCode, $SerialTail)
    }

    Write-Host ("    [ok] kernel booted through Limine into Rust " +
                "(isa-debug-exit code {0})" -f $ExpectedExitCode) -ForegroundColor Green
    Write-Host 'SMOKE PASSED' -ForegroundColor Green
    exit 0
}
finally {
    Pop-Location
}
