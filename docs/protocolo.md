# Protocolo de BeamLine

Hay dos procesos en cada equipo:
- la app (C#);
- `beamline-red` (Rust, iroh + iroh-blobs).

Entre los dos se usa el protocolo de módulos v1 de Rodaje y Sala: named pipe, JSON con marco, `--rodaje-pipe`. Este proceso lo reutiliza como biblioteca de `Sala/red` (`dathinel_red::modulo`). Un proceso es un equipo.

## 1. Llamadas de la app al proceso

| Función | Parámetros | Resultado |
|---|---|---|
| `abrir` | `identidad` (64 hex, clave **privada**), `alias` (1–64 caracteres), `almacen` (carpeta absoluta del almacén de iroh-blobs), `red` `{rele: "n0"\|"ninguno"\|URL https, soloRele, mdns, limiteSubida?}` (`soloRele` solo con un relé propio; `limiteSubida` en bytes por segundo), `tamanoMax`? (bytes que acepta por oferta; por defecto 1 TiB) | `clave` (pública, 64 hex), `billete` |
| `ofrecer` | `a`: clave de un equipo cercano o billete, o una lista de 1 a 8 de ellos (§7); `archivos`: lista (1–4096) de `{ruta, nombre}`, donde `ruta` es el archivo local absoluto y `nombre` la ruta relativa que verá el otro con `/` (la app expande las carpetas) | `envio` (u64), o `envios` (uno por destino, en el orden de `a`) si `a` es una lista. Saca las huellas una vez, conecta y ofrece en segundo plano; el resto llega por eventos |
| `responder` | `oferta` (u64), `aceptar` (índices de `archivos`; vacío o ausente = rechazar), `destino` (carpeta absoluta) | — Comprueba el espacio libre (`espacio`; el doble si el almacén está en el mismo volumen que `destino`, porque hasta la próxima limpieza hay dos copias) y empieza a bajar. Descuenta también el espacio comprometido por las otras aceptaciones |
| `cancelar` | `id` (un envío o una oferta recibida) | — Si la oferta aún no tiene respuesta, la rechaza |
| `recibir` | `esperaMs` (hasta 30 000) | `eventos`: lista. Llamada larga y cancelable |
| `estado` | — | `clave`, `alias`, `billete`, `cercanos` `[{clave, alias}]`, `releConectado`, `subidos` (bytes de datos servidos desde que se abrió) |
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
- `version`;
- `sin-directa` (con el relé de n0 no se consiguió conexión directa en 20 s: por ese relé no van archivos, §7).

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

## 7. Varios destinos, relé y límite de subida (fase 3)

**Envío a varios.** `ofrecer` con una lista en `a` abre un envío por destino, con los archivos importados una sola vez. Cancelar todos los envíos para también la importación. Si son dos o más, cada oferta lleva `grupo`: los billetes de todos los destinos, en el mismo orden para todos.
- El grupo ocupa **un solo turno** de envío: lo toma el primero que acepta y se suelta al acabar el último.
- El que recibe rechaza la oferta (marco inválido) si `grupo` no tiene de 2 a 8 billetes válidos y distintos, si no está él exactamente una vez o si está el que envía. Las direcciones del grupo solo se apuntan si acepta, y solo las de equipos que no conocía; se olvidan al dejar de compartir.
- Cada archivo de 4 MiB o más se parte en tantos trozos como receptores, alineados a bloques de 16 KiB; el último acaba en el último KiB del archivo. El receptor `k` baja el trozo `k` del emisor. Cada uno de los demás trozos se lo pregunta con `observe` al receptor que le toca y, en cuanto lo tiene entero, se lo pide a él. Se pasa al emisor si:
  - ese receptor se niega (no aceptó) o se va;
  - no tiene nada en 10 s, por ejemplo porque se saltó el archivo al tenerlo ya;
  - deja de avanzar 30 s;
  - sirve más despacio de 512 KiB/s (tope por trozo);
  - con el relé de n0, no se llega a él directo en 5 s.

  Al final se pide al emisor lo que falte, que normalmente es nada.
- **Qué sirve un receptor a los demás:** desde `listo`, solo **su trozo** (`get` con rangos dentro de él y `observe`) de las huellas que aceptó y que no tenía ya enteras antes. Así, quien sepa una huella no puede sacarla metiéndose en un grupo. Lo sigue haciendo 2 minutos después de terminar, con los blobs protegidos de la limpieza; si cancela, deja de servir en el acto.
- **Avance:** en un grupo, el que recibe le manda al emisor `{"type":"avance","bytes":n}` cada segundo por `beamline/1`. El emisor lo usa para su barra y para la inactividad, porque lo que él sirve directamente es solo un trozo.
- Medido en la prueba de 3 receptores con 24 MiB: el emisor sube 1,0 veces el tamaño y los receptores se sirven el resto entre ellos. Con 24 MiB + 777 bytes, igual y sin esperas.

**Relé.** Con el relé de n0 (`rele: "n0"`), el que recibe espera hasta 20 s a que la conexión con el emisor vaya por una ruta directa antes de pedir datos. Si no la hay, termina con `sin-directa` y el emisor ve el mismo código, porque por ese relé los archivos iban a 0,04 MiB/s (fase 0). Con un relé propio (URL https; el esquema puede venir en mayúsculas) los datos pueden ir por él, y `soloRele` obliga a que vayan siempre por él. Eso oculta la IP, y por eso con `soloRele` tampoco se anuncia el equipo por mDNS.
- shortcut: la ruta se mira al empezar, en la conexión de la oferta y en la de cada par del grupo. Si a mitad la ruta directa se cae, iroh vuelve al relé y no se detecta; tampoco el caso de un emisor que use el relé de n0 frente a un receptor con relé propio.

**Límite de subida.** Con `limiteSubida`, cada bloque de 16 KiB que sirve este equipo espera su turno en un solo ritmo para todo lo que sirve; al cerrar, las esperas se sueltan. Sin límite, ese gancho ni se activa.
- shortcut: el ritmo es común, sin reparto justo entre pares.
