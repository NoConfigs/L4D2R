# Installs the Android build toolchain for tools/build-apk.ps1 under
# %LOCALAPPDATA%\l4d2r-android (no admin rights needed):
#   Microsoft OpenJDK 17, Android SDK command-line tools, platform-tools,
#   platforms;android-34, build-tools;34.0.0, ndk;27.2.12479018,
#   the Rust aarch64-linux-android target and cargo-apk.
# Running it accepts the Android SDK license agreements (sdkmanager --licenses).
# Safe to run again: finished steps are skipped.
$ErrorActionPreference = 'Stop'
$root = Join-Path $env:LOCALAPPDATA 'l4d2r-android'
$downloads = Join-Path $root 'dl'
New-Item -ItemType Directory -Force $downloads | Out-Null

function Step($text) { Write-Host "== $text" }

$jdk = Get-ChildItem $root -Directory -Filter 'jdk-17*' -ErrorAction SilentlyContinue | Select-Object -First 1
if (-not $jdk) {
    Step 'Downloading Microsoft OpenJDK 17'
    $zip = Join-Path $downloads 'jdk17.zip'
    Invoke-WebRequest 'https://aka.ms/download-jdk/microsoft-jdk-17-windows-x64.zip' -OutFile $zip
    Expand-Archive $zip -DestinationPath $root -Force
    $jdk = Get-ChildItem $root -Directory -Filter 'jdk-17*' | Select-Object -First 1
}
$env:JAVA_HOME = $jdk.FullName
$env:PATH = "$($jdk.FullName)\bin;$env:PATH"

$sdk = Join-Path $root 'sdk'
$sdkmanager = Join-Path $sdk 'cmdline-tools\latest\bin\sdkmanager.bat'
if (-not (Test-Path $sdkmanager)) {
    Step 'Downloading Android SDK command-line tools'
    $zip = Join-Path $downloads 'cmdline-tools.zip'
    Invoke-WebRequest 'https://dl.google.com/android/repository/commandlinetools-win-11076708_latest.zip' -OutFile $zip
    $tmp = Join-Path $root 'sdk-tmp'
    Expand-Archive $zip -DestinationPath $tmp -Force
    New-Item -ItemType Directory -Force (Join-Path $sdk 'cmdline-tools') | Out-Null
    Move-Item (Join-Path $tmp 'cmdline-tools') (Join-Path $sdk 'cmdline-tools\latest')
    Remove-Item $tmp -Recurse -Force
}
if (-not (Test-Path (Join-Path $sdk 'ndk\27.2.12479018'))) {
    Step 'Accepting Android SDK licenses and installing SDK packages (~3 GB)'
    $yes = ('y' + "`n") * 20
    $yes | & $sdkmanager "--sdk_root=$sdk" --licenses | Out-Null
    & $sdkmanager "--sdk_root=$sdk" 'platform-tools' 'platforms;android-34' 'build-tools;34.0.0' 'ndk;27.2.12479018'
}
Step 'Rust Android target'
rustup target add aarch64-linux-android
if (-not (Get-Command cargo-apk -ErrorAction SilentlyContinue)) {
    Step 'Installing cargo-apk'
    cargo install cargo-apk
}
$keystore = Join-Path $root 'release.keystore'
if (-not (Test-Path $keystore)) {
    Step 'Creating a local signing key'
    $password = [Convert]::ToBase64String((1..18 | ForEach-Object { Get-Random -Maximum 256 }) -as [byte[]])
    Set-Content -Path (Join-Path $root 'keystore-password.txt') -Value $password -NoNewline
    & (Join-Path $jdk.FullName 'bin\keytool.exe') -genkeypair -keystore $keystore -alias l4d2r -keyalg RSA `
        -keysize 2048 -validity 10000 -storepass $password -keypass $password -dname 'CN=L4D2 Rewrite local build'
}
Step 'Android toolchain ready'
