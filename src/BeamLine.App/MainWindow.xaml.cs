using System.Collections.ObjectModel;
using System.Diagnostics;
using BeamLine.Nucleo;
using Microsoft.UI.Dispatching;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;
using Windows.ApplicationModel.DataTransfer;
using Windows.Storage;

namespace BeamLine.App;

public sealed partial class MainWindow : Window
{
    private const int MaxArchivos = 4096; // docs/protocolo.md §4
    private const int MaxSinFila = 256;

    private readonly Arranque _arranque;
    private readonly string _archivoAjustes;
    private readonly Ajustes _ajustes;
    private readonly string _identidad;
    private readonly DispatcherQueue _hilo;
    private readonly DispatcherQueueTimer _reloj;
    private readonly Queue<Oferta> _pendientes = new();
    // Eventos de un id cuya fila aún no existe (el envío vuelve de «ofrecer» después de que llegue su respuesta).
    private readonly Dictionary<long, List<object>> _sinFila = [];
    private Beam? _beam;
    private ContentDialog? _dialogo;
    private bool _hayDialogo;
    private long _ofertaEnDialogo = -1;

    public ObservableCollection<Destino> Destinos { get; } = [];
    public ObservableCollection<Transferencia> Transferencias { get; } = [];

    internal MainWindow(Arranque arranque)
    {
        _arranque = arranque;
        _hilo = DispatcherQueue.GetForCurrentThread();
        InitializeComponent();
        AppWindow.Resize(new Windows.Graphics.SizeInt32(960, 640));
        if (arranque.Autoprueba is not null) AppWindow.Move(new Windows.Graphics.PointInt32(-20000, -20000));
        Title = arranque.Perfil is null ? "BeamLine" : $"BeamLine ({arranque.Perfil})";

        _archivoAjustes = Path.Combine(arranque.Datos, "ajustes.json");
        _ajustes = Ajustes.Cargar(_archivoAjustes);
        if (arranque.Perfil is not null && !File.Exists(_archivoAjustes)) _ajustes.Alias = $"{Environment.MachineName} ({arranque.Perfil})";
        if (arranque.Autoprueba is not null)
        {
            _ajustes.Rele = "ninguno";
            _ajustes.Carpeta = Path.Combine(arranque.Datos, "recibido");
        }
        _identidad = Dathinel.Sala.Identidad.CargarOCrear(Path.Combine(arranque.Datos, "identidad.key"));
        foreach (Guardado g in _ajustes.Equipos)
        {
            // Los guardados por billete antes del 23-nov llevaban el billete como clave.
            string clave = g.Clave.StartsWith("beam1", StringComparison.Ordinal) ? Beam.ClaveDeBillete(g.Clave) ?? g.Clave : g.Clave;
            if (Destinos.Any(d => d.Clave == clave)) continue;
            Destinos.Add(new Destino(clave) { Alias = g.Alias, Billete = g.Billete, Guardado = true, Confianza = g.Confianza });
        }

        _reloj = _hilo.CreateTimer();
        _reloj.Interval = TimeSpan.FromSeconds(5);
        _reloj.Tick += async (_, _) => await RefrescarEstadoAsync();
        Closed += (_, _) => CerrarAsync().AsTask().Wait(TimeSpan.FromSeconds(3)); // beamline-red muere solo si muere la app
        ActualizarZona();
        _ = IniciarAsync();
    }

    private async Task IniciarAsync()
    {
        await AbrirBeamAsync();
        if (_arranque.Autoprueba is not null) await Autoprueba.CorrerAsync(this, _arranque);
    }

    // ---------- Red ----------

