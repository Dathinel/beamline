//! Prueba de concepto de la fase 0 (docs/plan.md): medir iroh-blobs y comprobar la reanudación.
use std::{path::{Path, PathBuf}, sync::OnceLock, time::Instant};

use anyhow::{Context, Result, bail, ensure};
use iroh::{Endpoint, EndpointAddr, RelayMode, RelayUrl, address_lookup::MemoryLookup, endpoint::presets, protocol::Router};
use iroh_blobs::{
    BlobFormat, BlobsProtocol, Hash,
    api::{blobs::{AddPathOptions, ImportMode}, downloader::DownloadProgressItem},
    store::fs::FsStore,
    ticket::BlobTicket,
};
use n0_future::StreamExt;

/// Opciones de red comunes: `--rele <url>`, `--solo-rele` y, solo con `--features prueba`, `--inseguro` para el relé local de Sala.
#[derive(Default)]
struct Red {
    rele: Option<RelayUrl>,
    solo_rele: bool,
    inseguro: bool,
}
static RED: OnceLock<Red> = OnceLock::new();

#[tokio::main]
async fn main() -> Result<()> {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    let mut red = Red::default();
    if let Some(i) = args.iter().position(|a| a == "--rele") {
        red.rele = Some(args.get(i + 1).context("--rele necesita una URL")?.parse()?);
        args.drain(i..=i + 1);
    }
    red.solo_rele = quitar(&mut args, "--solo-rele");
    red.inseguro = quitar(&mut args, "--inseguro");
    ensure!(!red.inseguro || cfg!(feature = "prueba"), "--inseguro solo existe con --features prueba");
    RED.set(red).ok();
    let a: Vec<&str> = args.iter().map(String::as_str).collect();
    match a.as_slice() {
        ["enviar", archivo, almacen] => enviar(archivo.as_ref(), almacen.as_ref()).await,
        ["recibir", billete, almacen, destino] => recibir(billete.parse()?, almacen.as_ref(), destino.as_ref()).await,
        ["local", archivo, carpeta] => local(archivo.as_ref(), carpeta.as_ref()).await,
        _ => {
            eprintln!("uso:\n  beamline-poc enviar <archivo> <almacén>\n  beamline-poc recibir <billete> <almacén> <destino>\n  beamline-poc local <archivo> <carpeta de trabajo>");
            std::process::exit(2)
        }
    }
}

fn quitar(args: &mut Vec<String>, opcion: &str) -> bool {
    let antes = args.len();
    args.retain(|a| a != opcion);
    args.len() != antes
}

async fn importar(store: &FsStore, archivo: &Path) -> Result<Hash> {
    let t = Instant::now();
    let tag = store
        .blobs()
        .add_path_with_opts(AddPathOptions {
            path: std::path::absolute(archivo)?,
            format: BlobFormat::Raw,
            mode: ImportMode::TryReference, // no copia el archivo al almacén
        })
        .await?;
    eprintln!("huella BLAKE3 en {:.1} s", t.elapsed().as_secs_f64());
    Ok(tag.hash)
}

/// Endpoint con relé de n0 o solo local; si se da el par, sus direcciones del billete se conocen sin buscarlas.
async fn endpoint(relay: bool, par: Option<&EndpointAddr>) -> Result<Endpoint> {
    let memoria = MemoryLookup::new();
    if let Some(p) = par {
        memoria.add_endpoint_info(p.clone());
    }
    let red = RED.get_or_init(Red::default);
    let mut b = if relay {
        match &red.rele {
            Some(u) => Endpoint::builder(presets::Minimal).relay_mode(RelayMode::Custom(u.clone().into())),
            None => Endpoint::builder(presets::N0),
        }
    } else {
        Endpoint::builder(presets::Minimal).relay_mode(RelayMode::Disabled)
    };
    if relay && red.solo_rele {
        b = b.clear_ip_transports();
    }
    #[cfg(feature = "prueba")]
    if red.inseguro {
        b = b.ca_tls_config(iroh::tls::CaTlsConfig::insecure_skip_verify());
    }
    Ok(b.address_lookup(memoria).bind().await?)
}

async fn servidor(store: &FsStore, relay: bool) -> Result<Router> {
    let ep = endpoint(relay, None).await?;
    Ok(Router::builder(ep).accept(iroh_blobs::ALPN, BlobsProtocol::new(store, None)).spawn())
}

