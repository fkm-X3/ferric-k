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

    $QemuName = if ($Arch -eq 'x64') { 'qemu-system-x86_64' } else { 'qemu-system-aarch64' }
    $Qemu = Get-Command $QemuName -ErrorAction SilentlyContinue
    if (-not $Qemu) {
        throw "$QemuName not found on PATH. Run: pwsh scripts/bootstrap.ps1"
    }

    if (-not $ImagePath) { $ImagePath = Join-Path $RepoRoot 'build\ferric.img' }
    if (-not (Test-Path $ImagePath)) {
        Write-Host '==> image missing, building it' -ForegroundColor Cyan
        & (Join-Path $PSScriptRoot 'build-image.ps1')
        if ($LASTEXITCODE -ne 0) { throw 'image build failed' }
    }

    # Mirrors the banners in ferric_unsafe_core::boot; keep in sync.
    $BootMarker = 'BOOT OK'
    $FramebufferMarker = 'FRAMEBUFFER OK'

    if ($Arch -eq 'x64') {
        # Mirrors crates/ferric-unsafe-core/src/qemu.rs; keep in sync.
        $ExitChannelArgs = @(
            '-device', 'isa-debug-exit,iobase=0x501,iosize=0x2'
        )
        $ExpectedExitCode = (0x10 -shl 1) -bor 1   # STATUS_BOOT_OK -> 33

        $MachineArgs = @('-M', 'q35', '-m', '2G',
                         '-hda', $ImagePath, '-serial', 'stdio')
    }
    else {
        $Firmware = Join-Path $RepoRoot 'third_party\firmware\edk2-aarch64-code.fd'
        if (-not (Test-Path $Firmware)) {
            throw "aarch64 UEFI firmware missing at $Firmware. Run: pwsh scripts/bootstrap.ps1"
        }

        # Semihosting provides the deterministic exit on aarch64 (no
        # isa-debug-exit there); SYS_EXIT passes the status byte through raw.
        # Mirrors qemu.rs; keep in sync.
        $ExitChannelArgs = @('-semihosting-config', 'enable=on,target=native')
        $ExpectedExitCode = 0x10                    # STATUS_BOOT_OK -> 16

        # -M virt ships no display device by default; ramfb gives the UEFI
        # firmware a GOP surface so Limine can hand over a linear framebuffer.
        $MachineArgs = @('-M', 'virt', '-cpu', 'cortex-a72', '-m', '2G',
                         '-bios', $Firmware,
                         '-drive', "if=virtio,format=raw,file=$ImagePath",
                         '-device', 'ramfb',
                         '-serial', 'stdio')
    }

    if (-not $Smoke) {
        Write-Host '==> booting QEMU (interactive; close window or Ctrl-C to stop)' `
            -ForegroundColor Cyan
        & $Qemu.Source @MachineArgs @ExitChannelArgs
        Write-Host ("QEMU exited with code {0}" -f $LASTEXITCODE)
        return
    }

    Write-Host '==> smoke boot (headless)' -ForegroundColor Cyan
    $LogDir = Join-Path $RepoRoot 'build'
    if (-not (Test-Path $LogDir)) {
        New-Item -ItemType Directory -Force -Path $LogDir | Out-Null
    }
    $StdoutLog = Join-Path $LogDir "last-smoke-$Arch-stdout.log"
    $StderrLog = Join-Path $LogDir "last-smoke-$Arch-stderr.log"

    # Headless + no-reboot: a guest reset (e.g. triple fault) exits instead of
    # looping forever, and its exit code can never match the assertion.
    $Proc = Start-Process -FilePath $Qemu.Source `
        -ArgumentList ($MachineArgs + $ExitChannelArgs +
                       @('-display', 'none', '-no-reboot')) `
        -NoNewWindow -PassThru `
        -RedirectStandardOutput $StdoutLog -RedirectStandardError $StderrLog

    if (-not $Proc.WaitForExit($SmokeTimeoutSec * 1000)) {
        Start-Process -FilePath taskkill.exe `
            -ArgumentList "/PID $($Proc.Id) /T /F" -Wait -WindowStyle Hidden
        Fail "kernel produced no serial banner + exit within ${SmokeTimeoutSec}s (killed QEMU)"
    }

    $SerialTail = if ((Test-Path $StdoutLog) -and (Get-Item $StdoutLog).Length -gt 0) {
        (Get-Content $StdoutLog | Select-Object -Last 10) -join "`n"
    } else { '<serial empty>' }

    if (-not (Test-Path $StdoutLog) -or
        -not (Select-String -Path $StdoutLog -SimpleMatch $BootMarker -Quiet)) {
        Fail ("serial log lacks '{0}' marker. Serial tail:`n{1}" -f $BootMarker, $SerialTail)
    }

    if (-not (Select-String -Path $StdoutLog -SimpleMatch $FramebufferMarker -Quiet)) {
        Fail ("serial log lacks '{0}' marker. Serial tail:`n{1}" -f `
                $FramebufferMarker, $SerialTail)
    }

    if ($Proc.ExitCode -ne $ExpectedExitCode) {
        Fail ("QEMU exit code {0}, expected {1}. Serial tail:`n{2}" -f `
                $Proc.ExitCode, $ExpectedExitCode, $SerialTail)
    }

    Write-Host ("    [ok] serial banners '{0}' + '{1}' asserted + clean exit code {2}" -f `
            $BootMarker, $FramebufferMarker, $ExpectedExitCode) -ForegroundColor Green
    Write-Host 'SMOKE PASSED' -ForegroundColor Green
    exit 0
}
finally {
    Pop-Location
}
