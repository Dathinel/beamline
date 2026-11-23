# Medir entre dos PC (fase 0)

Hace falta el ejecutable `poc\target\release\beamline-poc.exe` en los dos equipos (se copia tal cual) y un archivo grande, de 5 a 10 GiB, en el que envía.

1. **En el que envía:** `beamline-poc.exe enviar D:\grande.bin D:\almacen-envio`. Imprime un billete. Se deja abierto.
2. **En el que recibe:** `beamline-poc.exe recibir <billete> D:\almacen-recibo D:\recibido.bin`. Imprime MiB/s y Mbit/s.
3. **Reanudar:** se repite el paso 2 y se pulsa Ctrl+C a la mitad. Al volver a lanzarlo, la línea «ya había» debe mostrar lo bajado antes.

Se mide en tres casos y se apunta en la tabla:

| Caso | Cómo | MiB/s |
|---|---|---|
| Misma red, por cable o wifi | Los dos PC en casa | |
| Internet, conexión directa | Uno en otra red, por ejemplo el móvil compartiendo datos | |
| Solo por el relé de n0 | Pendiente: la PoC aún no tiene la opción `--solo-rele` | |

Antes de repetir una medida se borra la carpeta del almacén que recibe; si no, reanuda y no mide nada.