    private async Task AbrirBeamAsync()
    {
        MiAlias.Text = _ajustes.Alias;
        MiEstado.Text = "Conectando…";
        bool autoprueba = _arranque.Autoprueba is not null;
        try
        {
            Beam b = await Beam.AbrirAsync(Path.Combine(AppContext.BaseDirectory, "beamline-red.exe"),
                new OpcionesEquipo(_identidad, _ajustes.Alias, Path.Combine(_arranque.Datos, "almacen"),
                    new OpcionesRed(_ajustes.Rele, _ajustes.SoloRele && EsUrl(_ajustes.Rele), Mdns: !autoprueba,
                        LimiteSubida: _ajustes.LimiteSubidaMBs > 0 ? (long)(_ajustes.LimiteSubidaMBs * (1 << 20)) : null)), b =>
                {
                    // Cada evento se aplica solo si su red sigue siendo la actual (tras reabrir desde Ajustes, los ids empiezan de nuevo).
                    void En(Action a) => _hilo.TryEnqueue(() => { if (ReferenceEquals(_beam, b)) a(); });
                    b.OfertaRecibida += o => En(() => AlRecibirOferta(o));
                    b.OfertaRetirada += id => En(() => AlRetirarOferta(id));
                    b.Respondida += r => En(() => AlEvento(r.Envio, r));
                    b.Avanzo += a => En(() => AlEvento(a.Id, a));
                    b.Termino += f => En(() => AlEvento(f.Id, f));
                    b.CercanoCambio += c => En(() => AlCambiarCercano(c));
                    b.Caido += e => En(() => { MiEstado.Text = "La red se detuvo: " + e.Message; Avisar("Cierra y vuelve a abrir BeamLine.", InfoBarSeverity.Error); });
                    b.ManejadorFallo += e => Debug.WriteLine(e);
                });
            _beam = b;
            _reloj.Start();
            await RefrescarEstadoAsync();
        }
        catch (BeamException e)
        {
            MiEstado.Text = "Sin red";
            Avisar("No se pudo abrir la red: " + e.Message, InfoBarSeverity.Error);
        }
        ActualizarZona();
    }

    private async Task RefrescarEstadoAsync()
    {
        if (_beam is not { } b) return;
        try
        {
            Estado e = await b.EstadoAsync();
            if (!ReferenceEquals(_beam, b)) return;
            // Por si se perdió algún evento «cercano»: la lista del estado manda.
            foreach (Cercano c in e.Cercanos)
                if (Destinos.FirstOrDefault(d => d.Clave == c.Clave) is not { Cercano: true } d || d.Alias != c.Alias) AlCambiarCercano(c);
            foreach (Destino d in Destinos.Where(d => d.Cercano && e.Cercanos.All(c => c.Clave != d.Clave)).ToList())
                AlCambiarCercano(new Cercano(d.Clave, d.Alias, false));
            MiEstado.Text = _ajustes.Rele == "ninguno" ? "Sin relé: tu red y billetes con dirección directa"
                : !e.ReleConectado ? "Solo tu red (sin conexión con el relé)"
                : EsUrl(_ajustes.Rele) ? (_ajustes.SoloRele ? "Conectado por tu relé (tu IP queda oculta)" : "Conectado: tu red e internet, con tu relé")
                : "Conectado: tu red e internet";
        }
        catch (BeamException)
        {
            // Caido ya lo avisa.
        }
    }

    private static bool EsUrl(string rele) => rele.StartsWith("https://", StringComparison.OrdinalIgnoreCase);

    private async ValueTask CerrarAsync()
    {
        _reloj.Stop();
        Beam? b = Interlocked.Exchange(ref _beam, null);
        if (b is not null) await b.DisposeAsync().ConfigureAwait(false); // sin volver al hilo de la interfaz: Closed espera bloqueándolo
    }

    // ---------- Equipos ----------

    private void AlCambiarCercano(Cercano c)
    {
        Destino? d = Destinos.FirstOrDefault(x => x.Clave == c.Clave);
        if (c.Visto)
        {
            if (d is null) Destinos.Insert(0, d = new Destino(c.Clave));
            d.Alias = c.Alias;
            d.Cercano = true;
        }
        else if (d is not null)
        {
            d.Cercano = false;
            if (!d.Guardado) Destinos.Remove(d);
        }
        ActualizarZona();
    }

