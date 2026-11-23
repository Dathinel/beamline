#!/bin/sh
# Dentro del contenedor de build/linux/Dockerfile (lo lanza build/app-linux.ps1). Compila beamline-red, publica la app
# de Linux con Native AOT, la empaqueta y, con --autoprueba, la corre bajo Xvfb.
set -eu
VERSION="$1"; shift
mkdir -p /work && cd /work
# Copia limpia: los obj de Linux no se mezclan con los de Windows de la carpeta montada.
tar -C /src --exclude='./.git' --exclude='*/bin' --exclude='*/obj' --exclude='./red/target' --exclude='./poc/target' \
    --exclude='./dist' --exclude='./.claude' --exclude='./Sala/.git' --exclude='./Sala/.entorno' -cf - . | tar -xf -
export CARGO_TARGET_DIR=/target/rust
(cd red && cargo build --release --bin beamline-red)
SALIDA=/out/BeamLine-linux
rm -rf "$SALIDA"
# Sin tuberías: un publish que falla tiene que parar el script (sh no tiene pipefail).
if ! dotnet publish src/BeamLine.App.Desktop/BeamLine.App.Desktop.csproj -c Release -o "$SALIDA" --nologo -p:Version="$VERSION" > /out/publicar-linux.log 2>&1; then
    tail -40 /out/publicar-linux.log
    exit 1
fi
grep -E "error|warning|->" /out/publicar-linux.log | grep -v "Uno0001" | tail -20 || true
# El archivo de bloqueo que genera la copia limpia, para guardarlo en el repositorio si cambia.
cp src/BeamLine.App.Desktop/packages.lock.json /out/packages.lock.linux.json
cp "$CARGO_TARGET_DIR/release/beamline-red" "$SALIDA/"
cp LICENSE LICENCIAS.md "$SALIDA/"
cp /out/AVISOS-TERCEROS.txt "$SALIDA/"
rm -f "$SALIDA"/*.pdb "$SALIDA"/*.dbg "$SALIDA"/BeamLine.Nucleo.xml "$SALIDA"/Dathinel.Sala.xml "$SALIDA"/Rodaje.Modules.Protocol.xml
tar -C /out -czf "/out/BeamLine-$VERSION-linux-x64.tar.gz" BeamLine-linux
du -sh "$SALIDA" "/out/BeamLine-$VERSION-linux-x64.tar.gz"
if [ "${1:-}" = "--autoprueba" ]; then
    rm -f /out/autoprueba-linux.txt
    rc=0
    timeout 180 xvfb-run -a -s "-screen 0 1280x800x24" "$SALIDA/beamline" --autoprueba /out/autoprueba-linux.txt || rc=$?
    echo "salida=$rc"
    cat /out/autoprueba-linux.txt 2>/dev/null || echo "(no dejó resultado)"
    exit $rc
fi
