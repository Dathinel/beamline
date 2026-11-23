# Compila, empaqueta y (con -Autoprueba) prueba BeamLine para Linux en un contenedor de Docker (build\linux\Dockerfile).
# Deja dist\BeamLine-linux\ y dist\BeamLine-<versión>-linux-x64.tar.gz. Necesita Docker Desktop en marcha.
# Uso: powershell -NoProfile -ExecutionPolicy Bypass -File build\app-linux.ps1 [-Autoprueba]
param([switch]$Autoprueba)
$ErrorActionPreference = 'Stop'
$Raiz = Split-Path -Parent $PSScriptRoot
$Version = ([xml](Get-Content -LiteralPath (Join-Path $Raiz 'Directory.Build.props') -Raw)).Project.PropertyGroup.Version
docker build -q -t beamline-linux (Join-Path $PSScriptRoot 'linux') | Out-Null
if ($LASTEXITCODE) { throw 'No se pudo construir la imagen beamline-linux.' }
$dist = Join-Path $Raiz 'dist'
New-Item -ItemType Directory -Force $dist | Out-Null
$args = @('run', '--rm', '-v', "${Raiz}:/src:ro", '-v', "${dist}:/out", '-v', 'beamline-nuget:/root/.nuget/packages',
    '-v', 'beamline-cargo:/usr/local/cargo/registry', '-v', 'beamline-target:/target', 'beamline-linux',
    'sh', '/src/build/linux/app-linux.sh', $Version)
if ($Autoprueba) { $args += '--autoprueba' }
& docker @args
exit $LASTEXITCODE
