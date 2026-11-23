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
    private readonly Arranque _arranque;
    private readonly string _archivoAjustes;
    private readonly Ajustes _ajustes;
    private readonly string _identidad;
    private readonly DispatcherQueue _hilo;
    private readonly DispatcherQueueTimer _reloj;
    private readonly Queue<Oferta> _pendientes = new();
    private readonly Dictionary<long, string> _destinoDeOferta = [];
    private Beam? _beam;
    private ContentDialog? _dialogo;
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
            Destinos.Add(new Destino(g.Clave) { Alias = g.Alias, Billete = g.Billete, Guardado = true, Confianza = g.Confianza });

        _reloj = _hilo.CreateTimer();
        _reloj.Interval = TimeSpan.FromSeconds(5);
        _reloj.Tick += async (_, _) => await RefrescarEstadoAsync();
        Closed += (_, _) => Cerrar();
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
                    new OpcionesRed(_ajustes.Rele, Mdns: !autoprueba)));
            b.OfertaRecibida += o => EnHilo(() => AlRecibirOferta(o));
            b.OfertaRetirada += id => EnHilo(() => AlRetirarOferta(id));
            b.Respondida += r => EnHilo(() => AlResponder(r));
            b.Avanzo += a => EnHilo(() => AlAvanzar(a));
            b.Termino += f => EnHilo(() => AlTerminar(f));
            b.CercanoCambio += c => EnHilo(() => AlCambiarCercano(c));
            b.Caido += e => EnHilo(() => { MiEstado.Text = "La red se detuvo: " + e.Message; Avisar("Cierra y vuelve a abrir BeamLine.", InfoBarSeverity.Error); });
            b.ManejadorFallo += e => Debug.WriteLine(e);
            _beam = b;
            _reloj.Start();
            await RefrescarEstadoAsync();
        }
        catch (BeamException e)
        {
            MiEstado.Text = "Sin red";
            Avisar("No se pudo abrir la red: " + e.Message, InfoBarSeverity.Error);
        }
    }

    private async Task RefrescarEstadoAsync()
    {
        if (_beam is null) return;
        try
        {
            Estado e = await _beam.EstadoAsync();
            MiEstado.Text = _ajustes.Rele == "ninguno" ? "Solo tu red y billetes con dirección directa"
                : e.ReleConectado ? "Conectado: tu red e internet" : "Solo tu red (sin conexión con el relé)";
        }
        catch (BeamException)
        {
            // Caido ya lo avisa.
        }
    }

    private void EnHilo(Action a) => _hilo.TryEnqueue(() => a());

    private void Cerrar()
    {
        _reloj.Stop();
        Beam? b = Interlocked.Exchange(ref _beam, null);
        // beamline-red se cierra solo si muere la app; esto solo le deja guardar en orden.
        b?.DisposeAsync().AsTask().Wait(TimeSpan.FromSeconds(3));
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

    private Destino? Elegido => ListaDestinos.SelectedItem as Destino;

    private void ListaDestinos_SelectionChanged(object sender, SelectionChangedEventArgs e) => ActualizarZona();

    private void ActualizarZona()
    {
        SinEquipos.Visibility = Destinos.Count == 0 ? Visibility.Visible : Visibility.Collapsed;
        string? a = Elegido?.Alias ?? (BilleteValido(CajaBillete.Text) ? "el equipo del billete" : null);
        TextoZona.Text = a is null ? "Elige a la izquierda a quién enviar" : $"Suelta aquí archivos o carpetas para enviarlos a {a}";
        ElegirArchivos.IsEnabled = ElegirCarpeta.IsEnabled = a is not null && _beam is not null;
    }

    private static bool BilleteValido(string t) => t.Trim().StartsWith("beam1", StringComparison.Ordinal) && t.Trim().Length > 10;

    private void CajaBillete_TextChanged(object sender, TextChangedEventArgs e)
    {
        GuardarBillete.IsEnabled = BilleteValido(CajaBillete.Text);
        if (GuardarBillete.IsEnabled) ListaDestinos.SelectedItem = null;
        ActualizarZona();
    }

    private void GuardarBillete_Click(object sender, RoutedEventArgs e)
    {
        string billete = CajaBillete.Text.Trim();
        // La clave real se conoce cuando ese equipo nos ofrece algo; hasta entonces lo identifica el billete.
        Destino d = new(billete) { Billete = billete, Alias = $"Equipo {Destinos.Count(x => x.Guardado) + 1}", Guardado = true };
        Destinos.Add(d);
        CajaBillete.Text = "";
        ListaDestinos.SelectedItem = d;
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
        catch (IOException ex) { Avisar("No se pudieron guardar los ajustes: " + ex.Message, InfoBarSeverity.Warning); }
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

    private (string A, string Alias)? Objetivo()
    {
        if (Elegido is { } d) return (d.A, d.Alias);
        string t = CajaBillete.Text.Trim();
        return BilleteValido(t) ? (t, "el equipo del billete") : null;
    }

    internal async Task<long?> EnviarRutasAsync(IReadOnlyList<string> rutas)
    {
        if (_beam is null || rutas.Count == 0) return null;
        if (Objetivo() is not ({ } a, { } alias))
        {
            Avisar("Elige primero a quién enviar.", InfoBarSeverity.Informational);
            return null;
        }
        List<ArchivoAEnviar> lista = [];
        try
        {
            foreach (string r in rutas)
            {
                if (Directory.Exists(r)) lista.AddRange(Beam.DeCarpeta(r));
                else lista.Add(new ArchivoAEnviar(r, Path.GetFileName(r)));
            }
        }
        catch (Exception ex) when (ex is ArgumentException or IOException or UnauthorizedAccessException)
        {
            Avisar(ex.Message, InfoBarSeverity.Warning);
            return null;
        }
        if (lista.Count == 0)
        {
            Avisar("No hay archivos que enviar (las carpetas vacías no se envían).", InfoBarSeverity.Informational);
            return null;
        }
        long total = lista.Sum(x => new FileInfo(x.Ruta).Length);
        try
        {
            long id = await _beam.OfrecerAsync(a, lista);
            Transferencias.Insert(0, new Transferencia(id, false, $"Para {alias}: {Texto.Archivos(lista.Count)}, {Texto.Tamano(total)}", total)
            {
                Estado = "Esperando a que lo acepte…",
            });
            return id;
        }
        catch (BeamException ex)
        {
            Avisar("No se pudo enviar: " + ex.Message, InfoBarSeverity.Error);
            return null;
        }
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
        if (_dialogo is null) _ = MostrarOfertasAsync();
    }

    private async Task MostrarOfertasAsync()
    {
        while (_pendientes.TryDequeue(out Oferta? o))
        {
            ListView archivos = new() { SelectionMode = ListViewSelectionMode.Multiple, MaxHeight = 260 };
            foreach (ArchivoOfrecido a in o.Archivos) archivos.Items.Add($"{a.Nombre}  ·  {Texto.Tamano(a.Tamano)}");
            archivos.SelectAll();
            CheckBox confiar = new() { Content = "Guardar lo suyo sin preguntar a partir de ahora" };
            string corta = o.De.Length > 8 ? o.De[..8] : o.De;
            StackPanel cuerpo = new() { Spacing = 8 };
            cuerpo.Children.Add(new TextBlock { Text = $"Quiere enviarte {Texto.Archivos(o.Archivos.Count)} ({Texto.Tamano(o.Total)}). Equipo {corta}.", TextWrapping = TextWrapping.Wrap });
            cuerpo.Children.Add(archivos);
            cuerpo.Children.Add(new TextBlock { Text = "Se guardará en " + _ajustes.Carpeta, TextWrapping = TextWrapping.Wrap, Opacity = 0.7 });
            cuerpo.Children.Add(confiar);
            _dialogo = new ContentDialog
            {
                XamlRoot = Raiz.XamlRoot,
                Title = $"{o.Alias} quiere enviarte archivos",
                Content = cuerpo,
                PrimaryButtonText = "Aceptar",
                CloseButtonText = "Rechazar",
                DefaultButton = ContentDialogButton.Primary,
            };
            archivos.SelectionChanged += (_, _) => _dialogo.IsPrimaryButtonEnabled = archivos.SelectedItems.Count > 0;
            _ofertaEnDialogo = o.Id;
            ContentDialogResult r = await _dialogo.ShowAsync();
            bool retirada = _ofertaEnDialogo == -1;
            _ofertaEnDialogo = -1;
            if (retirada) continue;
            if (r == ContentDialogResult.Primary)
            {
                List<int> elegidos = [.. Enumerable.Range(0, o.Archivos.Count).Where(i => archivos.SelectedRanges.Any(g => i >= g.FirstIndex && i <= g.LastIndex))];
                if (confiar.IsChecked == true) Confiar(o);
                await AceptarAsync(o, elegidos, "Recibiendo…");
            }
            else
            {
                try { if (_beam is not null) await _beam.RechazarAsync(o.Id); }
                catch (BeamException) { /* ya caducó o se retiró */ }
            }
        }
        _dialogo = null;
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
        if (_beam is null) return;
        long total = elegidos.Sum(i => o.Archivos[i].Tamano);
        Transferencia t = new(o.Id, true, $"De {o.Alias}: {Texto.Archivos(elegidos.Count)}, {Texto.Tamano(total)}", total) { Estado = estado };
        Transferencias.Insert(0, t);
        try
        {
            Directory.CreateDirectory(_ajustes.Carpeta);
            await _beam.ResponderAsync(o.Id, elegidos, _ajustes.Carpeta);
        }
        catch (Exception ex) when (ex is BeamException or IOException or UnauthorizedAccessException)
        {
            t.Activa = false;
            t.Fallo = true;
            t.Estado = ex is BeamException { Codigo: "espacio" } ? "No hay espacio libre suficiente en el disco" : "No se pudo aceptar: " + ex.Message;
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

    private Transferencia? Buscar(long id) => Transferencias.FirstOrDefault(t => t.Id == id);

    private void AlResponder(Respuesta r)
    {
        if (Buscar(r.Envio) is not { } t || r.Rechazada) return; // el rechazo llega también como fin
        t.Estado = "Enviando…";
    }

    private void AlAvanzar(Avance a)
    {
        if (Buscar(a.Id) is not { } t) return;
        t.Total = a.Total;
        t.Progreso = a.Total == 0 ? 0 : 100.0 * a.Bytes / a.Total;
        t.Estado = $"{Texto.Tamano(a.Bytes)} de {Texto.Tamano(a.Total)}";
    }

    private void AlTerminar(Fin f)
    {
        if (Buscar(f.Id) is not { } t) return;
        t.Activa = false;
        t.Ok = f.Ok;
        if (f.Ok)
        {
            t.Progreso = 100;
            t.Estado = t.Entrante ? $"Recibido en {_ajustes.Carpeta}" : "Enviado";
            if (t.Entrante && f.Rutas.Count > 0) t.Mostrar = f.Rutas[0];
        }
        else
        {
            t.Fallo = true;
            t.Estado = Texto.Error(f.Error);
        }
    }

    private async void Cancelar_Click(object sender, RoutedEventArgs e)
    {
        if ((sender as FrameworkElement)?.Tag is not Transferencia t || _beam is null) return;
        try { await _beam.CancelarAsync(t.Id); }
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
        TextBox alias = new() { Header = "Nombre con el que te ven los demás", Text = _ajustes.Alias, MaxLength = 64 };
        TextBox carpeta = new() { Header = "Carpeta donde se guarda lo recibido", Text = _ajustes.Carpeta };
        ComboBox rele = new() { Header = "Conexión", HorizontalAlignment = HorizontalAlignment.Stretch };
        rele.Items.Add("Tu red e internet (relé público de n0 para encontrarse)");
        rele.Items.Add("Solo tu red local");
        rele.SelectedIndex = _ajustes.Rele == "ninguno" ? 1 : 0;
        StackPanel cuerpo = new() { Spacing = 12, MinWidth = 420 };
        cuerpo.Children.Add(alias);
        cuerpo.Children.Add(carpeta);
        cuerpo.Children.Add(rele);
        ContentDialog d = new()
        {
            XamlRoot = Raiz.XamlRoot,
            Title = "Ajustes",
            Content = cuerpo,
            PrimaryButtonText = "Guardar",
            CloseButtonText = "Cancelar",
            DefaultButton = ContentDialogButton.Primary,
        };
        if (_dialogo is not null || await d.ShowAsync() != ContentDialogResult.Primary) return;

        string nuevoRele = rele.SelectedIndex == 1 ? "ninguno" : "n0";
        bool reabrir = alias.Text.Trim() != _ajustes.Alias || nuevoRele != _ajustes.Rele;
        if (alias.Text.Trim().Length > 0) _ajustes.Alias = alias.Text.Trim();
        if (carpeta.Text.Trim().Length > 0) _ajustes.Carpeta = carpeta.Text.Trim();
        _ajustes.Rele = nuevoRele;
        GuardarEquipos();
        if (!reabrir) return;
        if (Transferencias.Any(t => t.Activa))
        {
            Avisar("El nombre y la conexión se aplican al volver a abrir BeamLine, para no cortar lo que está en curso.", InfoBarSeverity.Informational);
            return;
        }
        Cerrar();
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
        ListaDestinos.SelectedItem = d;
        ActualizarZona();
    }
}
