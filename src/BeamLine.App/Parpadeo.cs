using System.Runtime.InteropServices;

namespace BeamLine.App;

/// <summary>Hace parpadear el botón de la barra de tareas hasta que la ventana vuelva al frente (sin robarle el foco a nadie).</summary>
internal static partial class Parpadeo
{
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

    public static void SiNoEstaAlFrente(nint ventana)
    {
        if (ventana == 0 || GetForegroundWindow() == ventana) return;
        Flashwinfo f = new() { Tamano = (uint)Marshal.SizeOf<Flashwinfo>(), Ventana = ventana, Banderas = FlashwAll | FlashwTimernofg };
        FlashWindowEx(ref f);
    }
}
