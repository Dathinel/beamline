# Distribución de BeamLine para Windows: zip autocontenido, instalador (Inno Setup) y entrada del catálogo del Lanzador
# (sin firmar: la firma la hace quien publica). Antes: build\app.ps1 (deja dist\BeamLine publicado con AOT).
# Uso: powershell -NoProfile -ExecutionPolicy Bypass -File build\distribuir.ps1 [-SinInstalador]
param([switch]$SinInstalador)
$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName System.IO.Compression, System.IO.Compression.FileSystem
$Raiz = Split-Path -Parent $PSScriptRoot
$Version = ([xml](Get-Content -LiteralPath (Join-Path $Raiz 'Directory.Build.props') -Raw)).Project.PropertyGroup.Version
$Dist = Join-Path $Raiz 'dist'
$App = Join-Path $Dist 'BeamLine'
if (-not (Test-Path -LiteralPath (Join-Path $App 'BeamLine.exe'))) { throw 'Falta dist\BeamLine\BeamLine.exe: corre antes build\app.ps1.' }
if (-not (Test-Path -LiteralPath (Join-Path $App 'beamline-red.exe'))) { throw 'Falta beamline-red.exe junto a la app.' }

Copy-Item -LiteralPath (Join-Path $Raiz 'LICENSE'), (Join-Path $Raiz 'LICENCIAS.md') -Destination $App -Force
$leeme = @"
BeamLine $Version

Envía archivos y carpetas de cualquier tamaño a otros equipos: en tu red y por internet, cifrado de extremo a extremo.

- Abre BeamLine.exe en los dos equipos. Los de tu red aparecen solos; a uno de fuera, pásale tu billete («Copiar mi billete»).
- Marca uno o varios equipos y suelta archivos o carpetas en la zona azul. El otro elige qué acepta.
- Si se corta, al volver a enviar sigue donde iba.

Licencia: MIT. Componentes de terceros: LICENCIAS.md.
"@
[IO.File]::WriteAllText((Join-Path $App 'LEEME.txt'), $leeme, (New-Object Text.UTF8Encoding $false))
Get-ChildItem -LiteralPath $App -Recurse -File | Where-Object { $_.Extension -in '.pdb', '.xml' } | Remove-Item -Force

$zip = Join-Path $Dist "BeamLine-$Version-win-x64.zip"
if (Test-Path -LiteralPath $zip) { Remove-Item -LiteralPath $zip -Force }
$z = [IO.Compression.ZipFile]::Open($zip, [IO.Compression.ZipArchiveMode]::Create)
try {
    $padre = (Resolve-Path -LiteralPath $App).Path.TrimEnd('\') + '\'
    foreach ($f in Get-ChildItem -LiteralPath $App -Recurse -File) {
        [IO.Compression.ZipFileExtensions]::CreateEntryFromFile($z, $f.FullName, 'BeamLine/' + $f.FullName.Substring($padre.Length).Replace('\', '/'), [IO.Compression.CompressionLevel]::Optimal) | Out-Null
    }
} finally { $z.Dispose() }
Write-Host "Zip: $zip ($([math]::Round((Get-Item $zip).Length / 1MB, 1)) MB)"

# Entrada «app» del catálogo del Lanzador (formato de Subtitular). La URL supone el repositorio Dathinel/beamline.
function Huella([string]$ruta) {
    $h = [Security.Cryptography.SHA256]::Create(); $s = [IO.File]::OpenRead($ruta)
    try { [BitConverter]::ToString($h.ComputeHash($s)).Replace('-', '').ToLowerInvariant() } finally { $s.Dispose(); $h.Dispose() }
}
$entrada = [ordered]@{
    id = 'beamline'; version = $Version; kind = 'app'
    url = "https://github.com/Dathinel/beamline/releases/download/v$Version/BeamLine-$Version-win-x64.zip"
    sha256 = Huella $zip; size = (Get-Item $zip).Length; modulos = @(); modules = @()
}
$linux = Join-Path $Dist "BeamLine-$Version-linux-x64.tar.gz"
if (Test-Path -LiteralPath $linux) {
    $entrada.linux = [ordered]@{ url = "https://github.com/Dathinel/beamline/releases/download/v$Version/BeamLine-$Version-linux-x64.tar.gz"; sha256 = Huella $linux; size = (Get-Item $linux).Length }
}
[IO.File]::WriteAllText((Join-Path $Dist 'catalogo-beamline.json'), ($entrada | ConvertTo-Json -Depth 4), (New-Object Text.UTF8Encoding $false))
Write-Host "Entrada del catálogo (sin firmar): dist\catalogo-beamline.json"

if (-not $SinInstalador) {
    # shortcut: usa el Inno Setup 7.1.0 del .entorno de Rodaje; uno propio llega con preparar-entorno.ps1 cuando haya CI.
    $iscc = Join-Path (Split-Path -Parent $Raiz) 'Rodaje\.entorno\innosetup-7.1.0\ISCC.exe'
    if (-not (Test-Path -LiteralPath $iscc)) { throw "No encuentro Inno Setup en $iscc." }
    & $iscc '/Q' "/DVersion=$Version" "/DOrigen=$App" "/DLicencia=$(Join-Path $Raiz 'LICENSE')" "/O$Dist" (Join-Path $PSScriptRoot 'instalador\BeamLine.iss')
    if ($LASTEXITCODE) { throw "ISCC terminó con código $LASTEXITCODE." }
    Write-Host "Instalador: $(Join-Path $Dist "BeamLine-$Version-instalador.exe")"
}
