namespace BeamLine.App;

/// <summary>
/// Argumentos: <c>--perfil nombre</c> separa datos e identidad (dos instancias en un PC);
/// <c>--autoprueba salida.txt</c> envía y recibe con un equipo sin ventana, escribe el resultado y cierra.
/// </summary>
internal sealed record Arranque(string Datos, string? Perfil, string? Autoprueba)
{
    public static Arranque Leer(string[] args)
    {
        string? perfil = Valor(args, "--perfil");
        string? autoprueba = Valor(args, "--autoprueba");
        if (autoprueba is not null) perfil ??= "autoprueba-" + Environment.ProcessId;
        string raiz = Path.Combine(Environment.GetFolderPath(Environment.SpecialFolder.LocalApplicationData), "BeamLine");
        if (perfil is not null)
        {
            if (perfil.Length == 0 || perfil.IndexOfAny(Path.GetInvalidFileNameChars()) >= 0 || perfil.Contains("..", StringComparison.Ordinal))
                throw new ArgumentException("--perfil no es un nombre de carpeta válido.");
            raiz = Path.Combine(raiz, "perfiles", perfil);
        }
        return new Arranque(raiz, perfil, autoprueba is null ? null : Path.GetFullPath(autoprueba));
    }

    private static string? Valor(string[] args, string nombre)
    {
        int i = Array.FindIndex(args, a => string.Equals(a, nombre, StringComparison.OrdinalIgnoreCase));
        return i >= 0 && i + 1 < args.Length ? args[i + 1] : null;
    }
}
