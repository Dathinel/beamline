# Genera dist\AVISOS-TERCEROS.txt: los textos de licencia y avisos de lo que se distribuye con BeamLine.
# - Rust: cada crate que entra en beamline-red para Windows o Linux (cargo tree), con sus LICENSE*/COPYING*/NOTICE*;
#   los textos iguales salen una vez, con la lista de crates que los usan.
# - .NET: el runtime (va dentro del exe con Native AOT), Windows App SDK (Windows) y SkiaSharp/HarfBuzzSharp con sus
#   avisos de terceros (Linux; se leen del volumen de Docker beamline-nuget, que deja build\app-linux.ps1).
# Uso: powershell -NoProfile -ExecutionPolicy Bypass -File build\avisos.ps1
$ErrorActionPreference = 'Stop'
$Raiz = Split-Path -Parent $PSScriptRoot
. (Join-Path $PSScriptRoot 'entorno.ps1')
$Dist = Join-Path $Raiz 'dist'
New-Item -ItemType Directory -Force $Dist | Out-Null
$sb = New-Object System.Text.StringBuilder
function Linea([string]$t = '') { [void]$sb.AppendLine($t) }

Linea 'Avisos de terceros de BeamLine'
Linea '=============================='
Linea 'BeamLine (licencia en LICENSE) incluye los componentes de abajo. Resumen por componente en LICENCIAS.md.'
Linea 'Uno Platform (Uno.WinUI, Uno.Sdk, Uno.Fonts.*) va con Apache-2.0: su texto está entre los de los crates de Rust.'
Linea ''

# ---------- Rust ----------
Push-Location (Join-Path $Raiz 'red')
try {
    $meta = (cargo metadata --format-version 1 --locked | Out-String) | ConvertFrom-Json
    $usados = @{}
    foreach ($t in 'x86_64-pc-windows-msvc', 'x86_64-unknown-linux-gnu') {
        cargo tree --locked --target $t -e normal --prefix none -f '{p}' | ForEach-Object {
            $p = ($_ -replace ' \(\*\)$', '' -replace ' \(proc-macro\)$', '').Trim()
            if ($p) { $usados[$p] = $true }
        }
    }
} finally { Pop-Location }
if ($usados.Count -lt 50) { throw "cargo tree no dio la lista de crates ($($usados.Count))." }
$textos = [ordered]@{}
foreach ($pkg in $meta.packages) {
    if ($pkg.source -notlike 'registry+*') { continue }  # lo propio y Sala (por ruta) no son de terceros
    if (-not $usados.ContainsKey("$($pkg.name) v$($pkg.version)")) { continue }
    $dir = Split-Path -Parent $pkg.manifest_path
    $archivos = Get-ChildItem -LiteralPath $dir -File | Where-Object { $_.Name -match '^(LICEN[CS]E|COPYING|NOTICE|UNLICENSE)' } | Sort-Object Name
    $quien = "$($pkg.name) $($pkg.version) ($($pkg.license))"
    if (-not $archivos) {
        $clave = "(sin archivo de licencia en el paquete: $($pkg.license))"
        if (-not $textos.Contains($clave)) { $textos[$clave] = New-Object System.Collections.Generic.List[string] }
        $textos[$clave].Add($quien)
        continue
    }
    foreach ($a in $archivos) {
        $texto = [IO.File]::ReadAllText($a.FullName).Trim()
        if (-not $textos.Contains($texto)) { $textos[$texto] = New-Object System.Collections.Generic.List[string] }
        $textos[$texto].Add($quien)
    }
}
Linea "== Crates de Rust (beamline-red): $($usados.Count) paquetes =="
Linea ''
foreach ($t in $textos.Keys) {
    Linea ('-' * 78)
    Linea ('Usado por: ' + (($textos[$t] | Sort-Object -Unique) -join ', '))
    Linea ''
    Linea $t
    Linea ''
}

# ---------- .NET ----------
function Archivo([string]$titulo, [string]$ruta) {
    if (-not (Test-Path -LiteralPath $ruta)) { throw "Falta $ruta ($titulo)." }
    Linea ('=' * 78); Linea "== $titulo"; Linea ''
    Linea ([IO.File]::ReadAllText($ruta).Trim()); Linea ''
}
$dotnet = $env:DOTNET_ROOT
Archivo '.NET (runtime dentro del ejecutable): licencia' (Join-Path $dotnet 'LICENSE.txt')
Archivo '.NET: avisos de terceros' (Join-Path $dotnet 'ThirdPartyNotices.txt')
$nuget = $env:NUGET_PACKAGES
foreach ($p in 'microsoft.windowsappsdk.winui', 'microsoft.windowsappsdk.foundation', 'microsoft.windowsappsdk.base') {
    $d = Get-ChildItem -LiteralPath (Join-Path $nuget $p) -Directory | Sort-Object Name | Select-Object -Last 1
    Archivo "$p $($d.Name) (solo Windows): licencia" (Join-Path $d.FullName 'license.txt')
    if (Test-Path -LiteralPath (Join-Path $d.FullName 'NOTICE.txt')) { Archivo "$p $($d.Name): avisos" (Join-Path $d.FullName 'NOTICE.txt') }
}
# SkiaSharp y HarfBuzzSharp nativos de Linux: del volumen de NuGet del contenedor.
$tmp = Join-Path $Dist 'avisos-linux'
Remove-Item -Recurse -Force $tmp -ErrorAction SilentlyContinue
New-Item -ItemType Directory -Force $tmp | Out-Null
docker run --rm -v beamline-nuget:/n -v "${tmp}:/o" beamline-linux sh -c 'for p in skiasharp.nativeassets.linux harfbuzzsharp.nativeassets.linux; do d=$(ls -d /n/$p/*/ | tail -1); cp $d/LICENSE.txt /o/$p.LICENSE.txt; cp $d/THIRD-PARTY-NOTICES.txt /o/$p.THIRD-PARTY-NOTICES.txt; done' | Out-Null
if ($LASTEXITCODE) { throw 'No se pudieron leer los avisos de Skia y HarfBuzz del volumen beamline-nuget (corre antes build\app-linux.ps1).' }
foreach ($f in Get-ChildItem -LiteralPath $tmp -File | Sort-Object Name) { Archivo "$($f.BaseName) (solo Linux)" $f.FullName }
Remove-Item -Recurse -Force $tmp

$salida = Join-Path $Dist 'AVISOS-TERCEROS.txt'
[IO.File]::WriteAllText($salida, $sb.ToString(), (New-Object Text.UTF8Encoding $false))
Write-Host "Avisos: $salida ($([math]::Round((Get-Item $salida).Length / 1KB)) KB, $($usados.Count) crates, $($textos.Count) textos distintos)"
