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

## Entre dos PC (a mano)

Se copia `poc\target\release\beamline-poc.exe` a los dos equipos y se prepara un archivo grande, de 5 a 10 GiB.

1. **En el que envía:** `beamline-poc.exe enviar D:\grande.bin D:\almacen-envio`. Imprime un billete. Se deja abierto.
2. **En el que recibe:** `beamline-poc.exe recibir <billete> D:\almacen-recibo D:\recibido.bin`. Imprime MiB/s.
3. **Reanudar:** se repite el paso 2 y se pulsa Ctrl+C a la mitad. Al volver a lanzarlo, «ya había» debe mostrar lo bajado antes.

Opciones: `--solo-rele` (no usa IP directas) y `--rele <url>` (un relé propio). Antes de repetir una medida se borra el almacén que recibe; si no, reanuda y no mide nada.
