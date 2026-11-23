# Genera assets\marca\beamline.ico (16 a 256 px, PNG dentro del .ico) y beamline-256.png desde los SVG de
# assets\marca con el ffmpeg (librsvg) del .entorno de Rodaje. De 16 a 32 px usa beamline-pequeno.svg.
# El resultado se versiona: compilar no necesita este script.
#   powershell -NoProfile -ExecutionPolicy Bypass -File build\generar-iconos.ps1
$ErrorActionPreference = 'Stop'
$raiz = Split-Path -Parent $PSScriptRoot
$marca = Join-Path $raiz 'assets\marca'
$ffmpeg = Join-Path $raiz '..\Rodaje\.entorno\ffmpeg\bin\ffmpeg.exe'
if (-not (Test-Path -LiteralPath $ffmpeg)) { throw "Falta $ffmpeg (el .entorno de Rodaje)." }
$tamanos = 16, 20, 24, 32, 40, 48, 64, 128, 256
$tmp = Join-Path ([IO.Path]::GetTempPath()) ('beamline-iconos-' + [Guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $tmp | Out-Null
try {
    $datos = foreach ($t in $tamanos) {
        $svg = Join-Path $marca $(if ($t -le 32) { 'beamline-pequeno.svg' } else { 'beamline.svg' })
        $png = Join-Path $tmp "$t.png"
        & $ffmpeg -hide_banner -v error -y -width $t -height $t -i $svg -frames:v 1 -pix_fmt rgba $png
        if ($LASTEXITCODE -ne 0 -or -not (Test-Path -LiteralPath $png)) { throw "ffmpeg no pudo rasterizar $svg a $t px." }
        , [IO.File]::ReadAllBytes($png)
    }
    Copy-Item -LiteralPath (Join-Path $tmp '256.png') (Join-Path $marca 'beamline-256.png') -Force
    $ms = New-Object IO.MemoryStream
    $w = New-Object IO.BinaryWriter($ms)
    $w.Write([uint16]0); $w.Write([uint16]1); $w.Write([uint16]$tamanos.Count)
    $offset = 6 + 16 * $tamanos.Count
    for ($i = 0; $i -lt $tamanos.Count; $i++) {
        $l = if ($tamanos[$i] -ge 256) { 0 } else { $tamanos[$i] }
        $w.Write([byte]$l); $w.Write([byte]$l); $w.Write([byte]0); $w.Write([byte]0)
        $w.Write([uint16]1); $w.Write([uint16]32); $w.Write([uint32]$datos[$i].Length); $w.Write([uint32]$offset)
        $offset += $datos[$i].Length
    }
    foreach ($d in $datos) { $w.Write($d) }
    $w.Flush()
    [IO.File]::WriteAllBytes((Join-Path $marca 'beamline.ico'), $ms.ToArray())
    Write-Host "Icono: $(Join-Path $marca 'beamline.ico')"
} finally { Remove-Item -LiteralPath $tmp -Recurse -Force -ErrorAction SilentlyContinue }
