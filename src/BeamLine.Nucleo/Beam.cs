using System.Text.Json;
using System.Text.Json.Nodes;
using Rodaje.Modules.Protocol;

namespace BeamLine.Nucleo;

/// <summary>
/// Un equipo de BeamLine visto desde la app: abre beamline-red, manda y responde ofertas y avisa por eventos.
/// Los eventos llegan desde un hilo de fondo; la interfaz los pasa a su hilo.
/// </summary>
public sealed class Beam : IAsyncDisposable
{
    private readonly ModuleProcess _proceso;
    private readonly SemaphoreSlim _turno = new(3); // 4 llamadas a la vez en el protocolo, una es «recibir»
    private readonly CancellationTokenSource _fin = new();
    private Task? _lectura;

    public string Clave { get; private set; } = "";
    public string Billete { get; private set; } = "";

    public event Action<Oferta>? OfertaRecibida;
    public event Action<long>? OfertaRetirada;
    public event Action<Respuesta>? Respondida;
    public event Action<Avance>? Avanzo;
    public event Action<Fin>? Termino;
    public event Action<Cercano>? CercanoCambio;
    /// <summary>beamline-red dejó de responder: el equipo ya no sirve y hay que abrir otro.</summary>
    public event Action<BeamException>? Caido;

    private Beam(ModuleProcess proceso) => _proceso = proceso;

    public static async Task<Beam> AbrirAsync(string ejecutable, OpcionesEquipo o, CancellationToken ct = default)
    {
        ArgumentNullException.ThrowIfNull(o);
        ModuleProcess p;
        try
        {
            p = await ModuleProcess.StartAsync(ejecutable, options: new ModuleClientOptions { App = "beamline", AppVersion = "0.1.0" },
                cancellationToken: ct).ConfigureAwait(false);
        }
        catch (ModuleException ex)
        {
            throw new BeamException("sin-red", "No se pudo abrir beamline-red: " + ex.Message, ex);
        }
        Beam b = new(p);
        try
        {
            JsonElement r = await b.LlamarAsync("abrir", new JsonObject
            {
                ["identidad"] = o.Identidad,
                ["alias"] = o.Alias,
                ["almacen"] = Path.GetFullPath(o.Almacen),
                ["tamanoMax"] = o.TamanoMax,
                ["red"] = new JsonObject
                {
                    ["rele"] = o.Red.Rele,
                    ["soloRele"] = o.Red.SoloRele,
                    ["mdns"] = o.Red.Mdns,
                    ["releInseguro"] = o.ReleInseguro,
                },
            }, ct).ConfigureAwait(false);
            b.Clave = r.GetProperty("clave").GetString()!;
            b.Billete = r.GetProperty("billete").GetString()!;
        }
        catch
        {
            await b.DisposeAsync().ConfigureAwait(false);
            throw;
        }
        b._lectura = Task.Run(b.LeerAsync, CancellationToken.None);
        return b;
    }

    /// <summary>Ofrece archivos a <paramref name="a"/> (clave de un equipo cercano o billete). Devuelve el id del envío.</summary>
    public async Task<long> OfrecerAsync(string a, IReadOnlyList<ArchivoAEnviar> archivos, CancellationToken ct = default)
    {
        ArgumentNullException.ThrowIfNull(archivos);
        JsonArray lista = [];
        foreach (ArchivoAEnviar x in archivos)
            lista.Add((JsonNode)new JsonObject { ["ruta"] = Path.GetFullPath(x.Ruta), ["nombre"] = x.Nombre });
        JsonElement r = await LlamarAsync("ofrecer", new JsonObject { ["a"] = a, ["archivos"] = lista }, ct).ConfigureAwait(false);
        return r.GetProperty("envio").GetInt64();
    }

    /// <summary>Acepta los índices dados hacia <paramref name="destino"/>; sin índices, rechaza.</summary>
    public Task ResponderAsync(long oferta, IReadOnlyList<int> aceptar, string? destino, CancellationToken ct = default)
    {
        ArgumentNullException.ThrowIfNull(aceptar);
        JsonArray indices = [];
        foreach (int i in aceptar) indices.Add((JsonNode)JsonValue.Create(i));
        JsonObject p = new() { ["oferta"] = oferta, ["aceptar"] = indices };
        if (destino is not null) p["destino"] = Path.GetFullPath(destino);
        return LlamarAsync("responder", p, ct);
    }

    public Task RechazarAsync(long oferta, CancellationToken ct = default) => ResponderAsync(oferta, [], null, ct);

    public Task CancelarAsync(long id, CancellationToken ct = default) =>
        LlamarAsync("cancelar", new JsonObject { ["id"] = id }, ct);

    public async Task<IReadOnlyList<Cercano>> CercanosAsync(CancellationToken ct = default)
    {
        JsonElement r = await LlamarAsync("estado", null, ct).ConfigureAwait(false);
        return [.. r.GetProperty("cercanos").EnumerateArray().Select(c =>
            new Cercano(c.GetProperty("clave").GetString()!, c.GetProperty("alias").GetString()!, true))];
    }