    private List<Destino> Elegidos => [.. ListaDestinos.SelectedItems.OfType<Destino>()];

    private void ListaDestinos_SelectionChanged(object sender, SelectionChangedEventArgs e) => ActualizarZona();

    private void ActualizarZona()
    {
        SinEquipos.Visibility = Destinos.Count == 0 ? Visibility.Visible : Visibility.Collapsed;
        List<Destino> e = Elegidos;
        string? a = e.Count switch
        {
            0 => Beam.ClaveDeBillete(CajaBillete.Text) is not null ? "el equipo del billete" : null,
            1 => e[0].Alias,
            _ => $"{e.Count} equipos a la vez",
        };
        TextoZona.Text = a is null ? "Elige a la izquierda a quién enviar" : $"Suelta aquí archivos o carpetas para enviarlos a {a}";
        ElegirArchivos.IsEnabled = ElegirCarpeta.IsEnabled = a is not null && _beam is not null;
    }

    private void CajaBillete_TextChanged(object sender, TextChangedEventArgs e)
    {
        GuardarBillete.IsEnabled = Beam.ClaveDeBillete(CajaBillete.Text) is not null;
        if (GuardarBillete.IsEnabled) ListaDestinos.SelectedItems.Clear();
        ActualizarZona();
    }

    private void GuardarBillete_Click(object sender, RoutedEventArgs e)
    {
        string billete = CajaBillete.Text.Trim();
        if (Beam.ClaveDeBillete(billete) is not { } clave) return;
        Destino? d = Destinos.FirstOrDefault(x => x.Clave == clave);
        if (d is null) Destinos.Add(d = new Destino(clave) { Alias = $"Equipo {Destinos.Count(x => x.Guardado) + 1}" });
        d.Billete = billete;
        d.Guardado = true;
        CajaBillete.Text = "";
        ListaDestinos.SelectedItems.Clear();
        ListaDestinos.SelectedItems.Add(d);
        GuardarEquipos();
    }

    private void Guardar_Click(object sender, RoutedEventArgs e)
    {
        if ((sender as FrameworkElement)?.Tag is not Destino d) return;
        d.Guardado = !d.Guardado;
        if (!d.Guardado)
        {
            d.Confianza = false;
            if (!d.Cercano) Destinos.Remove(d);
        }
        GuardarEquipos();
        ActualizarZona();
    }

    private void Confianza_Click(object sender, RoutedEventArgs e)
    {
        if ((sender as FrameworkElement)?.Tag is not Destino d) return;
        d.Confianza = !d.Confianza;
        if (d.Confianza) d.Guardado = true;
        GuardarEquipos();
    }

    private void GuardarEquipos()
    {
        _ajustes.Equipos = [.. Destinos.Where(d => d.Guardado).Select(d => new Guardado { Alias = d.Alias, Clave = d.Clave, Billete = d.Billete, Confianza = d.Confianza })];
        try { _ajustes.Guardar(_archivoAjustes); }
        catch (Exception ex) when (ex is IOException or UnauthorizedAccessException) { Avisar("No se pudieron guardar los ajustes: " + ex.Message, InfoBarSeverity.Warning); }
    }

    private void CopiarBillete_Click(object sender, RoutedEventArgs e)
    {
        if (_beam is null) return;
        DataPackage p = new();
        p.SetText(_beam.Billete);
        Clipboard.SetContent(p);
        Avisar("Billete copiado. Quien lo tenga puede ofrecerte archivos; tú decides si los aceptas.", InfoBarSeverity.Success);
    }

    // ---------- Enviar ----------

