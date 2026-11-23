using System.Collections.Concurrent;

namespace BeamLine.Nucleo.Tests;

/// <summary>Dos procesos beamline-red de verdad en este PC, sin relé, hablando por loopback.</summary>
public sealed class PuntaAPuntaTests : IAsyncLifetime
{
    private readonly string _dir = Path.Combine(Path.GetTempPath(), "beamline-pruebas-" + Guid.NewGuid().ToString("N"));
    private Beam _ana = null!;
    private Beam _beto = null!;
    private readonly ConcurrentQueue<object> _eventosAna = new();
    private readonly ConcurrentQueue<object> _eventosBeto = new();

    internal static string RutaRed
    {
        get
        {
            string? env = Environment.GetEnvironmentVariable("BEAMLINE_RED");
            if (!string.IsNullOrEmpty(env)) return env;
            DirectoryInfo? d = new(AppContext.BaseDirectory);
            while (d is not null && !File.Exists(Path.Combine(d.FullName, "BeamLine.slnx"))) d = d.Parent;
            string exe = OperatingSystem.IsWindows() ? "beamline-red.exe" : "beamline-red";
            return Path.Combine(d!.FullName, "red", "target", "debug", exe);
        }
    }

    private async Task<Beam> AbrirAsync(string alias, ConcurrentQueue<object> cola)
    {
        return await Beam.AbrirAsync(RutaRed, new OpcionesEquipo(Dathinel.Sala.Identidad.Nueva(), alias,
            Path.Combine(_dir, "almacen-" + alias), new OpcionesRed("ninguno", Mdns: false)), b =>
        {
            b.OfertaRecibida += o => cola.Enqueue(o);
            b.Respondida += r => cola.Enqueue(r);
            b.Termino += f => cola.Enqueue(f);
            b.Avanzo += a => cola.Enqueue(a);
            b.OfertaRetirada += id => cola.Enqueue(id);
        });
    }

    public async Task InitializeAsync()
    {
        Directory.CreateDirectory(_dir);
        _ana = await AbrirAsync("ana", _eventosAna);
        _beto = await AbrirAsync("beto", _eventosBeto);
    }

    public async Task DisposeAsync()
    {
        await _ana.DisposeAsync();
        await _beto.DisposeAsync();
        try { Directory.Delete(_dir, true); } catch (IOException) { }
    }

    private static async Task<T> EsperarAsync<T>(ConcurrentQueue<object> cola, Func<T, bool>? cumple = null)
    {
        List<object> otros = [];
        DateTime hasta = DateTime.UtcNow.AddSeconds(60);
        while (DateTime.UtcNow < hasta)
        {
            while (cola.TryDequeue(out object? e))
            {
                if (e is T t && (cumple is null || cumple(t)))
                {
                    foreach (object o in otros) cola.Enqueue(o);
                    return t;
                }
                otros.Add(e);
            }
            await Task.Delay(50);
        }
        throw new TimeoutException($"No llegó {typeof(T).Name}; llegaron {string.Join(", ", otros)}");
    }

    private async Task<string> ArchivoAsync(string nombre, string texto = "a")
    {
        string f = Path.Combine(_dir, nombre);
        await File.WriteAllTextAsync(f, texto);
        return f;
    }

    [Fact]
    public async Task Una_carpeta_entera_llega_igual_y_con_su_estructura()
    {
        string origen = Path.Combine(_dir, "Vacaciones");
        Directory.CreateDirectory(Path.Combine(origen, "día 1"));
        byte[] grande = new byte[3 << 20];
        Random.Shared.NextBytes(grande);
        await File.WriteAllBytesAsync(Path.Combine(origen, "día 1", "vídeo.mp4"), grande);
        await File.WriteAllTextAsync(Path.Combine(origen, "notas.txt"), "hola");

        IReadOnlyList<ArchivoAEnviar> lista = Beam.DeCarpeta(origen);
        Assert.Equal(["Vacaciones/día 1/vídeo.mp4", "Vacaciones/notas.txt"], lista.Select(a => a.Nombre));
        long envio = await _ana.OfrecerAsync(_beto.Billete, lista);

        Oferta o = await EsperarAsync<Oferta>(_eventosBeto);
        Assert.Equal("ana", o.Alias);
        Assert.Equal(_ana.Clave, o.De);
        Assert.Equal(grande.Length + 4, o.Total);
        string destino = Path.Combine(_dir, "Descargas");
        await _beto.ResponderAsync(o.Id, [0, 1], destino);

        Assert.False((await EsperarAsync<Respuesta>(_eventosAna)).Rechazada);
        Fin fa = await EsperarAsync<Fin>(_eventosAna, f => f.Id == envio);
        Fin fb = await EsperarAsync<Fin>(_eventosBeto, f => f.Id == o.Id);
        Assert.True(fa.Ok, fa.Error);
        Assert.True(fb.Ok, fb.Error);
        Assert.Equal(2, fb.Rutas.Count);
        Assert.Equal(grande, await File.ReadAllBytesAsync(Path.Combine(destino, "Vacaciones", "día 1", "vídeo.mp4")));
        Assert.Equal("hola", await File.ReadAllTextAsync(Path.Combine(destino, "Vacaciones", "notas.txt")));
    }

