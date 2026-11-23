# Licencias de terceros

| Componente | Versión | Licencia |
|---|---|---|
| Sala (submódulo: `dathinel-red` como biblioteca, `Dathinel.Sala`, `Rodaje.Modules.Protocol`) | `main` | MIT |
| iroh | 1.4.1 | MIT o Apache-2.0 |
| iroh-blobs | 0.105.0 | MIT o Apache-2.0 |
| iroh-mdns-address-lookup | 0.6.1 | MIT o Apache-2.0 |
| fs4 | 0.13 | MIT o Apache-2.0 |
| tokio, tokio-util, serde, serde_json, n0-future | — | MIT (o MIT o Apache-2.0) |
| base64 | 0.22 | MIT o Apache-2.0 |
| blake3 (solo la PoC) | 1.x | CC0-1.0, Apache-2.0 o Apache-2.0 con excepción LLVM |
| xunit, Microsoft.NET.Test.Sdk (solo las pruebas) | — | Apache-2.0 o MIT |
| .NET (runtime en el exe con Native AOT) | 10.0 | MIT |
| Windows App SDK / WinUI 3 (solo Windows) | 2.3 | MIT |
| Uno Platform: Uno.WinUI, Uno.Sdk (solo Linux) | 7.0.65 / 7.0.22 | Apache-2.0 |
| Uno.Fonts.OpenSans, Uno.Fonts.Fluent (solo Linux) | 2.10.1 / 2.9.4 | Apache-2.0 |
| SkiaSharp, HarfBuzzSharp (solo Linux, con `libSkiaSharp.so` y `libHarfBuzzSharp.so`) | 4.156.0 / 14.4.0.100 | MIT |

**Rust:** `cd red; cargo deny check` (política en `red/deny.toml`, la de Sala: solo licencias permisivas) pasa entero el 23-nov: licencias, avisos, prohibidas y orígenes. `cargo deny list` da la lista completa por crate. Las excepciones de avisos (crates sin mantenimiento, sin vulnerabilidad) llevan su motivo en `deny.toml`.