    private void Zona_DragOver(object sender, DragEventArgs e)
    {
        if (e.DataView.Contains(StandardDataFormats.StorageItems) && Objetivo() is not null)
        {
            e.AcceptedOperation = DataPackageOperation.Copy;
            e.DragUIOverride.Caption = "Enviar";
        }
    }

    private async void Zona_Drop(object sender, DragEventArgs e)
    {
        if (!e.DataView.Contains(StandardDataFormats.StorageItems)) return;
        IReadOnlyList<IStorageItem> items = await e.DataView.GetStorageItemsAsync();
        await EnviarRutasAsync([.. items.Select(i => i.Path).Where(p => !string.IsNullOrEmpty(p))]);
    }

    private async void ElegirArchivos_Click(object sender, RoutedEventArgs e)
    {
        Microsoft.Windows.Storage.Pickers.FileOpenPicker p = new(AppWindow.Id);
        p.FileTypeFilter.Add("*");
        var r = await p.PickMultipleFilesAsync();
        if (r is not null) await EnviarRutasAsync([.. r.Select(x => x.Path)]);
    }

    private async void ElegirCarpeta_Click(object sender, RoutedEventArgs e)
    {
        Microsoft.Windows.Storage.Pickers.FolderPicker p = new(AppWindow.Id);
        var r = await p.PickSingleFolderAsync();
        if (r is not null) await EnviarRutasAsync([r.Path]);
    }

    private List<(string A, string Alias)>? Objetivo()
    {
        List<Destino> e = Elegidos;
        if (e.Count > 0) return [.. e.Select(d => (d.A, d.Alias))];
        string t = CajaBillete.Text.Trim();
        return Beam.ClaveDeBillete(t) is not null ? [(t, "el equipo del billete")] : null;
    }

    internal async Task<IReadOnlyList<long>> EnviarRutasAsync(IReadOnlyList<string> rutas)
    {
        if (_beam is not { } beam || rutas.Count == 0) return [];
        if (Objetivo() is not { Count: > 0 } destinos)
        {
            Avisar("Elige primero a quién enviar.", InfoBarSeverity.Informational);
            return [];
        }
        if (destinos.Count > 8)
        {
            Avisar("Se puede enviar a 8 equipos a la vez como mucho.", InfoBarSeverity.Informational);
            return [];
        }
        // Recorrer carpetas y leer tamaños puede tardar (disco lento, miles de archivos): fuera del hilo de la interfaz.
        (List<ArchivoAEnviar> Lista, long Total, string? Error) r = await Task.Run(() => Reunir(rutas));
        if (r.Error is not null)
        {
            Avisar(r.Error, InfoBarSeverity.Warning);
            return [];
        }
        try
        {
            IReadOnlyList<long> ids = destinos.Count == 1
                ? [await beam.OfrecerAsync(destinos[0].A, r.Lista)]
                : await beam.OfrecerVariosAsync([.. destinos.Select(d => d.A)], r.Lista);
            for (int i = 0; i < ids.Count; i++)
            {
                Insertar(new Transferencia(ids[i], false, $"Para {destinos[i].Alias}: {Texto.Archivos(r.Lista.Count)}, {Texto.Tamano(r.Total)}", r.Total)
                {
                    Estado = "Esperando a que lo acepte…",
                    Archivos = r.Lista.Count,
                    Aceptados = r.Lista.Count,
                });
            }
            return ids;
        }
        catch (BeamException ex)
        {
            Avisar("No se pudo enviar: " + ex.Message, InfoBarSeverity.Error);
            return [];
        }
    }

