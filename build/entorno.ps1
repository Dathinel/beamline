# Deja cargo en esta sesión de PowerShell: . build\entorno.ps1
# shortcut: en la fase 0 usa el .entorno de Sala (misma cadena de Rust); en la fase 1 entra Sala como submódulo y su preparar-entorno.ps1.
$Entorno = if ($env:BEAMLINE_ENTORNO) { $env:BEAMLINE_ENTORNO } else { Join-Path (Split-Path -Parent (Split-Path -Parent $PSScriptRoot)) 'Sala\.entorno' }
$env:RUSTUP_HOME = Join-Path $Entorno 'rustup'
$env:CARGO_HOME = Join-Path $Entorno 'cargo'
$env:PATH = (Join-Path $env:CARGO_HOME 'bin') + [System.IO.Path]::PathSeparator + $env:PATH