    [Fact]
    public async Task Rechazar_avisa_al_que_envia()
    {
        long envio = await _ana.OfrecerAsync(_beto.Billete, [new ArchivoAEnviar(await ArchivoAsync("a.txt"), "a.txt")]);
        Oferta o = await EsperarAsync<Oferta>(_eventosBeto);
        await _beto.RechazarAsync(o.Id);
        Assert.True((await EsperarAsync<Respuesta>(_eventosAna)).Rechazada);
        Assert.Equal("rechazado", (await EsperarAsync<Fin>(_eventosAna, x => x.Id == envio)).Error);
    }

    [Fact]
    public async Task Errores_de_llamada_llegan_con_su_codigo()
    {
        string f = await ArchivoAsync("a.txt");
        BeamException e = await Assert.ThrowsAsync<BeamException>(() => _ana.OfrecerAsync(_beto.Billete, [new ArchivoAEnviar(f, "../a.txt")]));
        Assert.Equal("nombre-invalido", e.Codigo);
        e = await Assert.ThrowsAsync<BeamException>(() => _ana.OfrecerAsync("beam1nada", [new ArchivoAEnviar(f, "a.txt")]));
        Assert.Equal("billete-invalido", e.Codigo);
        e = await Assert.ThrowsAsync<BeamException>(() => _beto.ResponderAsync(999, [0], _dir));
        Assert.Equal("desconocido", e.Codigo);
        e = await Assert.ThrowsAsync<BeamException>(() => _ana.CancelarAsync(999));
        Assert.Equal("desconocido", e.Codigo);
    }

    [Fact]
    public async Task Un_manejador_que_falla_no_para_los_eventos()
    {
        int fallos = 0;
        _beto.OfertaRecibida += _ => throw new InvalidOperationException("fallo de la interfaz");
        _beto.ManejadorFallo += _ => Interlocked.Increment(ref fallos);
        string f = await ArchivoAsync("a.txt");
        await _ana.OfrecerAsync(_beto.Billete, [new ArchivoAEnviar(f, "a.txt")]);
        Oferta o1 = await EsperarAsync<Oferta>(_eventosBeto);
        await _beto.RechazarAsync(o1.Id);
        await _ana.OfrecerAsync(_beto.Billete, [new ArchivoAEnviar(f, "a.txt")]);
        Oferta o2 = await EsperarAsync<Oferta>(_eventosBeto);
        Assert.NotEqual(o1.Id, o2.Id);
        Assert.True(fallos >= 2);
    }

    [Fact]
    public async Task El_estado_dice_si_hay_rele()
    {
        Estado e = await _ana.EstadoAsync();
        Assert.False(e.ReleConectado); // pruebas sin relé
        Assert.Empty(e.Cercanos);      // y sin mDNS
    }

    [Fact]
    public void La_clave_sale_del_billete()
    {
        Assert.Equal(_beto.Clave, Beam.ClaveDeBillete(_beto.Billete));
        Assert.Null(Beam.ClaveDeBillete("beam1nada"));
        Assert.Null(Beam.ClaveDeBillete(_beto.Clave));
    }

    [Fact]
    public async Task Un_manejador_de_fallos_que_lanza_tampoco_para_ni_impide_cerrar()
    {
        _beto.OfertaRecibida += _ => throw new InvalidOperationException("fallo de la interfaz");
        _beto.ManejadorFallo += _ => throw new InvalidOperationException("fallo del aviso");
        string f = await ArchivoAsync("a.txt");
        await _ana.OfrecerAsync(_beto.Billete, [new ArchivoAEnviar(f, "a.txt")]);
        Oferta o = await EsperarAsync<Oferta>(_eventosBeto);
        await _beto.RechazarAsync(o.Id);
        Assert.True((await EsperarAsync<Respuesta>(_eventosAna)).Rechazada);
    }

    [Fact]
    public void No_se_envia_una_unidad_entera() =>
        Assert.Throws<ArgumentException>(() => Beam.DeCarpeta(Path.GetPathRoot(Path.GetTempPath())!));

    [Fact]
    public async Task Cancelar_una_oferta_pendiente_la_retira_en_el_otro()
    {
        long envio = await _ana.OfrecerAsync(_beto.Billete, [new ArchivoAEnviar(await ArchivoAsync("a.txt"), "a.txt")]);
        Oferta o = await EsperarAsync<Oferta>(_eventosBeto);
        await _ana.CancelarAsync(envio);
        Assert.Equal(o.Id, await EsperarAsync<long>(_eventosBeto));
        Assert.Equal("cancelado", (await EsperarAsync<Fin>(_eventosAna, x => x.Id == envio)).Error);
    }
}
