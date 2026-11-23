using System.Diagnostics;
using System.Runtime.InteropServices;
using Microsoft.UI.Xaml;

namespace BeamLine.App;

/// <summary>Lo que cambia entre Windows y Linux (Plataforma.Desktop.cs). Esta es la parte de Windows (WinUI 3).</summary>
internal static partial class Plataforma
{
    public static string Red => "beamline-red.exe";

    public static string Descargas() =>
        Path.Combine(Environment.GetFolderPath(Environment.SpecialFolder.UserProfile), "Downloads");

    public static void Colocar(Window v, bool fueraDePantalla)
    {
        v.AppWindow.Resize(new Windows.Graphics.SizeInt32(960, 640));
        if (fueraDePantalla) v.AppWindow.Move(new Windows.Graphics.PointInt32(-20000, -20000));
    }

    public static async Task<IReadOnlyList<string>> ElegirArchivosAsync(Window v)
    {
        Microsoft.Windows.Storage.Pickers.FileOpenPicker p = new(v.AppWindow.Id);
        p.FileTypeFilter.Add("*");
        var r = await p.PickMultipleFilesAsync();
        return r is null ? [] : [.. r.Select(x => x.Path)];
    }

    public static async Task<string?> ElegirCarpetaAsync(Window v)
    {
        Microsoft.Windows.Storage.Pickers.FolderPicker p = new(v.AppWindow.Id);
        var r = await p.PickSingleFolderAsync();
        return r?.Path;
    }

    /// <summary>Abre el Explorador con el archivo elegido.</summary>
    public static void Mostrar(string ruta) =>
        Process.Start(new ProcessStartInfo("explorer.exe") { ArgumentList = { "/select," + ruta } });

    /// <summary>Hace parpadear el botón de la barra de tareas hasta que la ventana vuelva al frente (sin robarle el foco a nadie).</summary>
    public static void Parpadear(Window v)
    {
        nint ventana = WinRT.Interop.WindowNative.GetWindowHandle(v);
        if (ventana == 0 || GetForegroundWindow() == ventana) return;
        Flashwinfo f = new() { Tamano = (uint)Marshal.SizeOf<Flashwinfo>(), Ventana = ventana, Banderas = FlashwAll | FlashwTimernofg };
        FlashWindowEx(ref f);
    }

    [StructLayout(LayoutKind.Sequential)]
    private struct Flashwinfo
    {
        public uint Tamano;
        public nint Ventana;
        public uint Banderas;
        public uint Veces;
        public uint Ritmo;
    }

    private const uint FlashwAll = 3, FlashwTimernofg = 12;

    [LibraryImport("user32.dll")]
    [return: MarshalAs(UnmanagedType.Bool)]
    private static partial bool FlashWindowEx(ref Flashwinfo info);

    [LibraryImport("user32.dll")]
    private static partial nint GetForegroundWindow();
}
