namespace BeamLine.Nucleo.Tests;

public sealed class DeCarpetaTests
{
    [Fact]
    public void Nombra_con_la_carpeta_y_barras_y_salta_las_vacias()
    {
        string raiz = Path.Combine(Path.GetTempPath(), "beamline-carpeta-" + Guid.NewGuid().ToString("N"), "Fotos");
        Directory.CreateDirectory(Path.Combine(raiz, "b", "c"));
        Directory.CreateDirectory(Path.Combine(raiz, "vacía"));
        File.WriteAllText(Path.Combine(raiz, "b", "c", "z.jpg"), "z");
        File.WriteAllText(Path.Combine(raiz, "a.jpg"), "a");
        try
        {
            IReadOnlyList<ArchivoAEnviar> l = Beam.DeCarpeta(raiz + Path.DirectorySeparatorChar);
            Assert.Equal(["Fotos/a.jpg", "Fotos/b/c/z.jpg"], l.Select(a => a.Nombre));
            Assert.All(l, a => Assert.True(Path.IsPathFullyQualified(a.Ruta)));
        }
        finally
        {
            Directory.Delete(Path.GetDirectoryName(raiz)!, true);
        }
    }
}