    private static (List<ArchivoAEnviar>, long, string?) Reunir(IReadOnlyList<string> rutas)
    {
        List<ArchivoAEnviar> lista = [];
        long total = 0;
        try
        {
            foreach (string r in rutas)
            {
                if (Directory.Exists(r)) lista.AddRange(Beam.DeCarpeta(r));
                else lista.Add(new ArchivoAEnviar(r, Path.GetFileName(r)));
                if (lista.Count > MaxArchivos)
                    return (lista, 0, $"Son más de {MaxArchivos} archivos. Comprímelos o envíalos en varias veces.");
            }
            foreach (ArchivoAEnviar x in lista) total += new FileInfo(x.Ruta).Length;
        }
        catch (Exception ex) when (ex is ArgumentException or IOException or UnauthorizedAccessException)
        {
            return (lista, 0, ex.Message);
        }
        return lista.Count == 0 ? (lista, 0, "No hay archivos que enviar (las carpetas vacías no se envían).") : (lista, total, null);
    }

    // ---------- Recibir ----------

    private void AlRecibirOferta(Oferta o)
    {
        Destino? d = Destinos.FirstOrDefault(x => x.Clave == o.De);
        if (d is { Confianza: true })
        {
            _ = AceptarAsync(o, [.. Enumerable.Range(0, o.Archivos.Count)], "Guardado sin preguntar");
            return;
        }
        _pendientes.Enqueue(o);
        Parpadeo.SiNoEstaAlFrente(WinRT.Interop.WindowNative.GetWindowHandle(this));
        if (!_hayDialogo) _ = MostrarOfertasAsync();
    }

    private async Task MostrarOfertasAsync()
    {
        _hayDialogo = true;
        try
        {
            while (_pendientes.TryDequeue(out Oferta? o))
            {
                ListView archivos = new() { SelectionMode = ListViewSelectionMode.Multiple, MaxHeight = 260 };
                foreach (ArchivoOfrecido a in o.Archivos) archivos.Items.Add($"{a.Nombre}  ·  {Texto.Tamano(a.Tamano)}");
                archivos.SelectAll();
                CheckBox confiar = new() { Content = "Guardar lo suyo sin preguntar a partir de ahora" };
                StackPanel cuerpo = new() { Spacing = 8 };
                cuerpo.Children.Add(new TextBlock { Text = $"Quiere enviarte {Texto.Archivos(o.Archivos.Count)} ({Texto.Tamano(o.Total)}).", TextWrapping = TextWrapping.Wrap });
                cuerpo.Children.Add(new TextBlock { Text = "Huella del equipo: " + Texto.Huella(o.De), TextWrapping = TextWrapping.Wrap, Opacity = 0.7, IsTextSelectionEnabled = true });
                cuerpo.Children.Add(archivos);
                cuerpo.Children.Add(new TextBlock { Text = "Se guardará en " + _ajustes.Carpeta, TextWrapping = TextWrapping.Wrap, Opacity = 0.7 });
                cuerpo.Children.Add(confiar);
                ContentDialog dialogo = new()
                {
                    XamlRoot = Raiz.XamlRoot,
                    Title = $"{o.Alias} quiere enviarte archivos",
                    Content = cuerpo,
                    PrimaryButtonText = "Aceptar",
                    CloseButtonText = "Rechazar",
                    DefaultButton = ContentDialogButton.Primary,
                };
                _dialogo = dialogo;
                archivos.SelectionChanged += (_, _) => dialogo.IsPrimaryButtonEnabled = archivos.SelectedItems.Count > 0;
                _ofertaEnDialogo = o.Id;
                ContentDialogResult r = await dialogo.ShowAsync();
                bool retirada = _ofertaEnDialogo == -1;
                _ofertaEnDialogo = -1;
                _dialogo = null;
                if (retirada) continue;
                if (r == ContentDialogResult.Primary)
                {
                    List<int> elegidos = [.. Enumerable.Range(0, o.Archivos.Count).Where(i => archivos.SelectedRanges.Any(g => i >= g.FirstIndex && i <= g.LastIndex))];
                    if (confiar.IsChecked == true) Confiar(o);
                    await AceptarAsync(o, elegidos, "Recibiendo…");
                }
                else
                {
                    await RechazarAsync(o.Id);
                }
            }
        }
        finally
        {
            _hayDialogo = false;
            _dialogo = null;
            _ofertaEnDialogo = -1;
        }
    }

