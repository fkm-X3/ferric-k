#Requires -Version 7
<#
.SYNOPSIS
    Ferric-K quality gate: must pass before every commit.
.DESCRIPTION
    fmt -> clippy (both custom targets, -D warnings incl. undocumented-unsafe
    audit) -> build both targets -> ELF sanity (magic, machine, live entry).
#>
[CmdletBinding()]
param()

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

$RepoRoot = Split-Path -Parent $PSScriptRoot
Push-Location $RepoRoot
try {
    $script:StepNumber = 0

    function Step([string]$Name) {
        $script:StepNumber++
        Write-Host ("==> [{0}/5] {1}" -f $script:StepNumber, $Name) -ForegroundColor Cyan
    }

    function Assert-ExitCode([int]$Code, [string]$What) {
        if ($Code -ne 0) {
            Write-Host "CHECK FAILED during: $What" -ForegroundColor Red
            exit 1
        }
        Write-Host "    [ok] $What" -ForegroundColor Green
    }

    # ------------------------------------------------------------------
    # 1. Formatting
    # ------------------------------------------------------------------
    Step 'cargo fmt --check'
    cargo fmt --all --check
    Assert-ExitCode $LASTEXITCODE 'formatting'

    # ------------------------------------------------------------------
    # 2-3. Clippy against both kernel targets.
    # ------------------------------------------------------------------
    foreach ($Target in @('x86_64-ferric', 'aarch64-ferric')) {
        Step "clippy --target $Target"
        cargo clippy --workspace --target "targets/$Target.json" -- -D warnings
        Assert-ExitCode $LASTEXITCODE "clippy ($Target)"
    }

    # ------------------------------------------------------------------
    # 4-5. Build both targets and sanity-check the produced kernel ELF:
    #      magic \x7fELF, expected e_machine, non-zero entry (_start linked).
    # ------------------------------------------------------------------
    $Expectations = @{
        'x86_64-ferric'  = @{ Machine = 0x3E; Name = 'EM_X86_64' }
        'aarch64-ferric' = @{ Machine = 0xB7; Name = 'EM_AARCH64' }
    }

    foreach ($Target in @('x86_64-ferric', 'aarch64-ferric')) {
        Step "build --target $Target"
        cargo build --target "targets/$Target.json"
        Assert-ExitCode $LASTEXITCODE "build ($Target)"

        $Elf = Join-Path $RepoRoot "target/$Target/debug/ferric-kernel"
        if (-not (Test-Path $Elf)) {
            Write-Host "CHECK FAILED: kernel ELF not found at $Elf" -ForegroundColor Red
            exit 1
        }
        $Bytes = [IO.File]::ReadAllBytes($Elf)
        if ($Bytes.Length -lt 64 -or $Bytes[0] -ne 0x7F -or $Bytes[1] -ne 0x45 -or
            $Bytes[2] -ne 0x4C -or $Bytes[3] -ne 0x46) {
            Write-Host "CHECK FAILED: $Elf is not an ELF file" -ForegroundColor Red
            exit 1
        }
        $Machine = [BitConverter]::ToUInt16($Bytes, 0x12)
        $Entry = [BitConverter]::ToUInt64($Bytes, 0x18)
        $WantMachine = $Expectations[$Target].Machine
        if ($Machine -ne $WantMachine) {
            Write-Host ("CHECK FAILED: {0}: e_machine 0x{1:X4}, expected {2} (0x{3:X4})" -f `
                    $Elf, $Machine, $Expectations[$Target].Name, $WantMachine) -ForegroundColor Red
            exit 1
        }
        if ($Entry -eq 0) {
            Write-Host "CHECK FAILED: ${Elf}: entry point is zero (_start not linked)" -ForegroundColor Red
            exit 1
        }
        Write-Host ("    [ok] ELF valid: machine={0}, entry=0x{1:X}" -f $Expectations[$Target].Name, $Entry) `
            -ForegroundColor Green
    }

    Write-Host ''
    Write-Host 'CHECK PASSED: fmt + clippy(x2) + build(x2) + ELF sanity all green.' -ForegroundColor Green
}
finally {
    Pop-Location
}
