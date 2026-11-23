# Compila beamline-red (release) y publica la app (WinUI 3, Native AOT) en dist\BeamLine.
# Uso: powershell -NoProfile -ExecutionPolicy Bypass -File build\app.ps1 [-Autoprueba]
#   -Autoprueba  abre dist\BeamLine\BeamLine.exe --autoprueba (ventana fuera de pantalla): envía y recibe con un
#                equipo sin ventana, deja dist\autoprueba.txt y sale con 0 si todo fue bien.
param([switch]$Autoprueba)
$ErrorActionPreference = 'Stop'
$Raiz = Split-Path -Parent $PSScriptRoot
. (Join-Path $PSScriptRoot 'entorno.ps1')
# Native AOT necesita link.exe; vswhere tiene que estar en el PATH para encontrarlo.
$vs = Join-Path ${env:ProgramFiles(x86)} 'Microsoft Visual Studio\Installer'
if (Test-Path -LiteralPath $vs) { $env:PATH = "$env:PATH;$vs" }

Push-Location (Join-Path $Raiz 'red')
try { cargo build --release --bin beamline-red; if ($LASTEXITCODE) { throw 'cargo build falló' } } finally { Pop-Location }

$Salida = Join-Path $Raiz 'dist\BeamLine'
if (Test-Path -LiteralPath $Salida) { Remove-Item -LiteralPath $Salida -Recurse -Force }
dotnet publish (Join-Path $Raiz 'src\BeamLine.App\BeamLine.App.csproj') -c Release -o $Salida --nologo
if ($LASTEXITCODE) { throw 'dotnet publish falló' }
Write-Host "Publicado en $Salida ($([math]::Round((Get-ChildItem $Salida -Recurse | Measure-Object Length -Sum).Sum / 1MB)) MB)"

if ($Autoprueba) {
    $txt = Join-Path $Raiz 'dist\autoprueba.txt'
    Remove-Item -LiteralPath $txt -ErrorAction SilentlyContinue
    $p = Start-Process -FilePath (Join-Path $Salida 'BeamLine.exe') -ArgumentList '--autoprueba', "`"$txt`"" -PassThru
    if (-not $p.WaitForExit(120000)) { $p.Kill(); throw 'La autoprueba no terminó en 2 minutos' }
    Get-Content -LiteralPath $txt
    exit $p.ExitCode
}