    private async Task RechazarAsync(long oferta)
    {
        try { if (_beam is { } b) await b.RechazarAsync(oferta); }
        catch (BeamException) { /* ya caducó o se retiró */ }
    }

    private void Confiar(Oferta o)
    {
        Destino? d = Destinos.FirstOrDefault(x => x.Clave == o.De);
        if (d is null) Destinos.Add(d = new Destino(o.De) { Alias = o.Alias });
        d.Guardado = d.Confianza = true;
        GuardarEquipos();
        ActualizarZona();
    }

    private async Task AceptarAsync(Oferta o, List<int> elegidos, string estado)
    {
        if (_beam is not { } beam) return;
        long total = elegidos.Sum(i => o.Archivos[i].Tamano);
        string carpeta = _ajustes.Carpeta;
        Transferencia t = new(o.Id, true, $"De {o.Alias}: {Texto.Archivos(elegidos.Count)}, {Texto.Tamano(total)}", total) { Estado = estado, Carpeta = carpeta };
        Insertar(t);
        try
        {
            Directory.CreateDirectory(carpeta);
            await beam.ResponderAsync(o.Id, elegidos, carpeta);
        }
        catch (Exception ex) when (ex is BeamException or IOException or UnauthorizedAccessException)
        {
            t.Activa = false;
            t.Fallo = true;
            t.Progreso = 100;
            t.Estado = ex is BeamException { Codigo: "espacio" } ? "No hay espacio libre suficiente en el disco" : "No se pudo aceptar: " + ex.Message;
            // Sin respuesta, la oferta seguiría ocupando un hueco de los 4 hasta caducar; así el que envía se entera ya.
            await RechazarAsync(o.Id);
        }
    }

    private void AlRetirarOferta(long id)
    {
        if (_ofertaEnDialogo == id)
        {
            _ofertaEnDialogo = -1;
            _dialogo?.Hide();
            Avisar("Quien enviaba lo canceló.", InfoBarSeverity.Informational);
        }
        else if (_pendientes.Any(x => x.Id == id))
        {
            Oferta[] resto = [.. _pendientes.Where(x => x.Id != id)];
            _pendientes.Clear();
            foreach (Oferta x in resto) _pendientes.Enqueue(x);
        }
    }

    // ---------- Avance ----------

    private void Insertar(Transferencia t)
    {
        Transferencias.Insert(0, t);
        if (_sinFila.Remove(t.Id, out List<object>? eventos))
            foreach (object e in eventos) AlEvento(t.Id, e);
    }

    private void AlEvento(long id, object e)
    {
        Transferencia? t = Transferencias.FirstOrDefault(x => x.Id == id);
        if (t is null)
        {
            if (_sinFila.Count < MaxSinFila || _sinFila.ContainsKey(id))
            {
                if (!_sinFila.TryGetValue(id, out List<object>? l)) _sinFila[id] = l = [];
                if (l.Count < 64) l.Add(e);
            }
            return;
        }
        switch (e)
        {
            case Respuesta r: AlResponder(t, r); break;
            case Avance a: AlAvanzar(t, a); break;
            case Fin f: AlTerminar(t, f); break;
        }
    }

    private static void AlResponder(Transferencia t, Respuesta r)
    {
        if (r.Rechazada) return; // el rechazo llega también como fin
        t.Aceptados = r.Aceptados.Count;
        t.Estado = t.Aceptados == t.Archivos ? "Enviando…" : $"Aceptó {t.Aceptados} de {t.Archivos}; enviando…";
    }

