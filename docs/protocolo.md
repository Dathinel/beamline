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
| `responder` | `oferta` (u64), `aceptar` (índices de `archivos`; vacío o ausente = rechazar), `destino` (carpeta absoluta) | — Comprueba el espacio libre (`espacio`) y empieza a bajar |
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
| `fin` | `id`, `ok`, `error`?, `rutas`? (solo en el que recibe) | Terminó un envío o una recepción. Hay un solo `fin` por id |

Los `error` de `fin` son estos:
- `rechazado`;
- `cancelado`;
- `sin-conexion`;
- `red`;
- `huella` (el archivo cambió mientras se enviaba);
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
2. El que recibe contesta `{"type":"respuesta","aceptados":[…]}`.
3. El que recibe baja cada archivo aceptado por iroh-blobs, desde el que envía.
4. El que recibe manda `{"type":"fin","ok":…,"error"?}` y se cierra el stream.

Quien envía cancela cerrando el stream, y el que recibe ve `oferta-retirada` o `fin` `cancelado`.

**Quién puede pedir datos.** El que envía solo sirve por iroh-blobs a la clave de un envío aceptado y en curso, y solo las huellas que esa clave aceptó. Lo controla el gancho `connected` y `get` de iroh-blobs. Nadie más puede bajar nada, aunque sepa una huella.

## 4. Archivos

- **El que envía** importa los archivos sin copiarlos (`TryReference`). Si un archivo cambia mientras se envía, el que recibe lo detecta al verificar BLAKE3 por trozos y el envío termina con `huella`.
- **El que recibe** baja a su almacén. Una descarga cortada se reanuda sola si vuelve a llegar la misma oferta, porque el almacén guarda lo que ya bajó.
- **Al terminar cada archivo:**
  1. se exporta a `destino/nombre.beamline-parte`;
  2. se renombra al nombre final;
  3. se borra del almacén.

  Si el nombre ya existe, se usa `nombre (2).ext`, y así sucesivamente: nunca se pisa un archivo.
- **Nombres válidos**, que se comprueban en los dos lados:
  - van en forma relativa, con `/` entre partes, de 1 a 1024 bytes en total y cada parte de 1 a 255;
  - no se admiten `.`, `..`, raíz, unidad, `:`, `\`, caracteres de control ni `<>"|?*`;
  - no se admiten nombres reservados de Windows (`CON`, `NUL`, `COM1`…, también con extensión) ni partes terminadas en punto o espacio.

## 5. Límites

| Qué | Límite | Al pasarse |
|---|---|---|
| Archivos por oferta | 4096 | `demasiado-grande` |
| Tamaño de una oferta | `tamanoMax` del que recibe | Se rechaza sola (`fin` `rechazado` en el que envía) |
| Ofertas pendientes de respuesta | 4 en total | `ocupado` |
| Ofertas por minuto y por clave | 10 | Se cierra sin avisar a la app |
| Transferencias a la vez | 4 por lado | Las demás esperan turno |
| Tiempo sin respuesta | 10 minutos | Se retira la oferta |

## 6. Billete

Un billete es `beam1` seguido de base64url del JSON de la `EndpointAddr`, que lleva la clave, el relé y las direcciones. Sirve para enviar a un equipo que no está en la red local. No lleva ningún secreto: quien recibe acepta cada oferta, y los límites del §5 evitan que llenen el disco o la pantalla de ofertas.
