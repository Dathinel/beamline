# Medidas de la fase 0

## En un solo PC (22-nov)

Seguí el montaje que usa Rodaje: dos procesos con almacén e identidad distintos. Uno envía e imprime un billete, y el otro lo usa para descargar. Para la latencia usé el relé local de Sala (`dathinel-red --rele-prueba --latencia-ms N`, compilado con `--features prueba`). Todas las copias llegaron idénticas a las originales (`cmp`).

| Caso | Tamaño | MiB/s | Mbit/s |
|---|---|---|---|
| Un proceso, `local`: corta a la mitad y reanuda | 5 GiB | 109-125 | 916-1050 |
| Dos procesos, conexión directa (con el PC sin otra carga) | 2 GiB | 133,8 | 1122 |
| Dos procesos, directa, con una compilación en paralelo | 2 GiB | 55,9 | 469 |
| Solo relé, relé local sin retraso | 512 MiB | 60,8 | 510 |
| Solo relé, relé local con 30 ms por tramo | 512 MiB | 6,7 | 56 |
| Solo relé, **relé público de n0** (Virginia) | 16 MiB | 0,044 | 0,36 |

Qué significa:
- En conexión directa el límite lo pone la CPU (QUIC, BLAKE3 y disco), no iroh: pasa de 1 Gbit/s.
- Pasar por un relé cuesta la mitad aunque esté en el mismo PC.
- Con retraso, la velocidad cae a unos 56 Mbit/s. Es la cifra a esperar con un relé propio lejano, y mejora si está cerca de los dos equipos. El relé de pruebas retrasa por TCP, así que es una aproximación.
- **El relé de n0 se queda en unos 45 KiB/s**: 16 MiB tardaron 6 minutos. Confirma lo que dice Rodaje: por n0 solo pasan mensajes, nunca archivos. Los envíos por internet que no logren conexión directa necesitan un relé propio.

Faltan dos casos reales: los dos PC en la misma red, y uno de ellos en otra red (por ejemplo, el móvil compartiendo datos).

## Dos y más PC simulados (23-nov)

Cada «PC» es un contenedor Linux de Docker Desktop con su propia pila de red, en una misma red de Docker. `tc netem` pone retraso, pérdidas y ancho de banda a la salida de cada uno. Se mide el núcleo real (`red/examples/medir.rs`, sobre `Equipo`) compilado para Linux, con 1 GiB aleatorio. El tiempo incluye calcular las huellas BLAKE3 en el que envía. Todas las copias llegaron completas y verificadas.

```powershell
docker build -t beamline-medir build\medir
docker run --rm -v "<BeamLine>:/src:ro" -v beamline-target:/target -v beamline-cargo:/usr/local/cargo/registry -e CARGO_TARGET_DIR=/target beamline-medir sh -c "cd /src/red && cargo build --release --example medir"
powershell -File build\medir\medir.ps1 -Escenarios A,B,C,D,E,F,G,H,I,J
```

| Caso | Red simulada (por equipo) | MiB/s | Notas |
|---|---|---|---|
| A. Sin red simulada | — | 62,7 | Techo de la máquina virtual de Docker (16 CPU compartidas) |
| B. Red local gigabit | 0,25 ms, 1 Gbit/s | 82,1 | |
| G. Red local con el relé de n0 | igual que B | 69,4 | No salta `sin-directa`: la ruta directa se encuentra enseguida |
| H. Internet sin pérdidas | 15 ms (60 ms de ida y vuelta), 100 Mbit/s | 10,8 | 90 % del enlace |
| I. Solo retraso | 15 ms | 33,3 | |
| J. Solo pérdidas | 0,1 % | 111,7 | |
| C. Internet con pérdidas | 15 ms ± 2, 0,1 %, 100 Mbit/s | 3,8–4,7 | Ver abajo |
| D. Igual que C, por un relé propio y solo relé | C más un relé a 5 ms | 4,5 | El relé no es el cuello de botella |
| E. Uno a uno, subida del emisor a 100 Mbit/s | emisor 2 ms y 100 Mbit/s; receptor como B | 10,8 | 94,5 s |
| **F. A tres a la vez, misma subida** | igual que E | 9,3 por destino | **Los tres en 109,6 s, con el emisor subiendo 1,00 veces el tamaño.** Dos receptores sirvieron 683 MiB cada uno a los demás |

Qué significa:
- **El reparto funciona fuera de loopback.** Enviar a tres tarda solo un 16 % más que a uno, con la misma subida. Sin reparto serían unos 280 s.
- **Con retraso y sin pérdidas se llena el enlace (H).** Lo que hunde C es la combinación de pérdidas aleatorias del 0,1 % con 60 ms: el control de congestión de QUIC recorta la ventana a cada pérdida. Probé el BBR3 de noq (`QuicTransportConfig::congestion_controller_factory`) y quedó igual (4,7 MiB/s), así que se quedó el de por omisión. Un 0,1 % de pérdidas aleatorias es pesimista para una conexión de casa; en wifi malo se notará.
- **Lo que no simula Docker:** el cortafuegos de Windows la primera vez que se abre la app, los NAT de verdad (los routers de casa y del operador) ni el wifi. Eso sigue en «Entre dos PC (a mano)».

## Entre dos PC (a mano)

**Aceptada sin medir (Daniel, 23-nov):** solo hay un PC; valen las medidas en Docker de arriba.

Se copia `poc\target\release\beamline-poc.exe` a los dos equipos y se prepara un archivo grande, de 5 a 10 GiB.

1. **En el que envía:** `beamline-poc.exe enviar D:\grande.bin D:\almacen-envio`. Imprime un billete. Se deja abierto.
2. **En el que recibe:** `beamline-poc.exe recibir <billete> D:\almacen-recibo D:\recibido.bin`. Imprime MiB/s.
3. **Reanudar:** se repite el paso 2 y se pulsa Ctrl+C a la mitad. Al volver a lanzarlo, «ya había» debe mostrar lo bajado antes.

Opciones: `--solo-rele` (no usa IP directas) y `--rele <url>` (un relé propio). Antes de repetir una medida se borra el almacén que recibe; si no, reanuda y no mide nada.
