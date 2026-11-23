using Microsoft.UI.Xaml;

namespace BeamLine.App;

public partial class App : Application
{
    private MainWindow? _ventana;

    public App() => InitializeComponent();

    protected override void OnLaunched(LaunchActivatedEventArgs args)
    {
        _ventana = new MainWindow(Arranque.Leer(Environment.GetCommandLineArgs()));
        _ventana.Activate();
    }
}
