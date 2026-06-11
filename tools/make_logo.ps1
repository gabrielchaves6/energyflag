# Generates the EnergyFlag brand logo as a multi-resolution .ico (PNG-compressed entries)
# plus a PNG for the README. Same DeskFlag-blue rounded square as the sibling apps, with a
# white lightning bolt instead of KeyFlag's keycap.
Add-Type -AssemblyName System.Drawing

function Add-RoundRect([System.Drawing.Drawing2D.GraphicsPath]$p, [single]$x, [single]$y, [single]$w, [single]$h, [single]$r) {
    $d = $r * 2
    $p.AddArc($x, $y, $d, $d, 180, 90)
    $p.AddArc($x + $w - $d, $y, $d, $d, 270, 90)
    $p.AddArc($x + $w - $d, $y + $h - $d, $d, $d, 0, 90)
    $p.AddArc($x, $y + $h - $d, $d, $d, 90, 90)
    $p.CloseFigure()
}

function New-EnergyFlagBitmap([int]$size) {
    [single]$s = $size / 256.0
    $bmp = [System.Drawing.Bitmap]::new($size, $size, [System.Drawing.Imaging.PixelFormat]::Format32bppArgb)
    $g = [System.Drawing.Graphics]::FromImage($bmp)
    $g.SmoothingMode = 'AntiAlias'
    $g.InterpolationMode = 'HighQualityBicubic'
    $g.Clear([System.Drawing.Color]::Transparent)

    # DeskFlag-blue (#2563EB) rounded square — matches DeskFlag/KeyFlag exactly.
    $bg = [System.Drawing.Drawing2D.GraphicsPath]::new()
    Add-RoundRect $bg (8*$s) (8*$s) (240*$s) (240*$s) (56*$s)
    $brush = [System.Drawing.SolidBrush]::new([System.Drawing.Color]::FromArgb(255,37,99,235))
    $g.FillPath($brush, $bg)
    $brush.Dispose(); $bg.Dispose()

    # Lightning bolt "shadow" — a lighter tint of DeskFlag blue offset down, echoing the
    # keycap-depth trick on the KeyFlag logo.
    $bolt = @(
        [System.Drawing.PointF]::new(148*$s,  46*$s),
        [System.Drawing.PointF]::new( 86*$s, 142*$s),
        [System.Drawing.PointF]::new(124*$s, 142*$s),
        [System.Drawing.PointF]::new(106*$s, 210*$s),
        [System.Drawing.PointF]::new(172*$s, 110*$s),
        [System.Drawing.PointF]::new(132*$s, 110*$s)
    )
    $shadow = @()
    foreach ($pt in $bolt) { $shadow += [System.Drawing.PointF]::new($pt.X + 0, $pt.Y + (8*$s)) }
    $sb = [System.Drawing.SolidBrush]::new([System.Drawing.Color]::FromArgb(255,190,212,251))
    $g.FillPolygon($sb, $shadow); $sb.Dispose()

    # Lightning bolt top face — white.
    $wb = [System.Drawing.SolidBrush]::new([System.Drawing.Color]::White)
    $g.FillPolygon($wb, $bolt); $wb.Dispose()

    $g.Dispose()
    return $bmp
}

function Save-Ico([int[]]$sizes, [string]$path) {
    $pngs = @()
    foreach ($sz in $sizes) {
        $bmp = New-EnergyFlagBitmap $sz
        $ms = [System.IO.MemoryStream]::new()
        $bmp.Save($ms, [System.Drawing.Imaging.ImageFormat]::Png)
        $pngs += ,@($sz, $ms.ToArray())
        $bmp.Dispose(); $ms.Dispose()
    }
    $out = [System.IO.MemoryStream]::new()
    $bw = [System.IO.BinaryWriter]::new($out)
    $bw.Write([uint16]0); $bw.Write([uint16]1); $bw.Write([uint16]$pngs.Count) # ICONDIR
    $offset = 6 + 16 * $pngs.Count
    foreach ($e in $pngs) {
        $sz = $e[0]; $bytes = $e[1]
        $bw.Write([byte]($(if ($sz -ge 256) {0} else {$sz})))   # width
        $bw.Write([byte]($(if ($sz -ge 256) {0} else {$sz})))   # height
        $bw.Write([byte]0); $bw.Write([byte]0)                  # colors, reserved
        $bw.Write([uint16]1); $bw.Write([uint16]32)             # planes, bitcount
        $bw.Write([uint32]$bytes.Length)                        # bytesInRes
        $bw.Write([uint32]$offset)                              # imageOffset
        $offset += $bytes.Length
    }
    foreach ($e in $pngs) { $bw.Write([byte[]]$e[1]) }
    $bw.Flush()
    [System.IO.File]::WriteAllBytes($path, $out.ToArray())
    $bw.Dispose(); $out.Dispose()
    Write-Host "wrote $path ($((Get-Item $path).Length) bytes, $($pngs.Count) sizes)"
}

$assets = Join-Path $PSScriptRoot "..\rs\assets"
New-Item -ItemType Directory -Force -Path $assets | Out-Null

Save-Ico @(16,24,32,48,64,128,256) (Join-Path $assets "energyflag.ico")
$png = New-EnergyFlagBitmap 512
$png.Save((Join-Path $assets "logo.png"), [System.Drawing.Imaging.ImageFormat]::Png); $png.Dispose()
Write-Host "wrote logo.png (512)"
