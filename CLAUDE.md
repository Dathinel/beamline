# BeamLine (nombre provisional)

App de envío de archivos tipo LocalSend que también funciona por internet: iroh + iroh-blobs.
- **Privada y sin remoto hasta terminarla** (Daniel, 22-nov).
- Licencia MIT provisional: Daniel no la ha elegido.
- Nombre por decidir: Daniel propuso File Send, BeamLine Send y Skybridge Transfer. Antes de publicar hay que revisar la marca.

## Cómo está hecho

- `red\`: el proceso `beamline-red` en Rust. El núcleo está en `src\equipo.rs`. Habla el protocolo de módulos con la biblioteca de `Sala\red`, que entró en Sala con el PR #26, sin copiarla.
- `src\BeamLine.Nucleo\`: el cliente en C# (`Beam`). La identidad (DPAPI) viene de `Dathinel.Sala`.
- `src\BeamLine.App\`: la interfaz (WinUI 3 sin empaquetar, Native AOT, con la receta de Unicomp). `build\app.ps1` compila `beamline-red` en release y publica en `dist\BeamLine`, con `beamline-red.exe` al lado.
  - `--perfil x` separa los datos para abrir dos ventanas en un PC.
  - `-Autoprueba` envía y recibe con la ventana fuera de pantalla.
  - Datos en `%LOCALAPPDATA%\BeamLine`; lo recibido va a `Descargas\BeamLine`.
- El contrato está en `docs\protocolo.md`.
- Entorno: `. build\entorno.ps1`. Por ahora usa el `.entorno` de la copia de trabajo de Sala.
- Pruebas:
  - Rust: `cd red; cargo test` (y `--features prueba` para la del relé local). Hay 39: 21 unitarias (`src\equipo\pruebas.rs` y el reparto de `grupo.rs`) y 18 de punta a punta (`tests\transferencias.rs` y `tests\grupo.rs`).
  - C#: `dotnet test BeamLine.slnx`. Hay 11, y 10 de ellas usan procesos reales.
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
- Revisión de bloque hecha por Gemini Flash el 22-nov (ChatGPT, Devin y Argon estaban sin cuota). Sus 7 hallazgos de lógica están corregidos y cada uno tiene su prueba:
  - marcos de más de 4 KiB;
  - carrera de permisos, resuelta con el marco `listo`;
  - inactividad del que envía;
  - `.beamline-parte` huérfana;
  - duplicados al reanudar;
  - `CONIN$` y `CONOUT$`;
  - el mapa de ritmo.
  
  Quedan dos propuestas de estructura sin aplicar: partir `atender_oferta` y usar un enum de errores.

Detalles que conviene saber:
- El `Progress` del downloader cuenta desde el principio del blob, incluido lo que ya había.
- Con `presets::Minimal`, las direcciones del billete se dan por `MemoryLookup` o por mDNS.
- Los turnos se toman al aceptar, no al ofrecer: si no, cuatro ofertas sin respuesta bloquean la quinta. El que envía da los permisos antes de mandar `listo`.
- Exportar con `Copy` duplica en disco lo bajado hasta la próxima limpieza del almacén (shortcut anotado en `bajar`).

- Revisión de bloque de las fases 1 y 2 hecha por ChatGPT (gpt-6-astra) el 23-nov: 24 hallazgos.
  - Unidos: los del núcleo (1-3, 5-9, 14, 19) en `ea35ca5` y los del cliente y la interfaz (4, 11-13, 15-18, 21) en `0b5a046`, cada uno con su prueba.
  - El 10 (por el relé de n0 no deben ir archivos) pasa a la fase 3, con el relé propio.
  - Las preguntas 22-24 son para Daniel; abajo, con la opción recomendada.
  - Ojo: exportar desde iroh-blobs sobre el temporal ya reservado. Copiarlo a mano era un 46 % más lento.

## Siguiente paso

1. Preguntas abiertas para Daniel, avanzando con la recomendada:
   - «Sin relé» deja usar billetes con IP pública (recomendado) o aísla solo en la LAN;
   - límites a los equipos de confianza (recomendado: los mismos que al resto, más quitar la confianza con el clic derecho);
   - historial que dure al cerrar la app: aparcado (se recomienda dejarlo para después de la fase 3);
   - billetes sin secreto ni caducidad (recomendado, porque cada oferta se acepta a mano) o de un uso;
   - lo bajado a medias se guarda 7 días para reanudar.
2. Fase 2 (23-nov), en código:
   - equipos cercanos y guardados;
   - arrastrar y soltar, y elegir archivos o una carpeta;
   - diálogo para aceptar por archivo;
   - progreso, cancelar y «Mostrar»;
   - «Guardar lo suyo sin preguntar»;
   - billete copiable y ajustes (nombre, carpeta, solo red local).

   La autoprueba AOT pasa.

   Faltan:
   - el repaso de Daniel por fluidez (`docs\pruebas-a-mano.md`). Claude ya lo probó con el ratón el 23-nov con dos ventanas: elegir carpeta y archivos, aceptar una parte, rechazar, cancelar con el diálogo abierto, sin preguntar, billete, ajustes y «Mostrar». Arregló tres cosas: eventos perdidos al abrir, el recuento de aceptados y el aviso en la barra de tareas.
   - el mDNS entre dos procesos del mismo PC: falla en un sentido, según cuál se abra primero; la lista se corrige sola al llegar una oferta. Entre dos PC hay que comprobarlo con la medida.
   - arrastrar desde el Explorador: el control de pantalla no puede hacerlo; lo prueba Daniel.
   - el QR del billete, que no tiene sentido sin una app de móvil y se aparca.
3. Fase 3 (23-nov), en código (§7 de `docs\protocolo.md`):
   - enviar a varios, hasta 8. Cada receptor baja su trozo del emisor y el resto de los otros. Con 3 receptores y 24 MiB, el emisor sube 1,0 veces el tamaño;
   - con el relé de n0, los archivos solo van por conexión directa; si no la hay, fin `sin-directa` (hallazgo 10);
   - relé propio, «solo relé» (que exige un relé propio) y límite de subida en Ajustes;
   - autoprueba AOT con un paso de envío a dos.

   Claude lo probó con el ratón con tres ventanas: envío a dos a la vez, la huella en el diálogo, sin duplicados y la validación de la URL del relé.

   Faltan:
   - la revisión de bloque, en marcha con Gemini Argon;
   - medir de verdad entre PC, con un relé propio en un VPS y con el reparto fuera de loopback.
4. Fase 4: Linux con Uno, publicación y la entrada del Lanzador.
