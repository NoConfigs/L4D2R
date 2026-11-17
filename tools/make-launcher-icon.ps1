# Builds the launcher's branding from the L4D2R logo PNG:
#   launcher/assets/logo.png  the logo cropped to its artwork (header image)
#   launcher/app.ico          multi-size icon (exe, window and taskbar)
#   tools/make-launcher-icon.ps1 -Source path\to\logo.png
param([Parameter(Mandatory = $true)][string]$Source)
$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName System.Drawing
$root = Split-Path $PSScriptRoot -Parent
$assets = Join-Path $root 'launcher\assets'
New-Item -ItemType Directory -Force $assets | Out-Null

$src = [System.Drawing.Bitmap]::FromFile((Resolve-Path $Source))
# Artwork bounds: pixels that are visible and not near-black.
$minx = $src.Width; $miny = $src.Height; $maxx = 0; $maxy = 0
for ($y = 0; $y -lt $src.Height; $y += 2) {
    for ($x = 0; $x -lt $src.Width; $x += 2) {
        $p = $src.GetPixel($x, $y)
        if ($p.A -gt 24) {
            if ($x -lt $minx) { $minx = $x }; if ($x -gt $maxx) { $maxx = $x }
            if ($y -lt $miny) { $miny = $y }; if ($y -gt $maxy) { $maxy = $y }
        }
    }
}
$crop = New-Object System.Drawing.Rectangle ($minx, $miny, ($maxx - $minx + 2), ($maxy - $miny + 2))
$logo = $src.Clone($crop, [System.Drawing.Imaging.PixelFormat]::Format32bppArgb)
$logo.Save((Join-Path $assets 'logo.png'), [System.Drawing.Imaging.ImageFormat]::Png)
Write-Host "Logo: $($logo.Width)x$($logo.Height)"

# Icon frames: the logo centred on a transparent square, PNG-encoded.
$sizes = 16, 20, 24, 32, 40, 48, 64, 128, 256
$frames = foreach ($size in $sizes) {
    $bmp = New-Object System.Drawing.Bitmap $size, $size, ([System.Drawing.Imaging.PixelFormat]::Format32bppArgb)
    $g = [System.Drawing.Graphics]::FromImage($bmp)
    $g.InterpolationMode = [System.Drawing.Drawing2D.InterpolationMode]::HighQualityBicubic
    $g.PixelOffsetMode = [System.Drawing.Drawing2D.PixelOffsetMode]::HighQuality
    $g.SmoothingMode = [System.Drawing.Drawing2D.SmoothingMode]::HighQuality
    $g.Clear([System.Drawing.Color]::Transparent)
    $scale = [Math]::Min($size / $logo.Width, $size / $logo.Height)
    $w = $logo.Width * $scale; $h = $logo.Height * $scale
    $g.DrawImage($logo, [single](($size - $w) / 2), [single](($size - $h) / 2), [single]$w, [single]$h)
    $g.Dispose()
    $ms = New-Object System.IO.MemoryStream
    if ($size -ge 128) {
        # Large frames as PNG (Vista+ icon format).
        $bmp.Save($ms, [System.Drawing.Imaging.ImageFormat]::Png)
    } else {
        # Small frames as classic 32-bit DIBs: header, bottom-up BGRA, AND mask.
        $w = New-Object System.IO.BinaryWriter $ms
        $w.Write([UInt32]40); $w.Write([Int32]$size); $w.Write([Int32]($size * 2)); $w.Write([UInt16]1); $w.Write([UInt16]32)
        $w.Write([UInt32]0); $w.Write([UInt32]0); $w.Write([Int32]0); $w.Write([Int32]0); $w.Write([UInt32]0); $w.Write([UInt32]0)
        for ($y = $size - 1; $y -ge 0; $y--) {
            for ($x = 0; $x -lt $size; $x++) { $c = $bmp.GetPixel($x, $y); $w.Write([byte]$c.B); $w.Write([byte]$c.G); $w.Write([byte]$c.R); $w.Write([byte]$c.A) }
        }
        $maskRow = [int]([Math]::Ceiling($size / 32.0) * 4)
        $w.Write((New-Object byte[] ($maskRow * $size)))
        $w.Flush()
    }
    $bmp.Dispose()
    , @($size, $ms.ToArray())
}
$out = New-Object System.IO.MemoryStream
$bw = New-Object System.IO.BinaryWriter $out
$bw.Write([UInt16]0); $bw.Write([UInt16]1); $bw.Write([UInt16]$frames.Count)
$offset = 6 + 16 * $frames.Count
foreach ($f in $frames) {
    $s = $f[0]; $data = $f[1]
    $bw.Write([byte]($(if ($s -ge 256) { 0 } else { $s }))); $bw.Write([byte]($(if ($s -ge 256) { 0 } else { $s })))
    $bw.Write([byte]0); $bw.Write([byte]0); $bw.Write([UInt16]1); $bw.Write([UInt16]32)
    $bw.Write([UInt32]$data.Length); $bw.Write([UInt32]$offset)
    $offset += $data.Length
}
foreach ($f in $frames) { $bw.Write([byte[]]$f[1]) }
$bw.Flush()
[System.IO.File]::WriteAllBytes((Join-Path $root 'launcher\app.ico'), $out.ToArray())
Write-Host "Icon: $($frames.Count) sizes"
$logo.Dispose(); $src.Dispose()
