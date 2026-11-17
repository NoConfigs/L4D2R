# Builds the Android APK (android/Cargo.toml, arm64) into artifacts\android.
#
#   -GameData None    APK only (~45 MB). The game looks for its data in the
#                     app folder on the phone (see -GameData Push).
#   -GameData Push    APK only, then copies the Left 4 Dead 2 folders the game
#                     reads to the connected phone over USB (adb):
#                     /sdcard/Android/data/com.l4d2r.rewrite/files/Left 4 Dead 2
#   -GameData Bundle  Packs those folders into the APK; it is unpacked to the
#                     same app folder on first launch. Android cannot install
#                     an APK over 4 GB, so a full install (~15 GB) does not
#                     fit; the build stops if the data is too large.
#   -Install          adb install the APK on the connected phone.
param(
    [ValidateSet('None', 'Push', 'Bundle')][string]$GameData = 'None',
    [string]$GameDir = '',
    [switch]$Install
)
$ErrorActionPreference = 'Stop'
$repo = Split-Path $PSScriptRoot -Parent
$root = Join-Path $env:LOCALAPPDATA 'l4d2r-android'
$package = 'com.l4d2r.rewrite'
$folders = @('update', 'left4dead2_dlc3', 'left4dead2_dlc2', 'left4dead2_dlc1', 'left4dead2', 'hl2')
# Not read by the game.
$skip = @('*.mdmp', '*.dmp', '*.log', '*.cache')
function Step($text) { Write-Host "== $text" }

if (-not $GameDir) {
    $saved = Join-Path $repo 'runtime\game-dir.txt'
    $GameDir = if (Test-Path $saved) { (Get-Content $saved -Raw).Trim() } else {
        'C:\Program Files (x86)\Steam\steamapps\common\Left 4 Dead 2' }
}
if (-not (Test-Path (Join-Path $root 'sdk\ndk\27.2.12479018')) -or -not (Get-Command cargo-apk -ErrorAction SilentlyContinue)) {
    Step 'Android toolchain missing: running tools\setup-android.ps1'
    & (Join-Path $PSScriptRoot 'setup-android.ps1')
}
$jdk = Get-ChildItem $root -Directory -Filter 'jdk-17*' | Select-Object -First 1
$env:JAVA_HOME = $jdk.FullName
$env:ANDROID_HOME = Join-Path $root 'sdk'
$env:ANDROID_SDK_ROOT = $env:ANDROID_HOME
$env:ANDROID_NDK_ROOT = Join-Path $env:ANDROID_HOME 'ndk\27.2.12479018'
$env:ANDROID_NDK_HOME = $env:ANDROID_NDK_ROOT
$env:CARGO_APK_RELEASE_KEYSTORE = Join-Path $root 'release.keystore'
$env:CARGO_APK_RELEASE_KEYSTORE_PASSWORD = (Get-Content (Join-Path $root 'keystore-password.txt') -Raw).Trim()
$adb = Join-Path $env:ANDROID_HOME 'platform-tools\adb.exe'

function GameFiles {
    foreach ($folder in $folders) {
        $path = Join-Path $GameDir $folder
        if (Test-Path $path) {
            Get-ChildItem $path -Recurse -File | Where-Object {
                $name = $_.Name; -not ($skip | Where-Object { $name -like $_ })
            }
        }
    }
}

Step 'Preparing APK assets'
$assets = Join-Path $repo 'android\apk-assets'
if (Test-Path $assets) { Remove-Item $assets -Recurse -Force }
New-Item -ItemType Directory -Force $assets | Out-Null
# Bevy reads shaders etc. straight from the APK assets.
Copy-Item (Join-Path $repo 'assets\*') $assets -Recurse
$data = Join-Path $assets 'l4d2r-data'
New-Item -ItemType Directory -Force (Join-Path $data 'maps') | Out-Null
Copy-Item (Join-Path $repo 'maps\rewrite_lab.*') (Join-Path $data 'maps')
$manifest = [System.Collections.Generic.List[string]]::new()
Get-ChildItem $data -Recurse -File | ForEach-Object {
    $manifest.Add($_.FullName.Substring($data.Length + 1).Replace('\', '/'))
}
if ($GameData -eq 'Bundle') {
    $files = @(GameFiles)
    $bytes = ($files | Measure-Object Length -Sum).Sum
    $gb = [math]::Round($bytes / 1GB, 2)
    if ($bytes -gt 3.8GB) {
        throw "The game data is $gb GB; an APK must stay under 4 GB. Use -GameData Push to copy it to the phone instead."
    }
    Step "Bundling $($files.Count) game files ($gb GB)"
    foreach ($file in $files) {
        $relative = 'Left 4 Dead 2/' + $file.FullName.Substring($GameDir.TrimEnd('\').Length + 1).Replace('\', '/')
        $target = Join-Path $data $relative
        New-Item -ItemType Directory -Force (Split-Path $target) | Out-Null
        Copy-Item $file.FullName $target
        $manifest.Add($relative)
    }
}
# UTF-8 without a BOM (the Android side reads it as plain text).
[System.IO.File]::WriteAllText((Join-Path $data 'manifest.txt'), ($manifest -join "`n"))

Step 'Building the APK (cargo apk build --release)'
Push-Location (Join-Path $repo 'android')
try {
    cargo apk build --release
    if ($LASTEXITCODE -ne 0) { throw "cargo apk build failed ($LASTEXITCODE)" }
} finally { Pop-Location }
$out = Join-Path $repo 'artifacts\android'
New-Item -ItemType Directory -Force $out | Out-Null
$apk = Join-Path $out 'L4D2Rewrite.apk'
Copy-Item (Join-Path $repo 'android\target\release\apk\L4D2Rewrite.apk') $apk -Force
Step "APK: $apk ($([math]::Round((Get-Item $apk).Length / 1MB)) MB)"

if ($Install -or $GameData -eq 'Push') {
    $devices = & $adb devices | Select-String "`tdevice$"
    if (-not $devices) { throw 'No phone found by adb. Enable USB debugging and connect the phone.' }
}
if ($Install) {
    Step 'Installing on the phone'
    & $adb install -r $apk
    if ($LASTEXITCODE -ne 0) { throw 'adb install failed' }
}
if ($GameData -eq 'Push') {
    $remote = "/sdcard/Android/data/$package/files/Left 4 Dead 2"
    Step "Copying the game data to the phone ($remote)"
    & $adb shell mkdir -p "'$remote'"
    foreach ($folder in $folders) {
        $path = Join-Path $GameDir $folder
        if (Test-Path $path) {
            Step "  $folder"
            & $adb push --sync $path "$remote/"
            if ($LASTEXITCODE -ne 0) { throw "adb push of $folder failed" }
        }
    }
}
Step 'Done'
