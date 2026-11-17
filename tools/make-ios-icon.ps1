# Convert the launcher's original branding into the iPhone/iPad app icon.
# This preserves the artwork and centred aspect ratio used by launcher/app.ico.
param([string]$Source = (Join-Path $PSScriptRoot '..\launcher\assets\logo.png'))
$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName System.Drawing
$root = Split-Path $PSScriptRoot -Parent
$catalog = Join-Path $root 'ios\Assets.xcassets'
$icons = Join-Path $catalog 'AppIcon.appiconset'
New-Item -ItemType Directory -Path $icons -Force | Out-Null
$utf8 = [Text.UTF8Encoding]::new($false)
[IO.File]::WriteAllText((Join-Path $catalog 'Contents.json'), '{"info":{"author":"xcode","version":1}}', $utf8)

$slots = @(
    @('iphone','20x20','2x',40), @('iphone','20x20','3x',60),
    @('iphone','29x29','2x',58), @('iphone','29x29','3x',87),
    @('iphone','40x40','2x',80), @('iphone','40x40','3x',120),
    @('iphone','60x60','2x',120), @('iphone','60x60','3x',180),
    @('ipad','20x20','1x',20), @('ipad','20x20','2x',40),
    @('ipad','29x29','1x',29), @('ipad','29x29','2x',58),
    @('ipad','40x40','1x',40), @('ipad','40x40','2x',80),
    @('ipad','76x76','1x',76), @('ipad','76x76','2x',152),
    @('ipad','83.5x83.5','2x',167), @('ios-marketing','1024x1024','1x',1024)
)
$sourcePath = (Resolve-Path -LiteralPath $Source).Path
$logo = [Drawing.Bitmap]::FromFile($sourcePath)
try {
    foreach ($size in ($slots | ForEach-Object { [int]$_[3] } | Sort-Object -Unique)) {
        # RGB, with no alpha channel. iOS supplies the home-screen corner mask.
        $bitmap = [Drawing.Bitmap]::new($size, $size, [Drawing.Imaging.PixelFormat]::Format24bppRgb)
        $graphics = [Drawing.Graphics]::FromImage($bitmap)
        try {
            $graphics.Clear([Drawing.Color]::Black)
            $graphics.InterpolationMode = [Drawing.Drawing2D.InterpolationMode]::HighQualityBicubic
            $graphics.PixelOffsetMode = [Drawing.Drawing2D.PixelOffsetMode]::HighQuality
            $graphics.SmoothingMode = [Drawing.Drawing2D.SmoothingMode]::HighQuality
            $scale = [Math]::Min($size / [double]$logo.Width, $size / [double]$logo.Height)
            $w = [single]($logo.Width * $scale); $h = [single]($logo.Height * $scale)
            $graphics.DrawImage($logo, [single](($size-$w)/2), [single](($size-$h)/2), $w, $h)
            $bitmap.Save((Join-Path $icons "AppIcon-$size.png"), [Drawing.Imaging.ImageFormat]::Png)
        } finally { $graphics.Dispose(); $bitmap.Dispose() }
    }
} finally { $logo.Dispose() }
$images = @($slots | ForEach-Object {
    [ordered]@{ idiom=$_[0]; size=$_[1]; scale=$_[2]; filename="AppIcon-$($_[3]).png" }
})
$contents = [ordered]@{ images=$images; info=[ordered]@{ author='xcode'; version=1 } }
[IO.File]::WriteAllText((Join-Path $icons 'Contents.json'), ($contents | ConvertTo-Json -Depth 5), $utf8)
$provenance = [ordered]@{
    source='launcher/assets/logo.png'; sha256=(Get-FileHash -LiteralPath $sourcePath -Algorithm SHA256).Hash.ToLowerInvariant()
    background='#000000'; layout='Same centred, aspect-preserving layout as launcher/app.ico'
}
[IO.File]::WriteAllText((Join-Path $root 'ios\app-icon-source.json'), ($provenance | ConvertTo-Json), $utf8)
Write-Output "iOS app icons generated from $sourcePath (18 slots, 13 PNG sizes)."