    private static void AlAvanzar(Transferencia t, Avance a)
    {
        if (!t.Activa) return;
        DateTime ahora = DateTime.UtcNow;
        double seg = (ahora - t.CuandoAntes).TotalSeconds;
        if (t.CuandoAntes != default && seg > 0.2)
        {
            double ritmo = Math.Max(0, a.Bytes - t.BytesAntes) / seg;
            t.Ritmo = t.Ritmo == 0 ? ritmo : 0.7 * t.Ritmo + 0.3 * ritmo; // suavizado para que no salte
        }
        if (t.CuandoAntes == default || seg > 0.2)
        {
            t.BytesAntes = a.Bytes;
            t.CuandoAntes = ahora;
        }
        t.Total = a.Total;
        t.Progreso = a.Total == 0 ? 0 : 100.0 * a.Bytes / a.Total;
        string ritmoTexto = t.Ritmo > 0 ? $" · {Texto.Tamano((long)t.Ritmo)}/s · quedan {Texto.Duracion((a.Total - a.Bytes) / t.Ritmo)}" : "";
        t.Estado = $"{Texto.Tamano(a.Bytes)} de {Texto.Tamano(a.Total)}{ritmoTexto}";
    }

    private static void AlTerminar(Transferencia t, Fin f)
    {
        t.Activa = false;
        t.Ok = f.Ok;
        if (f.Ok)
        {
            t.Progreso = 100;
            t.Estado = t.Entrante ? $"Recibido en {t.Carpeta}"
                : t.Aceptados < t.Archivos ? $"Enviado: aceptó {t.Aceptados} de {t.Archivos}" : "Enviado";
            if (t.Entrante && f.Rutas.Count > 0) t.Mostrar = f.Rutas[0];
        }
        else
        {
            t.Fallo = true;
            t.Progreso = 100; // barra entera en rojo: se ve de un vistazo que no salió
            t.Estado = Texto.Error(f.Error);
        }
    }

    private async void Cancelar_Click(object sender, RoutedEventArgs e)
    {
        if ((sender as FrameworkElement)?.Tag is not Transferencia t || _beam is not { } b) return;
        try { await b.CancelarAsync(t.Id); }
        catch (BeamException) { /* ya había terminado */ }
    }

    private void Mostrar_Click(object sender, RoutedEventArgs e)
    {
        if ((sender as FrameworkElement)?.Tag is not Transferencia { Mostrar: { } ruta }) return;
        Process.Start(new ProcessStartInfo("explorer.exe") { ArgumentList = { "/select," + ruta } });
    }

    // ---------- Ajustes ----------

