# Deja dotnet y cargo en esta sesión de PowerShell: . build\entorno.ps1
# shortcut: usa el .entorno de la copia de trabajo de Sala (D:\...\Sala\.entorno); un preparar-entorno.ps1 propio llega con la CI, cuando haya remoto.
$Entorno = if ($env:BEAMLINE_ENTORNO) { $env:BEAMLINE_ENTORNO } else { Join-Path (Split-Path -Parent (Split-Path -Parent $PSScriptRoot)) 'Sala\.entorno' }
$S = [System.IO.Path]::PathSeparator
$env:DOTNET_ROOT = Join-Path $Entorno 'dotnet'
$env:NUGET_PACKAGES = Join-Path $Entorno 'nuget'
$env:DOTNET_CLI_HOME = $Entorno
$env:DOTNET_MULTILEVEL_LOOKUP = '0'
$env:DOTNET_CLI_TELEMETRY_OPTOUT = '1'
$env:DOTNET_NOLOGO = '1'
$env:RUSTUP_HOME = Join-Path $Entorno 'rustup'
$env:CARGO_HOME = Join-Path $Entorno 'cargo'
$env:PATH = $env:DOTNET_ROOT + $S + (Join-Path $env:CARGO_HOME 'bin') + $S + $env:PATH
