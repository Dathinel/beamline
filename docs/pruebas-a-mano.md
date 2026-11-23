# Pruebas a mano (fase 2: interfaz)

Las juzga Daniel. Lo que encuentre entra como corrección.

## Preparar
1. `powershell -NoProfile -ExecutionPolicy Bypass -File build\app.ps1` publica en `dist\BeamLine`. Tarda unos 10 min la primera vez y luego unos 2.
2. Abre dos ventanas en el mismo PC, cada una con sus datos y su identidad:
   `dist\BeamLine\BeamLine.exe --perfil ana` y `dist\BeamLine\BeamLine.exe --perfil beto`.

## Qué probar
1. **Se ven solas.** A los pocos segundos, cada ventana tiene a la otra en «Enviar a», con «En tu red».
2. **Arrastrar una carpeta.** En «ana», elige a beto y arrastra una carpeta con subcarpetas desde el Explorador hasta la zona azul. En «beto» sale el diálogo con la lista de archivos, todos marcados.
3. **Aceptar una parte.** Desmarca uno y pulsa Aceptar. Las dos barras avanzan. Al terminar, «Mostrar» abre el Explorador con lo recibido en `Descargas\BeamLine`, con la estructura de la carpeta y sin el archivo desmarcado.
4. **Rechazar.** Envía otra vez y rechaza: en «ana» queda en rojo con «Lo rechazó».
5. **Cancelar.** Envía un archivo grande (unos GB) y pulsa Cancelar a mitad, en cualquiera de las dos ventanas: las dos quedan en «Cancelado» y en Descargas no quedan restos.
6. **Retirar antes de responder.** Envía y cancela en «ana» con el diálogo abierto en «beto»: el diálogo se cierra solo y aparece el aviso.
7. **Sin preguntar.** Marca «Guardar lo suyo sin preguntar» al aceptar, o hazlo con el clic derecho sobre el equipo. El siguiente envío se guarda sin diálogo.
8. **Billete.** En «beto», pulsa «Copiar mi billete» y pégalo en la caja de «ana»: la zona dice «el equipo del billete» y se puede enviar. «Guardar en la lista» lo deja guardado.
9. **Ajustes.** Cambia el nombre de «ana»: en «beto» aparece con el nombre nuevo. Con «Solo tu red local», el estado lo dice.
10. **Fluidez.** Mira si algo se ve torpe: tamaños, textos, la zona de soltar, el tema claro y el oscuro.

## Comprobado sin ratón
`build\app.ps1 -Autoprueba` abre el exe AOT con la ventana fuera de pantalla. Recibe 5 MB sin preguntar de un equipo de confianza y envía una carpeta por el mismo camino que el botón. Deja `dist\autoprueba.txt`; el 23-nov tardó 480 ms.
