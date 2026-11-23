using System.Text.Json;
using System.Text.Json.Serialization;

namespace BeamLine.App;

/// <summary>Un equipo guardado: se le envía por clave (o billete si lo hay) y, si es de confianza, lo suyo se guarda sin preguntar.</summary>
internal sealed class Guardado
{
    public string Alias { get; set; } = "";
    public string Clave { get; set; } = "";
    public string? Billete { get; set; }
    public bool Confianza { get; set; }
}

internal sealed class Ajustes
{
    public string Alias { get; set; } = Environment.MachineName;
    public string Carpeta { get; set; } = Path.Combine(Environment.GetFolderPath(Environment.SpecialFolder.UserProfile), "Downloads", "BeamLine");
    /// <summary>«n0» (internet con el relé público), «ninguno» (solo directa) o una URL https de relé propio.</summary>
    public string Rele { get; set; } = "n0";
    /// <summary>Con relé propio: que todo vaya por él (oculta la IP a los demás).</summary>
    public bool SoloRele { get; set; }
    /// <summary>MiB por segundo que sirve como mucho este equipo; 0, sin límite.</summary>
    public double LimiteSubidaMBs { get; set; }
    public List<Guardado> Equipos { get; set; } = [];

    public static Ajustes Cargar(string archivo)
    {
        try
        {
            if (File.Exists(archivo))
                return JsonSerializer.Deserialize(File.ReadAllText(archivo), AjustesJson.Default.Ajustes) ?? new Ajustes();
        }
        catch (JsonException)
        {
            // Dañado: se empieza de cero sin pisarlo hasta el próximo guardado.
        }
        return new Ajustes();
    }

    public void Guardar(string archivo)
    {
        Directory.CreateDirectory(Path.GetDirectoryName(archivo)!);
        string tmp = archivo + ".tmp";
        File.WriteAllText(tmp, JsonSerializer.Serialize(this, AjustesJson.Default.Ajustes));
        File.Move(tmp, archivo, true);
    }
}

[JsonSourceGenerationOptions(WriteIndented = true, PropertyNamingPolicy = JsonKnownNamingPolicy.CamelCase)]
[JsonSerializable(typeof(Ajustes))]
internal sealed partial class AjustesJson : JsonSerializerContext;
