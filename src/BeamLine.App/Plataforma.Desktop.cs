using System.Diagnostics;
using Microsoft.UI.Xaml;

namespace BeamLine.App;

/// <summary>Lo que cambia entre Windows y Linux. Esta es la parte de Linux (Uno sobre X11): selectores con zenity o kdialog, como Rodaje.</summary>
internal static partial class Plataforma
{
    public static string Red => "beamline-red";

    // Uno en X11 no implementa mover ni redimensionar AppWindow (lanza NotImplementedException): se queda el tamaño de partida.
    public static void Colocar(Window v, bool fueraDePantalla)
    {
    }

    public static async Task<IReadOnlyList<string>> ElegirArchivosAsync(Window v)
    {
        string? r = await DialogoAsync(
            ["--file-selection", "--multiple", "--separator=\n", "--title=Elegir archivos"],
            ["--getopenfilename", ".", "--multiple", "--separate-output", "--title", "Elegir archivos"]);
        return r is null ? [] : [.. r.Split('\n', StringSplitOptions.RemoveEmptyEntries | StringSplitOptions.TrimEntries)];
    }

    public static async Task<string?> ElegirCarpetaAsync(Window v) =>
        (await DialogoAsync(["--file-selection", "--directory", "--title=Elegir carpeta"],
            ["--getexistingdirectory", ".", "--title", "Elegir carpeta"]))?.Trim() is { Length: > 0 } c ? c : null;

    /// <summary>Abre la carpeta del archivo con el gestor de archivos del escritorio.</summary>
    public static void Mostrar(string ruta)
    {
        try { Process.Start(new ProcessStartInfo("xdg-open") { ArgumentList = { Path.GetDirectoryName(ruta) ?? ruta } }); }
        catch (System.ComponentModel.Win32Exception) { /* sin xdg-open no hay a quién pedírselo */ }
    }

    // shortcut: sin aviso de urgencia de X11; la oferta igual sale en el diálogo al volver a la ventana.
    public static void Parpadear(Window v)
    {
    }

    /// <summary>zenity o, si no está, kdialog. Null si se cancela o no hay ninguno.</summary>
    private static async Task<string?> DialogoAsync(string[] zenity, string[] kdialog)
    {
        foreach ((string exe, string[] args) in new[] { ("zenity", zenity), ("kdialog", kdialog) })
        {
            ProcessStartInfo i = new(exe) { RedirectStandardOutput = true, UseShellExecute = false };
            foreach (string a in args) i.ArgumentList.Add(a);
            Process? p;
            try { p = Process.Start(i); }
            catch (System.ComponentModel.Win32Exception) { continue; }
            if (p is null) continue;
            using (p)
            {
                string salida = await p.StandardOutput.ReadToEndAsync();
                await p.WaitForExitAsync();
                return p.ExitCode == 0 ? salida : null;
            }
        }
        return null;
    }
}
