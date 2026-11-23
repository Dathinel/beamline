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

    // En X11 la barra la pone el gestor de ventanas: la de la app queda como cabecera.
    public static void Barra(Window v, UIElement barra)
    {
    }

    public static async Task<IReadOnlyList<string>> ElegirArchivosAsync(Window v)
    {
        string inicio = Inicio();
        string? r = await DialogoAsync(
            ["--file-selection", "--multiple", "--separator=\n", "--title=Elegir archivos", "--filename=" + inicio + "/"],
            ["--getopenfilename", inicio, "--multiple", "--separate-output", "--title", "Elegir archivos"]);
        // Sin recortar espacios: un nombre puede acabar en espacio. shortcut: un nombre con salto de línea se parte en dos.
        return r is null ? [] : [.. r.Split('\n', StringSplitOptions.RemoveEmptyEntries).Select(x => x.TrimEnd('\r'))];
    }

    public static async Task<string?> ElegirCarpetaAsync(Window v)
    {
        string inicio = Inicio();
        string? r = await DialogoAsync(["--file-selection", "--directory", "--title=Elegir carpeta", "--filename=" + inicio + "/"],
            ["--getexistingdirectory", inicio, "--title", "Elegir carpeta"]);
        return r?.TrimEnd('\n', '\r') is { Length: > 0 } c ? c : null;
    }

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

    /// <summary>La carpeta de descargas del escritorio (XDG, «Descargas» en español) o, si no se sabe, la del perfil.</summary>
    public static string Descargas()
    {
        string casa = Environment.GetFolderPath(Environment.SpecialFolder.UserProfile);
        try
        {
            string archivo = Path.Combine(Environment.GetFolderPath(Environment.SpecialFolder.ApplicationData), "user-dirs.dirs");
            foreach (string l in File.Exists(archivo) ? File.ReadLines(archivo) : [])
            {
                if (!l.StartsWith("XDG_DOWNLOAD_DIR=", StringComparison.Ordinal)) continue;
                string d = l["XDG_DOWNLOAD_DIR=".Length..].Trim('"').Replace("$HOME", casa, StringComparison.Ordinal);
                if (Path.IsPathRooted(d)) return d;
            }
        }
        catch (Exception ex) when (ex is IOException or UnauthorizedAccessException)
        {
        }
        return Path.Combine(casa, "Downloads");
    }

    private static string Inicio() => Environment.GetFolderPath(Environment.SpecialFolder.UserProfile);

    /// <summary>
    /// zenity o, si no está o no puede abrirse, kdialog. Null si el usuario cancela. Sin ninguno de los dos lanza
    /// <see cref="InvalidOperationException"/>, para que la ventana lo diga en vez de no hacer nada.
    /// </summary>
    private static async Task<string?> DialogoAsync(string[] zenity, string[] kdialog)
    {
        foreach ((string exe, string[] args) in new[] { ("zenity", zenity), ("kdialog", kdialog) })
        {
            ProcessStartInfo i = new(exe) { RedirectStandardOutput = true, RedirectStandardError = true, UseShellExecute = false };
            foreach (string a in args) i.ArgumentList.Add(a);
            Process? p;
            try { p = Process.Start(i); }
            catch (System.ComponentModel.Win32Exception) { continue; }
            if (p is null) continue;
            using (p)
            {
                string salida = await p.StandardOutput.ReadToEndAsync();
                await p.WaitForExitAsync();
                if (p.ExitCode == 0) return salida;
                if (p.ExitCode == 1) return null; // cancelado
                // Otro código (sin pantalla, opción no admitida…): se prueba el siguiente.
            }
        }
        throw new InvalidOperationException("Para elegir archivos hace falta zenity o kdialog. Instala uno (por ejemplo, «sudo apt install zenity») o arrastra los archivos a la ventana.");
    }
}
