# Protocolo de BeamLine

Hay dos procesos en cada equipo:
- la app (C#);
- `beamline-red` (Rust, iroh + iroh-blobs).

Entre los dos se usa el protocolo de módulos v1 de Rodaje y Sala: named pipe, JSON con marco, `--rodaje-pipe`. Este proceso lo reutiliza como biblioteca de `Sala/red` (`dathinel_red::modulo`). Un proceso es un equipo.

## 1. Llamadas de la app al proceso

| Función | Parámetros | Resultado |
|---|---|---|
| `abrir` | `identidad` (64 hex, clave **privada**), `alias` (1–64 caracteres), `almacen` (carpeta absoluta del almacén de iroh-blobs), `red` `{rele: "n0"\|"ninguno"\|URL https, soloRele, mdns}`, `tamanoMax`? (bytes que acepta por oferta; por defecto 1 TiB) | `clave` (pública, 64 hex), `billete` |
| `ofrecer` | `a`: clave de un equipo cercano o billete; `archivos`: lista (1–4096) de `{ruta, nombre}`, donde `ruta` es el archivo local absoluto y `nombre` la ruta relativa que verá el otro con `/` (la app expande las carpetas) | `envio` (u64). Saca las huellas, conecta y ofrece en segundo plano; el resto llega por eventos |
| `responder` | `oferta` (u64), `aceptar` (índices de `archivos`; vacío o ausente = rechazar), `destino` (carpeta absoluta) | — Comprueba el espacio libre (`espacio`; el doble si el almacén está en el mismo volumen que `destino`, porque hasta la próxima limpieza hay dos copias) y empieza a bajar. Descuenta también el espacio comprometido por las otras aceptaciones |
| `cancelar` | `id` (un envío o una oferta recibida) | — Si la oferta aún no tiene respuesta, la rechaza |
| `recibir` | `esperaMs` (hasta 30 000) | `eventos`: lista. Llamada larga y cancelable |
| `estado` | — | `clave`, `alias`, `billete`, `cercanos` `[{clave, alias}]`, `releConectado` |
| `cerrar` | — | — Cancela todo y cierra |

Errores de llamada: los códigos del protocolo (`invalid-parameters`, `busy`, …) más estos:
- `no-abierto` y `ya-abierto`;
- `billete-invalido`;
- `nombre-invalido`, que lleva el índice;
- `demasiado-grande`;
- `espacio`;
- `desconocido` (una oferta o un envío que no existe);
- `disco`.

## 2. Eventos (por `recibir`)

| Evento | Campos | Cuándo |
|---|---|---|
| `cercano` | `clave`, `alias`, `visto` (sí o no) | Un equipo aparece o desaparece por mDNS |
| `oferta` | `oferta`, `de` (clave), `alias`, `archivos` `[{nombre, tamano}]`, `total` | Alguien quiere enviar. La app pregunta, o responde sola si el equipo es favorito |
| `oferta-retirada` | `oferta` | Quien envía canceló o se fue antes de la respuesta |
| `respuesta` | `envio`, `aceptados` (índices) | El otro respondió; vacío significa que rechazó |
| `avance` | `id`, `bytes`, `total` | Como mucho 4 por segundo y por transferencia, en los dos lados |
| `fin` | `id`, `ok`, `error`?, `rutas`? (solo en el que recibe) | Terminó un envío o una recepción aceptada. Hay un solo `fin` por id. Una oferta que el que recibe rechaza o deja caducar no da `fin` en su lado: ya lo sabe |

Los `error` de `fin` son estos:
- `rechazado`;
- `cancelado`;
- `sin-conexion`;
- `red`;
- `huella` (el archivo cambió mientras se enviaba; lo dice el que envía, el que recibe ve `red`);
- `disco`;
- `ocupado` (el otro tiene demasiadas ofertas pendientes);
- `version`.

## 3. Entre equipos

Cada equipo es un endpoint de iroh. Hay dos ALPN:
- `beamline/1`: las ofertas;
- el de iroh-blobs: los datos.

Todo viaja cifrado de extremo a extremo con QUIC y TLS 1.3, y la identidad de cada equipo es la clave de su conexión.

Una oferta va en un stream bidireccional con marcos JSON (`marco`, tope de 960 KiB):

1. El que envía manda `{"type":"oferta","version":1,"alias":…,"archivos":[{"nombre","tamano","hash"}]}`.
2. Si acepta, el que recibe espera su turno de recepción y contesta `{"type":"respuesta","aceptados":[…]}`. Si rechaza, contesta con la lista vacía.
3. El que envía espera su turno de envío, da permiso a esas huellas y manda `{"type":"listo"}`. Antes de `listo` el otro no pide nada: si lo pidiera, se lo negarían.
4. El que recibe baja cada archivo aceptado por iroh-blobs, desde el que envía.
5. El que recibe manda `{"type":"fin","ok":…,"error"?}` y se cierra el stream.

Si el que envía no sirve ni un byte durante 10 minutos, da el envío por perdido (`red`) y suelta su turno. El receptor espera `listo` como máximo 10 minutos desde su respuesta; ese plazo incluye la espera del turno del emisor. También corta una descarga tras 10 minutos sin aumentar los bytes recibidos. En ambos casos emite `fin` con `red` y libera su turno y el espacio comprometido. Mientras espera la respuesta no ocupa turno, y si el otro desaparece, lo detecta el tiempo de inactividad de QUIC.

Quien envía cancela cerrando el stream, y el que recibe ve `oferta-retirada` o `fin` `cancelado`.

**Quién puede pedir datos.** El que envía solo sirve por iroh-blobs a la clave de un envío aceptado y en curso, y solo las huellas que esa clave aceptó. Lo controla el gancho `connected` y `get` de iroh-blobs. Cada pareja (par, huella) puede pertenecer a varios envíos: revocar uno conserva los demás y el progreso se registra en cada envío vigente. Nadie más puede bajar nada, aunque sepa una huella.

## 4. Archivos

- **El que envía** importa los archivos sin copiarlos (`TryReference`). La limpieza del almacén nunca borra esos archivos: solo borra los suyos internos. Si un archivo cambia mientras se envía, el que recibe lo detecta al verificar BLAKE3 por trozos y corta la descarga. El que envía ve que el archivo cambió (por tamaño o fecha) y termina con `huella`.
- **El que recibe** baja a su almacén con una reserva por archivo (`rx/<huella>/<segundos>`). Si se corta la red, la reserva queda y una oferta repetida reanuda desde lo bajado. Al completar o cancelar se borran todas las etiquetas `rx/<huella>/…`, incluidas las de intentos anteriores. Cada descarga activa mantiene además una protección temporal propia para no interferir con otras descargas del mismo blob. Las reservas de más de 7 días se borran al abrir y periódicamente, al ritmo de limpieza del almacén (por defecto cinco minutos). Si el progreso supera el tamaño ofrecido, se corta inmediatamente con `red` y se borran todas las reservas de esa huella.
- **Lo que ya llegó no se repite.** Si `destino/nombre` o uno de sus candidatos `nombre (2).ext`, `nombre (3).ext`, etc., ya existe con el mismo tamaño y la misma huella BLAKE3, ese archivo no se baja y su ruta va en `rutas`. Se buscan también candidatos posteriores a un hueco en la numeración. Pasa, por ejemplo, con una carpeta repetida tras un corte.
- **Al terminar cada archivo:**
  1. se copia a un temporal exclusivo `.beamline-<id>-<secuencia>.beamline-parte`, creado con `create_new` en la carpeta final;
  2. se publica sin reemplazar el destino (movimiento exclusivo en Windows; enlace duro y borrado del temporal en otros sistemas); si el nombre apareció entre tanto, se vuelve a buscar uno libre;
  3. se sueltan todas las reservas de esa huella y la próxima limpieza lo borra del almacén.

  Fuera de Windows el volumen de destino debe admitir enlaces duros; si no los admite, la publicación falla con `disco`.

  Si algo falla antes del paso 2, solo se borra el temporal creado por esa recepción: también cuando se corta la conexión.

  Si el nombre ya existe, se usa `nombre (2).ext`, y así sucesivamente: nunca se pisa un archivo.
- **Rutas físicas:** antes de crear carpetas o escribir se rechazan enlaces simbólicos y reparse points desde el destino hacia abajo, y se comprueba que el padre canónico permanece dentro del destino canónico. Fallan con `disco`.
  - shortcut: se acepta la ventana TOCTOU si un atacante local sustituye componentes entre comprobación y escritura.
- **Espacio comprometido:** cada aceptación reserva globalmente el doble de los bytes seleccionados y lo descuenta del espacio disponible al admitir las siguientes. La reserva se conserva hasta terminar, fallar o cancelar; es conservadora entre volúmenes y mientras se escriben los datos. La aceptación y la retirada/caducidad se coordinan: si aceptar devuelve éxito, esa recepción siempre produce un `fin`.
- **Nombres válidos**, que se comprueban en los dos lados:
  - van en forma relativa, con `/` entre partes, de 1 a 1024 bytes en total y cada parte de 1 a 255;
  - no se admiten `.`, `..`, raíz, unidad, `:`, `\`, caracteres de control ni `<>"|?*`;
  - no se admiten nombres reservados de Windows (`CON`, `NUL`, `COM1`…, `CONIN$`, `CONOUT$`, `COM¹`…, también con extensión) ni partes terminadas en punto o espacio.

## 5. Límites

| Qué | Límite | Al pasarse |
|---|---|---|
| Archivos por oferta | 4096 | `demasiado-grande` |
| Tamaño de una oferta | `tamanoMax` del que recibe | Se rechaza sola (`fin` `rechazado` en el que envía) |
| Ofertas pendientes, incluidas las que aún leen su cuerpo | 4 en total y 2 por par; se comprueba antes de leer el cuerpo | `ocupado` |
| Ofertas por minuto y por clave | 10 | Se cierra sin avisar a la app |
| Transferencias a la vez | 4 por lado | Las demás esperan turno (§3) |
| Envío o recepción sin avanzar; receptor esperando `listo` | 10 minutos | `fin` `red` y liberación del turno |
| Tiempo sin respuesta | 10 minutos | El que recibe retira la oferta |

**Cola de eventos:** como máximo 4096 eventos, contando los espacios reservados para operaciones en curso. Antes de admitir una oferta entrante o un envío local se reservan dos posiciones para sus eventos obligatorios. Si no caben, se devuelve `ocupado` al par o `busy` a `ofrecer`, hasta que la app vacíe la cola con `recibir`. Con la capacidad agotada se descartan nuevos `avance` y `cercano` (el estado de cercanos sigue disponible por `estado`). Nunca se descartan `fin`, `oferta`, `respuesta` ni `oferta-retirada`.

## 6. Billete

Un billete es `beam1` seguido de base64url del JSON de la `EndpointAddr`, que lleva la clave, el relé y las direcciones. Sirve para enviar a un equipo que no está en la red local. No lleva ningún secreto: quien recibe acepta cada oferta, y los límites del §5 evitan que llenen el disco o la pantalla de ofertas.
