# BeamLine (nombre provisional)

Envía archivos y carpetas de cualquier tamaño entre equipos, en la misma red o por internet, sin cuenta ni nube. Usa iroh (QUIC con TLS 1.3, cifrado de extremo a extremo) e iroh-blobs (verificación BLAKE3 por trozos y reanudación).

Estado: fase 0, prueba de concepto. El plan está en `docs/plan.md`.

## Prueba de concepto

```powershell
. build\entorno.ps1
cargo build --release --manifest-path poc\Cargo.toml
poc\target\release\beamline-poc.exe local <archivo> <carpeta>          # corta a la mitad, reanuda y comprueba la huella
poc\target\release\beamline-poc.exe enviar <archivo> <almacén>         # imprime un billete
poc\target\release\beamline-poc.exe recibir <billete> <almacén> <destino>
```

En este PC (22-nov), un archivo de 5 GiB por loopback: la primera mitad a 125 MiB/s; cortada y reanudada, baja solo los 2,5 GiB que faltaban, a 109 MiB/s, y la huella coincide.
