# Simula varios PC con contenedores Linux en una red de Docker y mide el núcleo real (red\examples\medir.rs).
# Cada contenedor tiene su propia pila de red; tc netem pone retraso, pérdidas y ancho de banda a la salida de cada uno.
# Uso: powershell -NoProfile -ExecutionPolicy Bypass -File build\medir\medir.ps1 [-Escenarios A,B,...] [-MiB 1024]
# Antes: imagen «beamline-medir» (Dockerfile de aquí) y «medir» compilado en el volumen beamline-target (ver docs\medir-entre-pc.md).
param([string[]]$Escenarios = @('A', 'B', 'C', 'D', 'E', 'F', 'G'), [int]$MiB = 1024)
$ErrorActionPreference = 'Stop'
$Red = 'beamline-medir'
$Bin = '/target/release/examples/medir'

function Limpiar {
    docker rm -f emisor r1 r2 r3 rele 2>$null | Out-Null
}

# $netem: '' sin red simulada, o los parámetros de «tc qdisc add dev eth0 root netem …».
function Contenedor([string]$nombre, [string]$netem, [string]$orden) {
    $tc = if ($netem) { "tc qdisc add dev eth0 root netem $netem && " } else { '' }
    docker run -d --name $nombre --network $Red --cap-add NET_ADMIN -e "BEAMLINE_CC=$env:BEAMLINE_CC" -v beamline-target:/target:ro -v beamline-comun:/comun beamline-medir sh -c "$tc$orden" | Out-Null
}

function Escenario([string]$id, [string]$titulo, [int]$receptores, [string]$netEmisor, [string]$netReceptor, [string]$opciones = '', [string]$netRele = $null) {
    Write-Host "`n== $id. $titulo" -ForegroundColor Cyan
    Limpiar
    if ($null -ne $netRele) {
        Contenedor 'rele' $netRele 'iroh-relay --dev'
        Start-Sleep 3
    }
    docker run --rm -v beamline-comun:/comun beamline-medir sh -c "rm -f /comun/$id-*.txt" | Out-Null
    $billetes = @()
    for ($k = 1; $k -le $receptores; $k++) {
        Contenedor "r$k" $netReceptor "$Bin recibir /tmp/r /comun/$id-r$k.txt $opciones"
        $billetes += "/comun/$id-r$k.txt"
    }
    Contenedor 'emisor' $netEmisor "$Bin enviar /comun/grande.bin $($billetes -join ' ') $opciones"
    docker wait emisor | Out-Null
    $salida = docker logs emisor 2>&1
    $salida | Where-Object { $_ -match 'RESULTADO|ENVIO' } | ForEach-Object { Write-Host "  $_" }
    for ($k = 1; $k -le $receptores; $k++) { docker logs "r$k" 2>&1 | Where-Object { $_ -match 'RECIBIDO|RECEPTOR' } | ForEach-Object { Write-Host "  r$k $_" } }
    Limpiar
}

docker network create $Red 2>$null | Out-Null
docker run --rm -v beamline-comun:/comun beamline-medir sh -c "test -s /comun/grande.bin && [ `$(stat -c %s /comun/grande.bin) -eq $($MiB * 1048576) ] || head -c $($MiB * 1048576) /dev/urandom > /comun/grande.bin" | Out-Null

$lan = 'delay 0.25ms rate 1gbit'
$internet = 'delay 15ms 2ms loss 0.1% rate 100mbit'
foreach ($e in $Escenarios) {
    switch ($e) {
        'A' { Escenario A 'Misma máquina virtual, sin red simulada (techo de CPU)' 1 '' '' }
        'B' { Escenario B 'Red local gigabit (0,5 ms de ida y vuelta)' 1 $lan $lan }
        'C' { Escenario C 'Internet: 60 ms de ida y vuelta, 0,1 % de pérdidas, 100 Mbit/s' 1 $internet $internet }
        'D' { Escenario D 'Internet por un relé propio, solo relé (relé a 5 ms)' 1 $internet $internet '--rele http://rele:3340 --solo-rele' 'delay 5ms rate 1gbit' }
        'E' { Escenario E 'Uno a uno con el emisor limitado a 100 Mbit/s de subida' 1 'delay 2ms rate 100mbit' $lan }
        'F' { Escenario F 'A tres a la vez, emisor limitado a 100 Mbit/s de subida (reparto)' 3 'delay 2ms rate 100mbit' $lan }
        'H' { Escenario H 'Internet sin pérdidas: 60 ms de ida y vuelta, 100 Mbit/s' 1 'delay 15ms rate 100mbit' 'delay 15ms rate 100mbit' }
        'I' { Escenario I 'Solo retraso: 60 ms de ida y vuelta, sin límite ni pérdidas' 1 'delay 15ms' 'delay 15ms' }
        'J' { Escenario J 'Solo pérdidas: 0,1 %, sin retraso ni límite' 1 'loss 0.1%' 'loss 0.1%' }
        'G' { Escenario G 'Red local con el relé de n0 (¿salta sin-directa por error?)' 1 $lan $lan '--n0' }
    }
}
docker network rm $Red 2>$null | Out-Null
