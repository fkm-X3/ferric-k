#Requires -Version 7

[CmdletBinding()]
param()

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

$RepoRoot = Split-Path -Parent $PSScriptRoot
Push-Location $RepoRoot
try {
    # Flags every kernel-target cargo invocation needs now that the global
    # `[unstable] build-std` was removed from .cargo/config.toml (it poisoned
    # host builds by rebuilding `core`; see ARCHITECTURE.md D-12).
    $KernelCargoArgs = @('-Zbuild-std=core,compiler_builtins', '-Zjson-target-spec')

    $script:StepNumber = 0

    function Step([string]$Name) {
        $script:StepNumber++
        Write-Host ("==> [{0}/8] {1}" -f $script:StepNumber, $Name) -ForegroundColor Cyan
    }

    function Assert-ExitCode([int]$Code, [string]$What) {
        if ($Code -ne 0) {
            Write-Host "CHECK FAILED during: $What" -ForegroundColor Red
            exit 1
        }
        Write-Host "    [ok] $What" -ForegroundColor Green
    }

    function Fail([string]$Message) {
        Write-Host "CHECK FAILED: $Message" -ForegroundColor Red
        exit 1
    }

    function Read-U16([byte[]]$B, [int]$Off) { [BitConverter]::ToUInt16($B, $Off) }
    function Read-U32([byte[]]$B, [int]$Off) { [BitConverter]::ToUInt32($B, $Off) }
    function Read-U64([byte[]]$B, [int]$Off) { [BitConverter]::ToUInt64($B, $Off) }

    function Get-LeHex([ulong]$Value) {
        -join ([BitConverter]::GetBytes($Value) | ForEach-Object { $_.ToString('x2') })
    }

    function Get-CString([byte[]]$Blob, [int]$Start) {
        $end = $Start
        while ($end -lt $Blob.Length -and $Blob[$end] -ne 0) { $end++ }
        [Text.Encoding]::ASCII.GetString($Blob, $Start, $end - $Start)
    }

    function Find-Bytes([byte[]]$Haystack, [byte[]]$Needle) {
        for ($i = 0; $i -le $Haystack.Length - $Needle.Length; $i++) {
            $matched = $true
            for ($j = 0; $j -lt $Needle.Length; $j++) {
                if ($Haystack[$i + $j] -ne $Needle[$j]) { $matched = $false; break }
            }
            if ($matched) { return $i }
        }
        return -1
    }

    # ------------------------------------------------------------------
    # Permanent Limine Boot Protocol structural gate (x86_64 image).
    # crates/ferric-unsafe-core/src/limine.rs (Limine PROTOCOL.md).
    # ------------------------------------------------------------------
    function Invoke-LimineElfGate([string]$ElfPath) {
        $StartMarker = @(
            [Convert]::ToUInt64('F6B8F4B39DE7D1AE', 16),
            [Convert]::ToUInt64('FAB91A6940FCB9CF', 16),
            [Convert]::ToUInt64('785C6ED015D3E316', 16),
            [Convert]::ToUInt64('181E920A7852B9D9', 16)
        )
        $EndMarker = @(
            [Convert]::ToUInt64('ADC0E0531BB10D03', 16),
            [Convert]::ToUInt64('9572709F31764C62', 16)
        )
        # Magic/ID words every image must contain somewhere in the section
        # (LE byte order). Ordered so failures report deterministically.
        $RequiredWords = [ordered]@{
            'base-revision-magic' = 'F9562B2D5C95A6C8'
            'hhdm-request-id'     = '48DCF1CB8AD2B852'
            'framebuffer-id'      = '9D5827DCD881DD75'
            'memmap-request-id'   = '67CF3D9D378A806F'
        }
        # Higher-half virtual base and a 2 GiB sanity ceiling for e_entry.
        $HigherHalfBase = [Convert]::ToUInt64('FFFFFFFF80000000', 16)
        $EntryCeiling   = [Convert]::ToUInt64('FFFFFFFFC0000000', 16)

        $Bytes = [IO.File]::ReadAllBytes($ElfPath)

        # --- Entry point lives in the canonical higher-half window -------
        $Entry = Read-U64 $Bytes 0x18
        if ($Entry -lt $HigherHalfBase -or $Entry -ge $EntryCeiling) {
            Fail ("{0}: e_entry 0x{1:X} outside higher-half range " +
                  "[0x{2:X}, 0x{3:X})" -f $ElfPath, $Entry, $HigherHalfBase, $EntryCeiling)
        }
        Write-Host ("    [ok] entry 0x{0:X} in higher-half window" -f $Entry) -ForegroundColor Green

        # --- Program headers: first PT_LOAD at the base, all 4K aligned --
        $PhOff      = [int](Read-U64 $Bytes 0x20)
        $PhEntSize  = Read-U16 $Bytes 0x36
        $PhNum      = Read-U16 $Bytes 0x38
        if ($PhNum -eq 0) { Fail "${ElfPath}: no program headers" }
        $SeenLoad = 0
        for ($i = 0; $i -lt $PhNum; $i++) {
            $Base = $PhOff + $i * $PhEntSize
            $Type  = Read-U32 $Bytes $Base
            if ($Type -ne 1) { continue }   # PT_LOAD
            $SeenLoad++
            $Vaddr = Read-U64 $Bytes ($Base + 0x10)
            $Align = Read-U64 $Bytes ($Base + 0x30)
            if ($Align -ne 0x1000) {
                Fail ("{0}: PT_LOAD #{1} p_align 0x{2:X}, expected 0x1000 " +
                      "(loader requires <=4 KiB pages)" -f $ElfPath, $i, $Align)
            }
            if ($SeenLoad -eq 1 -and $Vaddr -ne $HigherHalfBase) {
                Fail ("{0}: first PT_LOAD vaddr 0x{1:X}, expected 0x{2:X} " +
                      "(higher-half base)" -f $ElfPath, $Vaddr, $HigherHalfBase)
            }
        }
        if ($SeenLoad -eq 0) { Fail "${ElfPath}: no PT_LOAD segments" }
        Write-Host ("    [ok] {0} PT_LOAD segments, first at base, p_align=4KiB" -f $SeenLoad) `
            -ForegroundColor Green

        # --- Locate .limine_requests via the section header string table -
        $ShOff     = [int](Read-U64 $Bytes 0x28)
        $ShEntSize = Read-U16 $Bytes 0x3A
        $ShNum     = Read-U16 $Bytes 0x3C
        $ShStrNdx  = Read-U16 $Bytes 0x3E
        if ($ShNum -eq 0 -or $ShStrNdx -eq 0) {
            Fail "${ElfPath}: stripped section headers cannot prove Limine requests"
        }
        $StrTabHdr = $ShOff + $ShStrNdx * $ShEntSize
        $StrTabOff = [int](Read-U64 $Bytes ($StrTabHdr + 0x18))
        $StrTabLen = [int](Read-U64 $Bytes ($StrTabHdr + 0x20))
        $StrTab = $Bytes[$StrTabOff..($StrTabOff + $StrTabLen - 1)]

        for ($i = 0; $i -lt $ShNum; $i++) {
            $Hdr = $ShOff + $i * $ShEntSize
            $Name = Get-CString $StrTab ([int](Read-U32 $Bytes $Hdr))
            if ($Name -ne '.limine_requests') { continue }

            $Addr = Read-U64 $Bytes ($Hdr + 0x10)
            $Off  = [int](Read-U64 $Bytes ($Hdr + 0x18))
            $Len  = [int](Read-U64 $Bytes ($Hdr + 0x20))

            if (($Addr % 8) -ne 0) {
                Fail ("{0}: .limine_requests vaddr 0x{1:X} not 8-byte aligned" -f $ElfPath, $Addr)
            }
            if ($Len -lt 216) {
                Fail ("{0}: .limine_requests size {1} < 216 (markers + base rev " +
                      "+ 3 requests)" -f $ElfPath, $Len)
            }
            $Content = $Bytes[$Off..($Off + $Len - 1)]

            # Start marker must be the first 32 bytes...
            for ($w = 0; $w -lt 4; $w++) {
                $Want = Get-LeHex $StartMarker[$w]
                $Got  = Get-LeHex (Read-U64 $Content (8 * $w))
                if ($Got -ne $Want) {
                    Fail ("{0}: start marker word {1}: found 0x{2}, expected 0x{3}" -f `
                            $ElfPath, $w, $Got, $Want)
                }
            }
            # ...and the end marker the final 16 bytes.
            for ($w = 0; $w -lt 2; $w++) {
                $Want = Get-LeHex $EndMarker[$w]
                $Got  = Get-LeHex (Read-U64 $Content ($Len - 16 + 8 * $w))
                if ($Got -ne $Want) {
                    Fail ("{0}: end marker word {1}: found 0x{2}, expected 0x{3}" -f `
                            $ElfPath, $w, $Got, $Want)
                }
            }
            # Base revision magic + one ID word per requested feature.
            foreach ($Label in $RequiredWords.Keys) {
                $Hex = $RequiredWords[$Label]
                # Walk the hex string back-to-front so pairs land LE:
                # "48DCF1CB8AD2B852" -> bytes 52 B8 D2 8A CB F1 DC 48.
                $Pairs = for ($k = 14; $k -ge 0; $k -= 2) { $Hex.Substring($k, 2) }
                $Needle = [byte[]]($Pairs | ForEach-Object { [Convert]::ToByte($_, 16) })
                if ((Find-Bytes $Content $Needle) -lt 0) {
                    Fail ("{0}: .limine_requests missing {1} (0x{2})" -f `
                            $ElfPath, $Label, $Hex)
                }
            }

            Write-Host ("    [ok] .limine_requests: markers + base-rev + hhdm/" +
                        "framebuffer/memmap IDs, addr 0x{0:X} len {1}" -f $Addr, $Len) `
                -ForegroundColor Green
            return
        }
        Fail "${ElfPath}: .limine_requests section not found"
    }

    # ------------------------------------------------------------------
    # 1. Formatting
    # ------------------------------------------------------------------
    Step 'cargo fmt --check'
    cargo fmt --all --check
    Assert-ExitCode $LASTEXITCODE 'formatting'

    # ------------------------------------------------------------------
    # 2. Host-side clippy: libs and test code (the freestanding bin itself
    #    can never be checked for the host; it is covered by steps 3-4).
    # ------------------------------------------------------------------
    Step 'clippy (host: libs + tests)'
    cargo clippy --workspace --lib --tests -- -D warnings
    Assert-ExitCode $LASTEXITCODE 'clippy (host)'

    # ------------------------------------------------------------------
    # 3-4. Clippy against both kernel targets.
    # ------------------------------------------------------------------
    foreach ($Target in @('x86_64-ferric', 'aarch64-ferric')) {
        Step "clippy --target $Target"
        cargo clippy --workspace --target "targets/$Target.json" @KernelCargoArgs -- -D warnings
        Assert-ExitCode $LASTEXITCODE "clippy ($Target)"
    }

    # ------------------------------------------------------------------
    # 5-6. Build both targets; sanity-check each produced kernel ELF;
    #      run the full Limine structural gate on the x86_64 image.
    # ------------------------------------------------------------------
    $Expectations = @{
        'x86_64-ferric'  = @{ Machine = 0x3E; Name = 'EM_X86_64'; Limine = $true }
        'aarch64-ferric' = @{ Machine = 0xB7; Name = 'EM_AARCH64'; Limine = $false }
    }

    foreach ($Target in @('x86_64-ferric', 'aarch64-ferric')) {
        Step "build --target $Target (+ELF checks)"
        cargo build --target "targets/$Target.json" @KernelCargoArgs
        Assert-ExitCode $LASTEXITCODE "build ($Target)"

        $Elf = Join-Path $RepoRoot "target/$Target/debug/ferric-kernel"
        if (-not (Test-Path $Elf)) {
            Fail "kernel ELF not found at $Elf"
        }
        $Bytes = [IO.File]::ReadAllBytes($Elf)
        if ($Bytes.Length -lt 64 -or $Bytes[0] -ne 0x7F -or $Bytes[1] -ne 0x45 -or
            $Bytes[2] -ne 0x4C -or $Bytes[3] -ne 0x46) {
            Fail "$Elf is not an ELF file"
        }
        $Machine = Read-U16 $Bytes 0x12
        $Entry = Read-U64 $Bytes 0x18
        $WantMachine = $Expectations[$Target].Machine
        if ($Machine -ne $WantMachine) {
            Fail ("{0}: e_machine 0x{1:X4}, expected {2} (0x{3:X4})" -f `
                    $Elf, $Machine, $Expectations[$Target].Name, $WantMachine)
        }
        if ($Entry -eq 0) {
            Fail "${Elf}: entry point is zero (_start not linked)"
        }
        Write-Host ("    [ok] ELF valid: machine={0}, entry=0x{1:X}" -f $Expectations[$Target].Name, $Entry) `
            -ForegroundColor Green

        if ($Expectations[$Target].Limine) {
            Invoke-LimineElfGate $Elf
        }
    }

    # ------------------------------------------------------------------
    # 7. Host unit tests (Limine ABI layout contracts et al.)
    # ------------------------------------------------------------------
    Step 'test (host: ferric-unsafe-core)'
    cargo test -p ferric-unsafe-core --lib
    Assert-ExitCode $LASTEXITCODE 'host tests'

    # ------------------------------------------------------------------
    # 8. Boot proof: disk image + headless QEMU boot through Limine.
    #    The kernel signals completed early init via isa-debug-exit;
    #    run.ps1 asserts the resulting exit code (see qemu.rs).
    #    Child scripts exit non-zero on failure, which terminates this
    #    gate with their diagnostics on record.
    # ------------------------------------------------------------------
    Step 'disk image + QEMU smoke boot'
    & (Join-Path $PSScriptRoot 'build-image.ps1')
    if ($LASTEXITCODE -ne 0) { Fail 'image build' }
    & (Join-Path $PSScriptRoot 'run.ps1') -Arch x64 -Smoke
    Assert-ExitCode $LASTEXITCODE 'smoke boot'

    Write-Host ''
    Write-Host ('CHECK PASSED: fmt + clippy(host,x86_64,aarch64) + build(x2) + ' +
                'ELF/Limine gates + host tests + smoke boot all green.') -ForegroundColor Green
}
finally {
    Pop-Location
}
