param([string]$GameDir, [switch]$NonInteractive, [switch]$CheckOnly, [switch]$Offline)
$ErrorActionPreference = 'Stop'
$projectRoot = Split-Path -Parent $PSScriptRoot
function Resolve-Game([string]$candidate) {
    if ([string]::IsNullOrWhiteSpace($candidate)) { return $null }
    $candidate = $candidate.Trim().Trim('"')
    if (-not (Test-Path -LiteralPath $candidate -PathType Container)) { return $null }
    $resolved = (Resolve-Path -LiteralPath $candidate).Path
    if ((Split-Path -Leaf $resolved) -eq 'left4dead2') { $resolved = Split-Path -Parent $resolved }
    foreach ($file in @('left4dead2.exe','left4dead2/pak01_dir.vpk','left4dead2/maps/c1m1_hotel.bsp')) {
        if (-not (Test-Path -LiteralPath (Join-Path $resolved $file) -PathType Leaf)) { return $null }
    }
    return $resolved
}
try {
    $installation = Resolve-Game $GameDir
    if ($GameDir -and -not $installation) { throw 'That folder does not contain a complete L4D2 installation.' }
    if (-not $installation) {
        $candidates = @()
        $saved = Join-Path $projectRoot 'runtime/game-dir.txt'
        if (Test-Path -LiteralPath $saved) { $candidates += Get-Content -LiteralPath $saved -Raw -Encoding UTF8 }
        $steamRoots = @()
        $steam = Get-ItemProperty 'HKCU:\Software\Valve\Steam' -ErrorAction SilentlyContinue
        if ($steam -and $steam.SteamPath) { $steamRoots += $steam.SteamPath }
        foreach ($base in @(${env:ProgramFiles(x86)}, $env:ProgramFiles)) {
            if ($base) { $steamRoots += Join-Path $base 'Steam' }
        }
        foreach ($steamRoot in ($steamRoots | Select-Object -Unique)) {
            $candidates += Join-Path $steamRoot 'steamapps/common/Left 4 Dead 2'
            $libraries = Join-Path $steamRoot 'steamapps/libraryfolders.vdf'
            if (Test-Path -LiteralPath $libraries) {
                $content = Get-Content -LiteralPath $libraries -Raw
                foreach ($match in [regex]::Matches($content, '"path"\s+"([^"\r\n]+)"')) {
                    $libraryRoot = $match.Groups[1].Value.Replace('\\','\')
                    $candidates += Join-Path $libraryRoot 'steamapps/common/Left 4 Dead 2'
                }
            }
        }
        foreach ($candidate in ($candidates | Select-Object -Unique)) {
            $installation = Resolve-Game $candidate
            if ($installation) { break }
        }
        if (-not $NonInteractive) {
            if ($installation) {
                Write-Host "Found L4D2: $installation"
                if ((Read-Host 'Use this installation? [Y/n]').Trim() -match '^(n|no)$') { $installation = $null }
            }
            while (-not $installation) {
                $answer = Read-Host 'Enter your Left 4 Dead 2 folder, or Q to cancel'
                if ($answer -match '^(q|quit)$') { throw 'Build cancelled.' }
                $installation = Resolve-Game $answer
                if (-not $installation) { Write-Host 'L4D2 executable, base VPK and Hotel map were not found there.' }
            }
        }
    }
    if (-not $installation) { throw 'L4D2 not found. Run BUILD.cmd interactively or pass -GameDir.' }
    Write-Host "Verified installation: $installation"
    if ($CheckOnly) { exit 0 }
    if (-not (Get-Command cargo -ErrorAction SilentlyContinue) -or -not (Get-Command rustc -ErrorAction SilentlyContinue)) {
        throw 'Install Rust 1.95+ with the Windows MSVC toolchain, then reopen the terminal. See README.md.'
    }
    $rustVersion = & rustc --version
    if ($LASTEXITCODE -ne 0 -or $rustVersion -notmatch 'rustc (\d+\.\d+\.\d+)') { throw 'Could not determine the Rust version.' }
    if ([version]$Matches[1] -lt [version]'1.95.0') { throw 'This build needs Rust 1.95 or newer.' }
    Push-Location $projectRoot
    try {
        New-Item -ItemType Directory -Force -Path 'runtime','local-research/parity' | Out-Null
        Write-Host 'Building release. The first build downloads dependencies and can take several minutes.'
        $buildArgs = @('build','--locked','--release','--bin','l4d2-rewrite')
        if ($Offline) { $buildArgs += '--offline' }
        & cargo @buildArgs
        if ($LASTEXITCODE -ne 0) { throw 'Build failed. Check the Cargo output; Windows also needs Visual Studio C++ Build Tools and a Windows SDK.' }
        $targetRoot = if ($env:CARGO_TARGET_DIR) { $env:CARGO_TARGET_DIR } else { 'target' }
        Copy-Item -LiteralPath (Join-Path $targetRoot 'release/l4d2-rewrite.exe') -Destination 'runtime/l4d2-rewrite.exe' -Force
        # Steamworks (Steam name, lobbies, relayed connections): the exe
        # loads steam_api64.dll from its own folder.
        $steamDll = Get-ChildItem -Path (Join-Path $targetRoot 'release/build') -Filter 'steam_api64.dll' -Recurse -ErrorAction SilentlyContinue | Sort-Object LastWriteTime -Descending | Select-Object -First 1
        if ($steamDll) { Copy-Item -LiteralPath $steamDll.FullName -Destination 'runtime/steam_api64.dll' -Force }
        [IO.File]::WriteAllText((Join-Path $projectRoot 'runtime/game-dir.txt'), $installation)
        Write-Host 'Build complete. Open PLAY.cmd, or TEST MAP.cmd for the laboratory.' -ForegroundColor Green
    } finally { Pop-Location }
} catch {
    Write-Host $_.Exception.Message -ForegroundColor Red
    exit 1
}
