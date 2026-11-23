using System.ComponentModel;
using System.Runtime.CompilerServices;

namespace BeamLine.App;

public abstract partial class Observable : INotifyPropertyChanged
{
    public event PropertyChangedEventHandler? PropertyChanged;

    protected void Poner<T>(ref T campo, T valor, [CallerMemberName] string? nombre = null)
    {
        if (EqualityComparer<T>.Default.Equals(campo, valor)) return;
        campo = valor;
        PropertyChanged?.Invoke(this, new PropertyChangedEventArgs(nombre));
    }

    protected void Avisar(string nombre) => PropertyChanged?.Invoke(this, new PropertyChangedEventArgs(nombre));
}

/// <summary>A quién se puede enviar: un equipo cercano (mDNS), uno guardado o los dos.</summary>
public sealed partial class Destino(string clave) : Observable
{
    private string _alias = "";
    private bool _cercano;
    private bool _guardado;
    private bool _confianza;

    public string Clave { get; } = clave;
    public string? Billete { get; set; }
    public string Alias { get => _alias; set => Poner(ref _alias, value); }
    public bool Cercano { get => _cercano; set { Poner(ref _cercano, value); Avisar(nameof(Detalle)); } }
    public bool Guardado { get => _guardado; set { Poner(ref _guardado, value); Avisar(nameof(Detalle)); Avisar(nameof(TextoGuardar)); } }
    public bool Confianza { get => _confianza; set { Poner(ref _confianza, value); Avisar(nameof(Detalle)); } }

    /// <summary>Lo que se pasa a «ofrecer»: el billete lleva direcciones; la clave sola se busca por mDNS o por internet.</summary>
    public string A => Billete ?? Clave;

    public string Detalle
    {
        get
        {
            string donde = Cercano ? "En tu red" : Guardado ? "Guardado" : "";
            if (Confianza) donde += " · guarda sin preguntar";
            string corta = Clave.Length > 8 ? Clave[..8] : Clave;
            return corta.Length == 0 ? donde : $"{donde} · {corta}";
        }
    }

    public string TextoGuardar => Guardado ? "Quitar de la lista" : "Guardar en la lista";
}

/// <summary>Un envío o una recepción, con su avance.</summary>
public sealed partial class Transferencia(long id, bool entrante, string titulo, long total) : Observable
{
    private double _progreso;
    private string _estado = "";
    private bool _activa = true;
    private bool _fallo;
    private string? _mostrar;

    public long Id { get; } = id;
    public bool Entrante { get; } = entrante;
    public string Titulo { get; } = titulo;
    public long Total { get; set; } = total;
    public double Progreso { get => _progreso; set => Poner(ref _progreso, value); }
    public string Estado { get => _estado; set => Poner(ref _estado, value); }
    public bool Activa { get => _activa; set => Poner(ref _activa, value); }
    public bool Fallo { get => _fallo; set => Poner(ref _fallo, value); }
    public bool Ok { get; set; }

    /// <summary>Lo que abre «Mostrar» en el Explorador al terminar de recibir.</summary>
    public string? Mostrar { get => _mostrar; set { Poner(ref _mostrar, value); Avisar(nameof(PuedeMostrar)); } }
    public bool PuedeMostrar => _mostrar is not null;
}

internal static class Texto
{
    public static string Tamano(long b) => b switch
    {
        < 1024 => $"{b} B",
        < 1L << 20 => $"{b / 1024.0:0.#} KB",
        < 1L << 30 => $"{b / (double)(1L << 20):0.#} MB",
        _ => $"{b / (double)(1L << 30):0.##} GB",
    };

    public static string Archivos(int n) => n == 1 ? "1 archivo" : $"{n} archivos";

    public static string Error(string? codigo) => codigo switch
    {
        "rechazado" => "Lo rechazó",
        "cancelado" => "Cancelado",
        "sin-conexion" => "No se pudo conectar con el otro equipo",
        "red" => "Se cortó la conexión; si se vuelve a enviar, sigue donde iba",
        "huella" => "Un archivo cambió mientras se enviaba",
        "disco" => "No se pudo escribir en el disco",
        "ocupado" => "El otro equipo tiene demasiadas ofertas pendientes",
        "version" => "El otro equipo usa una versión de BeamLine que no es compatible",
        "cerrado" => "Se cerró BeamLine",
        null or "" => "Falló",
        _ => codigo,
    };
}
