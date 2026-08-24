#Requires -Version 7

[CmdletBinding()]
param(
    [ValidateSet('x64')]
    [string]$Arch = 'x64',
    [string]$ImagePath,
    [ValidateRange(16, 4096)]
    [int]$SizeMb = 64
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

$RepoRoot = Split-Path -Parent $PSScriptRoot
Push-Location $RepoRoot
try {
    $SectorSize                = 512
    $Heads                     = 64      # geometry: 64 * 32 sectors == 1 MiB/cylinder
    $SectorsPerTrack           = 32
    $FirstPartStartSector      = 2048    # 1 MiB alignment; doubles as the gap
                                         # where limine bios-install puts stage 2
    $FatTypeWithLba            = 0x0E    # FAT16 primary partition, LBA-mapped

    function Step([string]$Name) { Write-Host "==> $Name" -ForegroundColor Cyan }
    function Ok([string]$Message) { Write-Host "    [ok] $Message" -ForegroundColor Green }

    function Assert-Tool([string]$Name) {
        if (-not (Get-Command $Name -ErrorAction SilentlyContinue)) {
            throw "'$Name' not found on PATH. Run: pwsh scripts/bootstrap.ps1"
        }
    }

    function Invoke-Checked([scriptblock]$Cmd, [string]$What) {
        & $Cmd
        if ($LASTEXITCODE -ne 0) { throw "$What failed (exit $LASTEXITCODE)" }
        Ok $What
    }

    # MBR CHS encoding: head byte, sector low 6 bits + cylinder bits 8-9 in
    # the high 2, cylinder bits 0-7; out-of-range clamps to 63/255/1023.
    function ConvertTo-ChsBytes([uint32]$Lba) {
        $spt   = $SectorsPerTrack
        $head  = [Math]::Min(255, [int](($Lba / $spt) % $Heads))
        $cyl   = [Math]::Min(1023, [int]($Lba / ($spt * $Heads)))
        $sect  = [Math]::Min(63, [int]($Lba % $spt) + 1)
        return @([byte]$head,
                 [byte](($sect -band 0x3F) -bor ((($cyl -shr 8) -band 0x3) -shl 6)),
                 [byte]($cyl -band 0xFF))
    }

    Step 'Inputs'
    foreach ($Tool in @('mformat', 'mmd', 'mcopy', 'mdir')) { Assert-Tool $Tool }

    $LimineExe = Join-Path $RepoRoot 'third_party\limine\limine.exe'
    if (-not (Test-Path $LimineExe)) {
        throw "third_party/limine missing. Run: pwsh scripts/bootstrap.ps1"
    }

    if ($Arch -ne 'x64') { throw "Arch '$Arch' arrives later." }

    $KernelElf = Join-Path $RepoRoot 'target\x86_64-ferric\debug\ferric-kernel'
    if (-not (Test-Path $KernelElf)) {
        throw ("Kernel ELF not found at $KernelElf.`n" +
               'Build it first: cargo build --target targets/x86_64-ferric.json ' +
               '-Zbuild-std=core,compiler_builtins -Zjson-target-spec')
    }
    $KernelSize = (Get-Item $KernelElf).Length

    $ConfPath = Join-Path $RepoRoot 'boot\limine.conf'
    $BiosSys  = Join-Path $RepoRoot 'third_party\limine\limine-bios.sys'
    foreach ($File in @($ConfPath, $BiosSys,
                        (Join-Path $RepoRoot 'third_party\limine\BOOTX64.EFI'),
                        (Join-Path $RepoRoot 'third_party\limine\BOOTIA32.EFI'))) {
        if (-not (Test-Path $File)) { throw "missing input: $File" }
    }
    Ok "kernel ELF ($KernelSize bytes), config, Limine payloads"

    # ------------------------------------------------------------------
    # Image + partition table
    # ------------------------------------------------------------------
    Step 'Create image'
    if (-not $ImagePath) { $ImagePath = Join-Path $RepoRoot 'build\ferric.img' }
    $ImageDir = Split-Path -Parent $ImagePath
    if (-not (Test-Path $ImageDir)) { New-Item -ItemType Directory -Force -Path $ImageDir | Out-Null }

    $TotalSectors     = [uint32]($SizeMb * 1MB / $SectorSize)
    $PartStartSector  = [uint32]$FirstPartStartSector
    $PartSectors      = [uint32]($TotalSectors - $FirstPartStartSector)

    # Preallocated, not sparse: mtools/QEMU treat holes inconsistently.
    $Stream = [IO.File]::Create($ImagePath)
    try { $Stream.SetLength([int64]$TotalSectors * $SectorSize) } finally { $Stream.Dispose() }

    $Mbr = New-Object byte[] $SectorSize
    $Mbr[446] = 0x80                                  # entry 1: bootable flag
    $ChsStart = ConvertTo-ChsBytes $PartStartSector   # cosmetic (LBA fields rule)
    $ChsEnd   = ConvertTo-ChsBytes ($PartStartSector + $PartSectors - 1)
    $Mbr[447] = $ChsStart[0]; $Mbr[448] = $ChsStart[1]; $Mbr[449] = $ChsStart[2]
    $Mbr[450] = $FatTypeWithLba
    $Mbr[451] = $ChsEnd[0];   $Mbr[452] = $ChsEnd[1];   $Mbr[453] = $ChsEnd[2]
    ([BitConverter]::GetBytes($PartStartSector)).CopyTo($Mbr, 454)  # start LBA
    ([BitConverter]::GetBytes($PartSectors)).CopyTo($Mbr, 458)      # count
    $Mbr[510] = 0x55; $Mbr[511] = 0xAA                # boot signature

    $Stream = [IO.File]::Open($ImagePath, 'Open', 'ReadWrite')
    try { $Stream.Write($Mbr, 0, $SectorSize) } finally { $Stream.Dispose() }
    Ok ("{0}: {1} MiB, partition 1 type 0x{2:X} @ LBA {3} ({4} MiB)" -f `
            (Split-Path -Leaf $ImagePath), $SizeMb, $FatTypeWithLba,
            $PartStartSector, ($PartSectors * $SectorSize / 1MB))

    Step 'Format FAT'
    $Off = [int64]$PartStartSector * $SectorSize
    $ImgAtOff = "{0}@@{1}" -f $ImagePath, $Off
    Invoke-Checked `
        { mformat -i $ImgAtOff -t ($PartSectors / ($Heads * $SectorsPerTrack)) `
                  -h $Heads -s $SectorsPerTrack :: } `
        'mformat FAT16'

    Step 'Stage files'
    Invoke-Checked { mmd -i $ImgAtOff ::EFI ::EFI/BOOT } 'mmd EFI/BOOT'
    Invoke-Checked { mcopy -i $ImgAtOff $ConfPath '::limine.conf' } 'copy limine.conf'
    Invoke-Checked { mcopy -i $ImgAtOff $KernelElf '::kernel.elf' } 'copy kernel.elf'
    Invoke-Checked { mcopy -i $ImgAtOff $BiosSys '::limine-bios.sys' } 'copy limine-bios.sys'
    Invoke-Checked {
        mcopy -i $ImgAtOff `
              (Join-Path $RepoRoot 'third_party\limine\BOOTX64.EFI') `
              (Join-Path $RepoRoot 'third_party\limine\BOOTIA32.EFI') `
              '::EFI/BOOT/'
    } 'copy UEFI loaders'

    Step 'Install Limine BIOS stages'
    # Capture the installer's banner so it cannot interleave with step markers.
    $InstallLog = & $LimineExe bios-install $ImagePath 2>&1
    if ($LASTEXITCODE -ne 0) { throw "limine bios-install failed (exit $LASTEXITCODE)" }
    Ok 'limine bios-install'
    Write-Host (($InstallLog | ForEach-Object { "        $_" }) -join "`n") `
        -ForegroundColor DarkGray

    Step 'Validate image'
    $Listing = @(mdir -i $ImgAtOff ::) + @(mdir -i $ImgAtOff '::EFI/BOOT')
    if ($LASTEXITCODE -ne 0) { throw 'mdir validation failed' }
    $Flat = ($Listing -join "`n").ToLowerInvariant()
    foreach ($Needle in @('limine.conf', 'kernel', 'limine-bios.sys',
                          'bootx64', 'bootia32')) {
        if ($Flat -notmatch [regex]::Escape($Needle)) {
            throw "image validation: '$Needle' not found in FAT directory listing"
        }
    }
    Ok 'FAT contents complete'

    $BootSector = New-Object byte[] 512
    $Stream = [IO.File]::OpenRead($ImagePath)
    try { [void]$Stream.Read($BootSector, 0, 512) } finally { $Stream.Dispose() }
    if (($BootSector[0..7] | Where-Object { $_ -ne 0 }).Count -eq 0) {
        throw 'image validation: MBR boot code area is still empty (bios-install no-op?)'
    }
    Ok 'MBR contains installed Limine stages'

    Write-Host ''
    Write-Host "Image ready: $ImagePath" -ForegroundColor Green
}
finally {
    Pop-Location
}