    private async void AbrirAjustes_Click(object sender, RoutedEventArgs e)
    {
        if (_hayDialogo) return;
        TextBox alias = new() { Header = "Nombre con el que te ven los demás", Text = _ajustes.Alias, MaxLength = 64 };
        TextBox carpeta = new() { Header = "Carpeta donde se guarda lo recibido", Text = _ajustes.Carpeta };
        ComboBox rele = new() { Header = "Conexión", HorizontalAlignment = HorizontalAlignment.Stretch };
        rele.Items.Add("Tu red e internet (relé público de n0 para encontrarse)");
        rele.Items.Add("Sin relé: tu red y billetes con dirección directa");
        rele.Items.Add("Tu red e internet con tu propio relé");
        rele.SelectedIndex = _ajustes.Rele == "ninguno" ? 1 : EsUrl(_ajustes.Rele) ? 2 : 0;
        TextBox url = new() { Header = "Dirección de tu relé", PlaceholderText = "https://rele.ejemplo.com", Text = EsUrl(_ajustes.Rele) ? _ajustes.Rele : "" };
        CheckBox soloRele = new() { Content = "Que todo pase por mi relé (oculta mi IP)", IsChecked = _ajustes.SoloRele };
        void Propio() => url.Visibility = soloRele.Visibility = rele.SelectedIndex == 2 ? Visibility.Visible : Visibility.Collapsed;
        rele.SelectionChanged += (_, _) => Propio();
        Propio();
        NumberBox limite = new()
        {
            Header = "Límite de subida en MB/s (0, sin límite)",
            Value = _ajustes.LimiteSubidaMBs,
            Minimum = 0,
            Maximum = 10_000,
            SpinButtonPlacementMode = NumberBoxSpinButtonPlacementMode.Compact,
        };
        StackPanel cuerpo = new() { Spacing = 12, MinWidth = 420 };
        cuerpo.Children.Add(alias);
        cuerpo.Children.Add(carpeta);
        cuerpo.Children.Add(rele);
        cuerpo.Children.Add(url);
        cuerpo.Children.Add(soloRele);
        cuerpo.Children.Add(limite);
        ContentDialog d = new()
        {
            XamlRoot = Raiz.XamlRoot,
            Title = "Ajustes",
            Content = cuerpo,
            PrimaryButtonText = "Guardar",
            CloseButtonText = "Cancelar",
            DefaultButton = ContentDialogButton.Primary,
        };
        ContentDialogResult r;
        _hayDialogo = true;
        try
        {
            r = await d.ShowAsync();
        }
        finally
        {
            _hayDialogo = false;
        }
        if (r == ContentDialogResult.Primary)
        {
            string nuevo = rele.SelectedIndex switch { 1 => "ninguno", 2 => url.Text.Trim(), _ => "n0" };
            if (rele.SelectedIndex == 2 && !Uri.TryCreate(nuevo, UriKind.Absolute, out _) | !EsUrl(nuevo))
            {
                Avisar("La dirección del relé tiene que empezar por https://. No se cambió la conexión.", InfoBarSeverity.Warning);
                nuevo = _ajustes.Rele;
            }
            double mbs = double.IsNaN(limite.Value) ? 0 : Math.Max(0, limite.Value);
            await AplicarAjustesAsync(alias.Text.Trim(), carpeta.Text.Trim(), nuevo, soloRele.IsChecked == true, mbs);
        }
        if (_pendientes.Count > 0 && !_hayDialogo) _ = MostrarOfertasAsync(); // ofertas llegadas con Ajustes abierto
    }

    private async Task AplicarAjustesAsync(string alias, string carpeta, string rele, bool soloRele, double limiteMBs)
    {
        bool reabrir = (alias.Length > 0 && alias != _ajustes.Alias) || rele != _ajustes.Rele
            || soloRele != _ajustes.SoloRele || limiteMBs != _ajustes.LimiteSubidaMBs;
        if (alias.Length > 0) _ajustes.Alias = alias;
        if (carpeta.Length > 0) _ajustes.Carpeta = carpeta;
        _ajustes.Rele = rele;
        _ajustes.SoloRele = soloRele;
        _ajustes.LimiteSubidaMBs = limiteMBs;
        GuardarEquipos();
        if (!reabrir) return;
        if (Transferencias.Any(t => t.Activa) || _pendientes.Count > 0)
        {
            Avisar("El nombre, la conexión y el límite se aplican al volver a abrir BeamLine, para no cortar lo que está en curso.", InfoBarSeverity.Informational);
            return;
        }
        await CerrarAsync();
        _sinFila.Clear();
        foreach (Destino x in Destinos.Where(x => x.Cercano).ToList()) AlCambiarCercano(new Cercano(x.Clave, x.Alias, false));
        await AbrirBeamAsync();
    }

    private void Avisar(string texto, InfoBarSeverity nivel)
    {
        Aviso.Message = texto;
        Aviso.Severity = nivel;
        Aviso.IsOpen = true;
    }

    // ---------- Para la autoprueba ----------

    internal Beam? Red => _beam;
    internal Ajustes AjustesActuales => _ajustes;

    internal void AnadirConfianza(string clave, string billete, string alias)
    {
        Destino d = new(clave) { Alias = alias, Billete = billete, Guardado = true, Confianza = true };
        Destinos.Add(d);
        ListaDestinos.SelectedItems.Add(d);
        ActualizarZona();
    }
}
