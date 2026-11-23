# Plan: app de envío de archivos entre equipos (nombre por decidir)

Estado: aprobado por Daniel el 22-nov (opción B, empezar por la fase 0). Nombre provisional: BeamLine.

## Qué es

Enviar archivos y carpetas de cualquier tamaño a uno o varios equipos, en la misma red o por internet, sin cuenta ni nube.
- **En la misma red:** los equipos aparecen solos, como en LocalSend.
- **Por internet:** se comparte un billete, como texto o QR.
- **Cifrado:** siempre de extremo a extremo (QUIC + TLS 1.3, con la clave de cada equipo).
- **Descargas:** se reanudan si se cortan y se verifican por trozos.

## Lo que se reutiliza

- **De Sala:** el modelo de proceso de red en Rust con un cliente en C# por named pipe (`Rodaje.Modules.Protocol`), la identidad guardada con DPAPI, el billete con secreto, el descubrimiento en la red local con mDNS de iroh y el relé configurable. Todo esto ya está probado en Windows, Linux y macOS.
- **De iroh-blobs** (n0, MIT o Apache-2.0, 0.105 sobre iroh 1.x): verificación incremental con BLAKE3, descargas por rangos, reanudación, carpetas y descarga desde varios equipos a la vez. Es lo que usa `sendme`.

## Decisión de arquitectura (recomendada, provisional)

| Opción | Ventajas | Desventajas |
|---|---|---|
| **A. Usar Sala tal cual** | No hay que escribir red nueva | Tope de 4 GiB por archivo, no reanuda (el `.parte` se borra), estrella con anfitrión, roles y orden que aquí sobran. Para cambiarlo hay que tocar Sala, que también usan Rodaje y Unicomp |
| **B. Proceso propio en Rust (iroh + iroh-blobs) y Sala como submódulo solo por el protocolo y las piezas de C#** ✔ | Reanuda, sin tope práctico, carpetas, varios destinos. No toca Sala | Un binario de Rust más. iroh-blobs sigue en 0.x y su API puede cambiar |
| C. Meter iroh-blobs en `dathinel-red` | También le serviría a Rodaje para los proxies (su fase 4) | Cambia una pieza compartida mientras dos sesiones trabajan en ella. Se puede hacer después, si B sale bien |

**Recomendada: B.** Si funciona, se propone C como mejora de Sala, y las dos apps compartirían un mismo transporte de archivos.

## Fases

**0. Prueba de concepto y cifras (sin interfaz).**
- Crear `.entorno\` con Rust fijado y .NET 10.
- Un binario mínimo que envíe un archivo de 10 GiB con iroh-blobs entre los dos PC de Daniel.
- **Terminado cuando:**
  - hay cifras de velocidad medidas por red local directa, por internet directa y por el relé de n0;
  - un corte a la mitad reanuda sin repetir lo ya recibido;
  - un archivo de más de 4 GiB llega con la huella correcta.
- Si iroh-blobs falla aquí, se vuelve a la opción A o C.

**1. Núcleo sin interfaz, con pruebas.**
- *Proceso de red:*
  - identidad;
  - mDNS (equipos cercanos con alias);
  - billetes de un uso con caducidad.
- *Ofrecer un envío:* manda una lista con nombres, tamaños y huellas, y el receptor acepta todo, acepta una parte o rechaza.
- *Transferencia:* descargar con avance, cancelar y reanudar; escribir en un temporal y moverlo al final; nombres seguros, sin `..` ni rutas absolutas.
- *Cliente C#* tipado, con sus eventos.
- *Límites:* tamaño máximo aceptado, espacio en disco y ofertas por minuto.
- **Terminado cuando:**
  - pasan las pruebas de envío, rechazo, cancelación, reanudación, carpeta con subcarpetas, archivo cambiado a mitad, ruta maliciosa y disco lleno;
  - pasan las pruebas hostiles al estilo de Sala (billete falso, oferta enorme);
  - la CI está en verde en Windows y Linux.

**2. Interfaz WinUI 3 (Native AOT, en `dist\`).**
- Equipos cercanos, arrastrar y soltar, y el diálogo de aceptar con vista previa de nombres y tamaños.
- Progreso con velocidad y tiempo restante, e historial.
- Billete con QR, botón de copiar y campo de pegar.
- Favoritos y «guardar sin preguntar» en tres modos: nunca, solo favoritos o siempre.
- Carpeta de destino.
- Aviso de equipo nuevo, con la huella en cuatro palabras como en Sala.
- **Terminado cuando:**
  - la autoprueba AOT está en verde;
  - Daniel recibió `pruebas-a-mano.md`;
  - hubo control de pantalla tras cada lote (regla del 20-nov).

**3. Varios destinos y relé propio.**
- Enviar a N equipos a la vez. Con iroh-blobs, los receptores también se sirven entre ellos y el emisor no sube N veces lo mismo.
- Preferencias:
  - relé propio (URL);
  - «solo relé» (ocultar la IP);
  - límite de velocidad.
- **Terminado cuando** un envío a 3 equipos funciona y el emisor sube menos de 2 veces el tamaño.

**4. Linux y publicación.**
- La misma interfaz con Uno, como Rodaje (D-066).
- Zip e instalador.
- Entrada firmada en el catálogo del Lanzador.
- `LICENCIAS.md` y `docs/legal.md`.
- **Terminado cuando** se ha probado entre Windows y Linux y Daniel ha dado el visto bueno para publicar.

**Fuera por ahora:** Android e iOS (es la gran ventaja de LocalSend; posible fase 5 junto a «Migrar apps a Android APK»), envío por enlace a un navegador y códigos cortos tipo croc (necesitan un servidor propio).

## Riesgos

- **Relé de n0:** es gratuito, sin cifras publicadas, y n0 lo declara «para desarrollo y pruebas». Cuando los routers no dejan conectar directo, los envíos grandes por internet necesitarán un relé propio, un VPS con los puertos 80 y 443 (unos 4-6 USD al mes, estimación sin verificar). Lo que cuesta de verdad es el tráfico.
- **Velocidades:** no hay cifras públicas fiables de iroh. Las da la fase 0.
- **iroh-blobs en 0.x:** se fija la versión exacta y se actualiza a propósito.
- **Nombre:** «Zipline» es la marca registrada de Zipline International (drones de reparto) y «Skyline» está muy usada. Antes de publicar, hay que buscarlo en `TRADEMARK.md`, como se hizo con Rodaje.
