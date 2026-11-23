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

## Ya comprobado por Claude con el ratón (23-nov)
Los puntos 2 (con «Elegir carpeta»), 3, 4, 6, 7, 8 y 9, más «Mostrar» y el parpadeo en la barra de tareas. Quedan para Daniel: arrastrar desde el Explorador, cancelar a mitad de un archivo grande y el punto 10.
En un solo PC, el punto 1 falla en un sentido (mDNS entre dos procesos); entre dos PC debería verse en los dos.

## Comprobado sin ratón
`build\app.ps1 -Autoprueba` abre el exe AOT con la ventana fuera de pantalla. Recibe 5 MB sin preguntar de un equipo de confianza y envía una carpeta por el mismo camino que el botón. Deja `dist\autoprueba.txt`; el 23-nov tardó 480 ms.

## Linux (fase 4)

`build\app-linux.ps1 -Autoprueba` compila en Docker y deja `dist\BeamLine-<versión>-linux-x64.tar.gz`. La autoprueba pasa bajo Xvfb, y una captura (`dist\captura-linux.png`) muestra la ventana bien dibujada. Lo que Daniel tiene que probar con el USB de Linux:
1. Descomprimir el `.tar.gz` **en el home o en una partición ext4**: en exFAT o NTFS se pierde el permiso de ejecución y `beamline-red` no arranca. Abrir `BeamLine-linux/beamline`. ¿Abre en menos de un segundo y se ve bien?
2. «Elegir archivos» y «Elegir carpeta» usan `zenity` (o `kdialog` en KDE): ¿salen, y se envía lo elegido?
3. Con BeamLine abierto en Windows y en Linux, en la misma red, ¿se ven los dos? Enviar en los dos sentidos.
4. Arrastrar archivos desde el gestor de archivos a la zona azul.
5. «Mostrar» abre la carpeta con el gestor de archivos (`xdg-open`).
6. El billete: «Copiar mi billete» en Linux y pegarlo en Windows, y al revés. Es la forma de llegar a un equipo de fuera, y en Linux el portapapeles solo lo probó la autoprueba de Rodaje.
7. Ajustes (nombre, carpeta, relé, límite) y el clic derecho sobre un equipo («Guardar lo suyo sin preguntar»). La autoprueba no los abre.
8. Sin zenity ni kdialog, «Elegir archivos» tiene que avisar de que falta. Recibir va a la carpeta de descargas del escritorio («Descargas» en español).

Ya comprobado sin Daniel: Windows envía a Linux por un relé y la huella SHA-256 coincide (23-nov).

Lo que en Linux no hace lo mismo que en Windows: no recuerda el tamaño de la ventana (Uno en X11 no lo permite) y la barra de tareas no parpadea al llegar una oferta.
