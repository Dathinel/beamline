using System.Diagnostics;
using BeamLine.Nucleo;
using Microsoft.UI.Xaml;

namespace BeamLine.App;

/// <summary>
/// <c>--autoprueba salida.txt</c>: con la ventana fuera de pantalla, un equipo sin ventana («probador») envía a la app
/// (que lo tiene como de confianza y guarda sin preguntar) y la app le envía de vuelta por el mismo camino que el botón.
/// Escribe «ok …» o «fallo …» en la salida y cierra con 0 o 1.
/// </summary>
internal static class Autoprueba
{
    public static async Task CorrerAsync(MainWindow v, Arranque a)
    {
        Stopwatch reloj = Stopwatch.StartNew();
        List<string> lineas = [];
        bool ok = false;
        string dir = Path.Combine(a.Datos, "probador");
        try
        {
            Beam app = v.Red ?? throw new InvalidOperationException("la app no abrió su red");
            TaskCompletionSource<Fin> finProbador = new(TaskCreationOptions.RunContinuationsAsynchronously);
            await using Beam probador = await Beam.AbrirAsync(Path.Combine(AppContext.BaseDirectory, Plataforma.Red),
                new OpcionesEquipo(Dathinel.Sala.Identidad.Nueva(), "probador", Path.Combine(dir, "almacen"), new OpcionesRed("ninguno", Mdns: false)), b =>
                {
                    b.Termino += f => finProbador.TrySetResult(f);
                    b.OfertaRecibida += o => _ = b.ResponderAsync(o.Id, [.. Enumerable.Range(0, o.Archivos.Count)], Path.Combine(dir, "recibido"));
                });
            v.AnadirConfianza(probador.Clave, probador.Billete, "probador");

            // 1. El probador envía 5 MB: la app guarda sin preguntar.
            Directory.CreateDirectory(dir);
            byte[] datos = new byte[5 << 20];
            Random.Shared.NextBytes(datos);
            string origen = Path.Combine(dir, "ida.bin");
            await File.WriteAllBytesAsync(origen, datos);
            await probador.OfrecerAsync(app.Billete, [new ArchivoAEnviar(origen, "ida.bin")]);
            Fin ida = await Esperar(finProbador.Task, "ida");
            finProbador = new(TaskCreationOptions.RunContinuationsAsynchronously);
            string llegada = Path.Combine(v.AjustesActuales.Carpeta, "ida.bin");
            bool idaOk = ida.Ok && File.Exists(llegada) && (await File.ReadAllBytesAsync(llegada)).AsSpan().SequenceEqual(datos);
            lineas.Add($"ida (probador → app, 5 MB, sin preguntar): {(idaOk ? "ok" : "fallo " + ida.Error)}");

            // 2. La app envía una carpeta al probador por EnviarRutasAsync, como el botón «Elegir carpeta».
            string carpeta = Path.Combine(dir, "Vuelta");
            Directory.CreateDirectory(Path.Combine(carpeta, "sub"));
            await File.WriteAllTextAsync(Path.Combine(carpeta, "sub", "nota.txt"), "hola desde la app");
            IReadOnlyList<long> envio = await v.EnviarRutasAsync([carpeta]);
            Fin vuelta = await Esperar(finProbador.Task, "vuelta");
            string recibido = Path.Combine(dir, "recibido", "Vuelta", "sub", "nota.txt");
            bool vueltaOk = envio.Count == 1 && vuelta.Ok && File.Exists(recibido) && await File.ReadAllTextAsync(recibido) == "hola desde la app";
            lineas.Add($"vuelta (app → probador, carpeta): {(vueltaOk ? "ok" : "fallo " + vuelta.Error)}");

            // 3. A dos a la vez: se marca otro probador además del primero y se envían 6 MB a los dos (fase 3).
            TaskCompletionSource<Fin> finOtro = new(TaskCreationOptions.RunContinuationsAsynchronously);
            finProbador = new(TaskCreationOptions.RunContinuationsAsynchronously);
            await using Beam otro = await Beam.AbrirAsync(Path.Combine(AppContext.BaseDirectory, Plataforma.Red),
                new OpcionesEquipo(Dathinel.Sala.Identidad.Nueva(), "probador 2", Path.Combine(dir, "almacen-2"), new OpcionesRed("ninguno", Mdns: false)), b =>
                {
                    b.Termino += f => finOtro.TrySetResult(f);
                    b.OfertaRecibida += o => _ = b.ResponderAsync(o.Id, [.. Enumerable.Range(0, o.Archivos.Count)], Path.Combine(dir, "recibido-2"));
                });
            v.AnadirConfianza(otro.Clave, otro.Billete, "probador 2");
            byte[] grande = new byte[6 << 20];
            Random.Shared.NextBytes(grande);
            string doble = Path.Combine(dir, "a-dos.bin");
            await File.WriteAllBytesAsync(doble, grande);
            IReadOnlyList<long> dos = await v.EnviarRutasAsync([doble]);
            Fin f1 = await Esperar(finProbador.Task, "a dos (1)");
            Fin f2 = await Esperar(finOtro.Task, "a dos (2)");
            bool dosOk = dos.Count == 2 && f1.Ok && f2.Ok
                && (await File.ReadAllBytesAsync(Path.Combine(dir, "recibido", "a-dos.bin"))).AsSpan().SequenceEqual(grande)
                && (await File.ReadAllBytesAsync(Path.Combine(dir, "recibido-2", "a-dos.bin"))).AsSpan().SequenceEqual(grande);
            lineas.Add($"a dos a la vez (6 MB, reparto entre receptores): {(dosOk ? "ok" : $"fallo {f1.Error} {f2.Error}")}");

            // 4. Un equipo que no es de confianza ofrece dos archivos: sale el diálogo, se desmarca el primero y se acepta.
            await using Beam extrano = await Beam.AbrirAsync(Path.Combine(AppContext.BaseDirectory, Plataforma.Red),
                new OpcionesEquipo(Dathinel.Sala.Identidad.Nueva(), "extraño", Path.Combine(dir, "almacen-3"), new OpcionesRed("ninguno", Mdns: false)));
            TaskCompletionSource<Fin> finExtrano = new(TaskCreationOptions.RunContinuationsAsynchronously);
            extrano.Termino += f => finExtrano.TrySetResult(f);
            await File.WriteAllTextAsync(Path.Combine(dir, "uno.txt"), "uno");
            await File.WriteAllTextAsync(Path.Combine(dir, "dos.txt"), "dos");
            v.ElegirEnPrueba = l => l.SelectedItems.Remove(l.Items[0]);
            await extrano.OfrecerAsync(app.Billete, [new ArchivoAEnviar(Path.Combine(dir, "uno.txt"), "uno.txt"), new ArchivoAEnviar(Path.Combine(dir, "dos.txt"), "dos.txt")]);
            Fin f4 = await Esperar(finExtrano.Task, "diálogo");
            string carpetaApp = v.AjustesActuales.Carpeta;
            bool dialogoOk = f4.Ok && File.Exists(Path.Combine(carpetaApp, "dos.txt")) && !File.Exists(Path.Combine(carpetaApp, "uno.txt"));
            lineas.Add($"diálogo de oferta (desmarcar uno y aceptar): {(dialogoOk ? "ok" : "fallo " + f4.Error)}");
            ok = idaOk && vueltaOk && dosOk && dialogoOk;
        }
#pragma warning disable CA1031 // la autoprueba deja cualquier fallo escrito
        catch (Exception ex)
#pragma warning restore CA1031
        {
            lineas.Add("excepción: " + ex);
        }
        lineas.Insert(0, $"{(ok ? "ok" : "fallo")} en {reloj.ElapsedMilliseconds} ms");
        await File.WriteAllLinesAsync(a.Autoprueba!, lineas);
        v.Close();
        try { Directory.Delete(a.Datos, true); } catch (IOException) { } catch (UnauthorizedAccessException) { }
        Environment.Exit(ok ? 0 : 1);
    }

    private static async Task<Fin> Esperar(Task<Fin> t, string que) =>
        await t.WaitAsync(TimeSpan.FromSeconds(60)).ConfigureAwait(true) is { } f ? f : throw new TimeoutException(que);
}
