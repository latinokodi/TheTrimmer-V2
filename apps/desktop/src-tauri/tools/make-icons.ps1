# Generates TheTrimmer's application icons.
#
# The mark is the product in one glyph: a horizontal bar representing a segment, with the first part
# in the head-patch colour (the frames a cut has to re-encode) and the rest in the body colour (the
# frames that are the original packets). That distinction is the entire product, so it is the icon.
#
# Run from apps/desktop/src-tauri:
#     pwsh -File tools/make-icons.ps1
#
# The output is deterministic, so an icon change is a reviewable diff rather than an opaque binary.

param(
    [string]$OutDir = (Join-Path $PSScriptRoot '..' 'icons')
)

$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName System.Drawing

$surface = [System.Drawing.Color]::FromArgb(255, 16, 19, 23)
$body = [System.Drawing.Color]::FromArgb(255, 231, 236, 242)
$head = [System.Drawing.Color]::FromArgb(255, 185, 138, 224)

function New-Mark {
    param([int]$Size)

    $bmp = New-Object System.Drawing.Bitmap($Size, $Size, [System.Drawing.Imaging.PixelFormat]::Format32bppArgb)
    $g = [System.Drawing.Graphics]::FromImage($bmp)
    $g.SmoothingMode = [System.Drawing.Drawing2D.SmoothingMode]::AntiAlias
    $g.InterpolationMode = [System.Drawing.Drawing2D.InterpolationMode]::HighQualityBicubic
    $g.Clear([System.Drawing.Color]::Transparent)

    # Rounded background.
    $pad = [math]::Max(1, [int]($Size * 0.055))
    $radius = [int]($Size * 0.22)
    $d = $radius * 2
    $path = New-Object System.Drawing.Drawing2D.GraphicsPath
    $path.AddArc($pad, $pad, $d, $d, 180, 90)
    $path.AddArc($Size - $pad - $d, $pad, $d, $d, 270, 90)
    $path.AddArc($Size - $pad - $d, $Size - $pad - $d, $d, $d, 0, 90)
    $path.AddArc($pad, $Size - $pad - $d, $d, $d, 90, 90)
    $path.CloseFigure()
    $g.FillPath((New-Object System.Drawing.SolidBrush($surface)), $path)

    # The segment bar: head third in the head colour, the rest in the body colour.
    $barLeft = [int]($Size * 0.16)
    $barRight = [int]($Size * 0.84)
    $barTop = [int]($Size * 0.40)
    $barHeight = [math]::Max(2, [int]($Size * 0.20))
    $barRadius = [int]($barHeight / 2)
    $sweep = [int]($Size * 0.30)

    $bar = New-Object System.Drawing.Drawing2D.GraphicsPath
    $bd = $barRadius * 2
    $bar.AddArc($barLeft, $barTop, $bd, $bd, 180, 90)
    $bar.AddArc($barRight - $bd, $barTop, $bd, $bd, 270, 90)
    $bar.AddArc($barRight - $bd, $barTop + $barHeight - $bd, $bd, $bd, 0, 90)
    $bar.AddArc($barLeft, $barTop + $barHeight - $bd, $bd, $bd, 90, 90)
    $bar.CloseFigure()
    $g.FillPath((New-Object System.Drawing.SolidBrush($body)), $bar)

    # Fill the head sweep first, clipped to the bar's own rounded shape, so the two colours meet
    # exactly at the keyframe tick instead of overlapping by a pixel.
    $clipState = $g.Save()
    $g.SetClip($bar, [System.Drawing.Drawing2D.CombineMode]::Intersect)
    $headRect = New-Object System.Drawing.Rectangle($barLeft, ($barTop - 2), $sweep, ($barHeight + 4))
    $g.SetClip($headRect, [System.Drawing.Drawing2D.CombineMode]::Intersect)
    $g.FillRectangle((New-Object System.Drawing.SolidBrush($head)), $headRect)
    $g.Restore($clipState)

    # A keyframe tick where the copy begins, which is the mark the whole method turns on.
    $tickX = $barLeft + $sweep
    $tickPen = New-Object System.Drawing.Pen($surface, [math]::Max(1, [int]($Size * 0.035)))
    $tickHalf = [int]($Size * 0.10)
    $g.DrawLine($tickPen, $tickX, ($barTop - $tickHalf), $tickX, ($barTop + $barHeight + $tickHalf))

    $g.Dispose()
    return $bmp
}

New-Item -ItemType Directory -Force -Path $OutDir | Out-Null

$sizes = @(16, 24, 32, 48, 64, 128, 256)
$bitmaps = @{}
foreach ($size in $sizes) {
    $bitmaps[$size] = New-Mark -Size $size
}

# PNGs, for the platforms that want them.
foreach ($size in @(32, 128, 256)) {
    $target = Join-Path $OutDir "${size}x${size}.png"
    $bitmaps[$size].Save($target, [System.Drawing.Imaging.ImageFormat]::Png)
    Write-Host "wrote $target"
}
$bitmaps[256].Save((Join-Path $OutDir 'icon.png'), [System.Drawing.Imaging.ImageFormat]::Png)
Write-Host "wrote $(Join-Path $OutDir 'icon.png')"

# A real multi-image ICO: every size in one file, which is what Windows wants so the taskbar, the
# alt-tab switcher and Explorer each get a size drawn for them rather than a downscaled 256.
$icoSizes = @(16, 24, 32, 48, 64, 128, 256)
$pngBytes = @{}
foreach ($size in $icoSizes) {
    $ms = New-Object System.IO.MemoryStream
    $bitmaps[$size].Save($ms, [System.Drawing.Imaging.ImageFormat]::Png)
    $pngBytes[$size] = $ms.ToArray()
    $ms.Dispose()
}

$icoPath = Join-Path $OutDir 'icon.ico'
$stream = [System.IO.File]::Create($icoPath)
$writer = New-Object System.IO.BinaryWriter($stream)
$writer.Write([uint16]0)                  # reserved
$writer.Write([uint16]1)                  # type: icon
$writer.Write([uint16]$icoSizes.Count)    # image count
$offset = 6 + (16 * $icoSizes.Count)
foreach ($size in $icoSizes) {
    $bytes = $pngBytes[$size]
    $dim = if ($size -ge 256) { 0 } else { $size }
    $writer.Write([byte]$dim)             # width
    $writer.Write([byte]$dim)             # height
    $writer.Write([byte]0)                # palette count
    $writer.Write([byte]0)                # reserved
    $writer.Write([uint16]1)              # colour planes
    $writer.Write([uint16]32)             # bits per pixel
    $writer.Write([uint32]$bytes.Length)  # size of the image data
    $writer.Write([uint32]$offset)        # offset of the image data
    $offset += $bytes.Length
}
foreach ($size in $icoSizes) {
    $writer.Write($pngBytes[$size])
}
$writer.Flush()
$writer.Dispose()
$stream.Dispose()
Write-Host "wrote $icoPath"

foreach ($size in $sizes) { $bitmaps[$size].Dispose() }
