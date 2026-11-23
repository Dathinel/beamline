namespace BeamLine.Nucleo;

/// <summary>Error de beamline-red con su código (docs/protocolo.md §1), o «sin-red» si el proceso no responde.</summary>
public sealed class BeamException(string codigo, string mensaje, Exception? causa = null) : Exception(mensaje, causa)
{
    public string Codigo { get; } = codigo;
}

/// <summary>Un archivo local que se ofrece: <paramref name="Nombre"/> es la ruta relativa que verá el otro, con «/».</summary>
public sealed record ArchivoAEnviar(string Ruta, string Nombre);

public sealed record ArchivoOfrecido(string Nombre, long Tamano);

/// <summary>Alguien quiere enviar: se acepta o se rechaza con <see cref="Beam.ResponderAsync"/>.</summary>
public sealed record Oferta(long Id, string De, string Alias, IReadOnlyList<ArchivoOfrecido> Archivos, long Total);

public sealed record Respuesta(long Envio, IReadOnlyList<int> Aceptados)
{
    public bool Rechazada => Aceptados.Count == 0;
}

public sealed record Avance(long Id, long Bytes, long Total);

/// <summary>Fin de un envío o de una recepción. <see cref="Rutas"/> solo en el que recibe.</summary>
public sealed record Fin(long Id, bool Ok, string? Error, IReadOnlyList<string> Rutas);

public sealed record Cercano(string Clave, string Alias, bool Visto);

/// <summary>Equipos cercanos por mDNS, si el relé contesta (sin relé, solo hay conexión directa) y los bytes servidos.</summary>
public sealed record Estado(IReadOnlyList<Cercano> Cercanos, bool ReleConectado, long Subidos);

/// <summary>
/// Cómo sale a la red: relé «n0», «ninguno» o una URL https; «solo relé» (solo con relé propio) oculta la IP;
/// <paramref name="LimiteSubida"/> en bytes por segundo, o null sin límite.
/// </summary>
public sealed record OpcionesRed(string Rele = "n0", bool SoloRele = false, bool Mdns = true, long? LimiteSubida = null);

public sealed record OpcionesEquipo(string Identidad, string Alias, string Almacen, OpcionesRed Red)
{
    /// <summary>Lo más que se acepta en una oferta; más grande se rechaza sola.</summary>
    public long TamanoMax { get; init; } = 1L << 40;

    /// <summary>Solo pruebas: no verificar el certificado del relé local de pruebas.</summary>
    internal bool ReleInseguro { get; init; }
}
