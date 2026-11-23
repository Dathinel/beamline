using Uno.UI.Hosting;

namespace BeamLine.App;

/// <summary>Arranque en Linux: el anfitrión X11 de Uno (en Windows lo genera WinUI).</summary>
internal static class Program
{
    [STAThread]
    public static void Main(string[] args)
    {
        var host = UnoPlatformHostBuilder.Create()
            .App(() => new App())
            .UseX11()
            .Build();
        host.Run();
    }
}
