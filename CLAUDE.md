# BeamLine (nombre provisional)

App de envío de archivos tipo LocalSend que también funciona por internet: iroh + iroh-blobs.
- **Privada y sin remoto hasta terminarla** (Daniel, 22-nov).
- Licencia MIT provisional: Daniel no la ha elegido.
- Nombre por decidir: Daniel propuso File Send, BeamLine Send y Skybridge Transfer. Antes de publicar hay que revisar la marca.

## Cómo está hecho

- `red\`: el proceso `beamline-red` en Rust. El núcleo está en `src\equipo.rs`. Habla el protocolo de módulos con la biblioteca de `Sala\red`, que entró en Sala con el PR #26, sin copiarla.
- `src\BeamLine.Nucleo\`: el cliente en C# (`Beam`). La identidad (DPAPI) viene de `Dathinel.Sala`.
- El contrato está en `docs\protocolo.md`.
- Entorno: `. build\entorno.ps1`. Por ahora usa el `.entorno` de la copia de trabajo de Sala.
- Pruebas:
  - Rust: `cd red; cargo test`. Hay 14: 5 unitarias y 9 de punta a punta, que incluyen las hostiles y mDNS.
  - C#: `dotnet test BeamLine.slnx`. Hay 5, y 4 de ellas usan dos procesos reales.
  - No hay CI porque no hay remoto.

## Estado

- Fase 0 (PoC y cifras) cerrada.
  - Cifras en `docs\medir-entre-pc.md`: directa 134 MiB/s, relé local 61, relé de n0 0,044.
  - **Por n0 no van archivos.**
  - Falta medir entre dos PC reales.
- Fase 1, núcleo sin interfaz (22-nov):
  - ofertas con aceptar o rechazar por archivo;
  - solo se sirve lo aceptado a quien lo aceptó;
  - reanuda tras un corte;
  - nombres seguros y límites;
  - mDNS con alias;
  - cliente en C# con eventos.
- La revisión de bloque (ChatGPT, solo lectura) está en curso.

Detalles que conviene saber:
- El `Progress` del downloader cuenta desde el principio del blob, incluido lo que ya había.
- Con `presets::Minimal`, las direcciones del billete se dan por `MemoryLookup` o por mDNS.
- El turno de envío se toma al aceptar, no al ofrecer: si no, cuatro ofertas sin respuesta bloquean la quinta.
- Exportar con `Copy` duplica en disco lo bajado hasta la próxima limpieza del almacén (shortcut anotado en `bajar`).

## Siguiente paso

1. Aplicar lo que encuentre la revisión de bloque.
2. Fase 2: interfaz WinUI 3 AOT. Lista de cercanos, arrastrar y soltar, diálogo de aceptar, progreso, QR del billete, favoritos y guardado sin preguntar.