    /// <summary>
    /// Lista para <see cref="OfrecerAsync"/> con todos los archivos de <paramref name="carpeta"/>, nombrados
    /// «carpeta/sub/archivo». Salta enlaces simbólicos y carpetas vacías.
    /// </summary>
    public static IReadOnlyList<ArchivoAEnviar> DeCarpeta(string carpeta)
    {
        DirectoryInfo raiz = new(Path.GetFullPath(carpeta));
        string prefijo = raiz.Name;
        List<ArchivoAEnviar> lista = [];
        EnumerationOptions op = new() { RecurseSubdirectories = true, AttributesToSkip = FileAttributes.ReparsePoint, IgnoreInaccessible = true };
        foreach (FileInfo f in raiz.EnumerateFiles("*", op))
        {
            string relativa = Path.GetRelativePath(raiz.FullName, f.FullName).Replace('\\', '/');
            lista.Add(new ArchivoAEnviar(f.FullName, prefijo + "/" + relativa));
        }
        lista.Sort((x, y) => string.CompareOrdinal(x.Nombre, y.Nombre));
        return lista;
    }

    private async Task LeerAsync()
    {
        while (!_fin.IsCancellationRequested)
        {
            JsonElement r;
            try
            {
                r = await LlamarSinTurnoAsync("recibir", new JsonObject { ["esperaMs"] = 30_000 }, _fin.Token).ConfigureAwait(false);
            }
            catch (OperationCanceledException) when (_fin.IsCancellationRequested)
            {
                return;
            }
            catch (BeamException ex)
            {
                if (!_fin.IsCancellationRequested) Caido?.Invoke(ex);
                return;
            }
            foreach (JsonElement ev in r.GetProperty("eventos").EnumerateArray())
                Despachar(ev);
        }
    }

    private void Despachar(JsonElement ev)
    {
        switch (ev.GetProperty("type").GetString())
        {
            case "oferta":
                OfertaRecibida?.Invoke(new Oferta(ev.GetProperty("oferta").GetInt64(), Texto(ev, "de"), Texto(ev, "alias"),
                    [.. ev.GetProperty("archivos").EnumerateArray().Select(a => new ArchivoOfrecido(Texto(a, "nombre"), a.GetProperty("tamano").GetInt64()))],
                    ev.GetProperty("total").GetInt64()));
                break;
            case "oferta-retirada":
                OfertaRetirada?.Invoke(ev.GetProperty("oferta").GetInt64());
                break;
            case "respuesta":
                Respondida?.Invoke(new Respuesta(ev.GetProperty("envio").GetInt64(),
                    [.. ev.GetProperty("aceptados").EnumerateArray().Select(i => i.GetInt32())]));
                break;
            case "avance":
                Avanzo?.Invoke(new Avance(ev.GetProperty("id").GetInt64(), ev.GetProperty("bytes").GetInt64(), ev.GetProperty("total").GetInt64()));
                break;
            case "fin":
                Termino?.Invoke(new Fin(ev.GetProperty("id").GetInt64(), ev.GetProperty("ok").GetBoolean(),
                    ev.TryGetProperty("error", out JsonElement e) ? e.GetString() : null,
                    ev.TryGetProperty("rutas", out JsonElement rs) ? [.. rs.EnumerateArray().Select(x => x.GetString()!)] : []));
                break;
            case "cercano":
                CercanoCambio?.Invoke(new Cercano(Texto(ev, "clave"), Texto(ev, "alias"), ev.GetProperty("visto").GetBoolean()));
                break;
        }
    }

    private static string Texto(JsonElement e, string campo) => e.GetProperty(campo).GetString() ?? "";

    private async Task<JsonElement> LlamarAsync(string funcion, JsonObject? p, CancellationToken ct)
    {
        await _turno.WaitAsync(ct).ConfigureAwait(false);
        try
        {
            return await LlamarSinTurnoAsync(funcion, p, ct).ConfigureAwait(false);
        }
        finally
        {
            _turno.Release();
        }
    }

    private async Task<JsonElement> LlamarSinTurnoAsync(string funcion, JsonObject? p, CancellationToken ct)
    {
        JsonElement? parametros = p is null ? null : JsonSerializer.SerializeToElement(p, BeamJson.Default.JsonObject);
        try
        {
            ModuleResult r = await _proceso.Client.InvokeAsync(funcion, parametros, cancellationToken: ct).ConfigureAwait(false);
            return r.Value ?? default;
        }
        catch (ModuleCallException ex)
        {
            throw new BeamException(ex.Code, ex.Message, ex);
        }
        catch (ModuleException ex)
        {
            throw new BeamException("sin-red", "beamline-red dejó de responder: " + ex.Message, ex);
        }
        catch (ObjectDisposedException ex)
        {
            throw new BeamException("sin-red", "beamline-red ya está cerrado.", ex);
        }
    }

    public async ValueTask DisposeAsync()
    {
        await _fin.CancelAsync().ConfigureAwait(false);
        if (_lectura is not null)
        {
            try { await _lectura.ConfigureAwait(false); } catch (OperationCanceledException) { }
        }
        try { await LlamarSinTurnoAsync("cerrar", null, default).ConfigureAwait(false); }
        catch (BeamException) { }
        await _proceso.DisposeAsync().ConfigureAwait(false);
        _fin.Dispose();
        _turno.Dispose();
    }
}

[System.Text.Json.Serialization.JsonSerializable(typeof(JsonObject))]
internal sealed partial class BeamJson : System.Text.Json.Serialization.JsonSerializerContext;