async fn enviar(archivo: &Path, almacen: &Path) -> Result<()> {
    let store = FsStore::load(almacen).await?;
    let hash = importar(&store, archivo).await?;
    let router = servidor(&store, true).await?;
    router.endpoint().online().await;
    println!("{}", BlobTicket::new(router.endpoint().addr(), hash, BlobFormat::Raw));
    eprintln!("sirviendo; Ctrl+C para terminar");
    tokio::signal::ctrl_c().await?;
    router.shutdown().await?;
    Ok(())
}

/// Descarga hasta terminar o hasta tener `corte` bytes; devuelve (bytes que ya había, bytes bajados).
async fn bajar(store: &FsStore, ep: &Endpoint, billete: &BlobTicket, corte: Option<u64>) -> Result<(u64, u64)> {
    let previos = store.remote().local(billete.hash_and_format()).await?.local_bytes();
    let t = Instant::now();
    let mut s = store.downloader(ep).download(billete.hash(), Some(billete.addr().id)).stream().await?;
    let mut bajados = 0;
    while let Some(item) = s.next().await {
        match item {
            // Progress cuenta desde el principio del blob, incluido lo que ya había.
            DownloadProgressItem::Progress(n) => {
                bajados = n.saturating_sub(previos);
                if corte.is_some_and(|c| n >= c) {
                    break;
                }
            }
            DownloadProgressItem::Error(e) => return Err(e.into()),
            DownloadProgressItem::DownloadError => bail!("error de descarga"),
            _ => {}
        }
    }
    let seg = t.elapsed().as_secs_f64();
    eprintln!(
        "ya había {:.1} MiB; bajados {:.1} MiB en {seg:.1} s = {:.2} MiB/s ({:.1} Mbit/s)",
        mib(previos), mib(bajados), mib(bajados) / seg, bajados as f64 * 8.0 / 1e6 / seg
    );
    Ok((previos, bajados))
}

async fn recibir(billete: BlobTicket, almacen: &Path, destino: &Path) -> Result<()> {
    let store = FsStore::load(almacen).await?;
    let ep = endpoint(true, Some(billete.addr())).await?;
    bajar(&store, &ep, &billete, None).await?;
    store.blobs().export(billete.hash(), std::path::absolute(destino)?).await?;
    eprintln!("guardado en {}", destino.display());
    ep.close().await;
    Ok(())
}

/// Autoprueba en un solo equipo: corta la descarga a la mitad, la reanuda y comprueba el resultado.
async fn local(archivo: &Path, carpeta: &Path) -> Result<()> {
    let total = std::fs::metadata(archivo).context("archivo")?.len();
    let emisor = FsStore::load(carpeta.join("emisor")).await?;
    let hash = importar(&emisor, archivo).await?;
    let router = servidor(&emisor, false).await?;
    let billete = BlobTicket::new(router.endpoint().addr(), hash, BlobFormat::Raw);

    let dir_r = carpeta.join("receptor");
    let receptor = FsStore::load(&dir_r).await?;
    let ep = endpoint(false, Some(billete.addr())).await?;
    bajar(&receptor, &ep, &billete, Some(total / 2)).await?;
    receptor.shutdown().await?;
    ep.close().await;

    // Proceso nuevo en la práctica: almacén reabierto y endpoint nuevo.
    let receptor = FsStore::load(&dir_r).await?;
    let ep = endpoint(false, Some(billete.addr())).await?;
    let (previos, bajados) = bajar(&receptor, &ep, &billete, None).await?;
    ensure!(previos >= total / 4, "no reanudó: solo había {previos} bytes");
    ensure!(bajados <= total - previos + (1 << 20), "volvió a bajar lo que ya tenía");

    let salida: PathBuf = std::path::absolute(carpeta.join("recibido.bin"))?;
    receptor.blobs().export(hash, &salida).await?;
    ensure!(std::fs::metadata(&salida)?.len() == total, "tamaño distinto");
    let mut h = blake3::Hasher::new();
    h.update_mmap(&salida)?;
    ensure!(h.finalize().as_bytes() == hash.as_bytes(), "huella distinta");
    println!("OK: reanudó desde {:.1} MiB de {:.1} MiB y la huella coincide", mib(previos), mib(total));
    router.shutdown().await?;
    Ok(())
}

fn mib(b: u64) -> f64 {
    b as f64 / (1 << 20) as f64
}
