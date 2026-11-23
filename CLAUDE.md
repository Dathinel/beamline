# BeamLine (nombre provisional)

App de envío de archivos tipo LocalSend que también funciona por internet: iroh + iroh-blobs. **Privada y sin remoto hasta terminarla** (Daniel, 22-nov). Licencia MIT provisional, porque Daniel no la eligió. El nombre está por decidir: Daniel propuso File Send, BeamLine Send y Skybridge Transfer; antes de publicar hay que revisar la marca.

## Estado

- Plan aprobado el 22-nov (`docs/plan.md`). Opción B: proceso propio en Rust con iroh-blobs; Sala entra como submódulo solo por el protocolo de módulos y las piezas de C#. No se toca Sala.
- **Fase 0:**
  - La PoC (`poc\`, iroh 1.4.1 igual que Sala, iroh-blobs 0.105) pasa la autoprueba `local` con 5 GiB: corta, reanuda, baja solo lo que falta y la huella coincide.
  - Falta medir entre los dos PC de Daniel (`docs/medir-entre-pc.md`) y añadir `--solo-rele`.
- Ojo:
  - el `Progress` del downloader cuenta desde el inicio del blob, incluido lo que ya había;
  - con `presets::Minimal` hay que dar las direcciones del billete por `MemoryLookup`, o la descarga falla con «Unable to download».

## Siguiente paso

Las cifras entre los PC, y después la fase 1: núcleo con ofertas, aceptar o rechazar, carpetas, mDNS, límites y pruebas hostiles.
