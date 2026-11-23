//! Fase 3 de punta a punta: envío a varios con reparto de la subida, límite de subida y «solo relé».

use std::{
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use beamline_red::equipo::{Config, Equipo, Rele};
use serde_json::Value;
use tokio::sync::Notify;

struct Carpeta(PathBuf);

impl Carpeta {
    fn nueva() -> Self {
        let r = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("target")
            .join(format!("prueba-{:016x}", rand::random::<u64>()));
        std::fs::create_dir_all(&r).unwrap();
        Self(r)
    }
}

impl Drop for Carpeta {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn config(dir: &Path, alias: &str) -> Config {
    Config {
        identidad: iroh::SecretKey::generate(),
        alias: alias.into(),
        almacen: dir.join(format!("almacen-{alias}")),
        rele: Rele::Ninguno,
        solo_rele: false,
        mdns: false,
        tamano_max: 1 << 40,
        inseguro: false,
        limpieza: Duration::from_secs(1),
        espera_respuesta: Duration::from_secs(30),
        limite_subida: None,
    }
}

async fn esperar(e: &Equipo, vistos: &mut Vec<Value>, f: impl Fn(&Value) -> bool) -> Value {
    if let Some(i) = vistos.iter().position(&f) {
        return vistos.remove(i);
    }
    let nada = Notify::new();
    let hasta = tokio::time::Instant::now() + Duration::from_secs(120);
    while tokio::time::Instant::now() < hasta {
        vistos.extend(e.recibir(Duration::from_millis(500), &nada).await);
        if let Some(i) = vistos.iter().position(&f) {
            return vistos.remove(i);
        }
    }
    panic!("no llegó el evento; vistos: {vistos:?}");
}

fn aleatorio(ruta: &Path, n: usize) -> Vec<u8> {
    let mut datos = vec![0u8; n];
    rand::fill(&mut datos[..]);
    std::fs::write(ruta, &datos).unwrap();
    datos
}

/// Envía `tamano` bytes a tres a la vez y devuelve (bytes que subió el emisor, bytes que se sirvieron entre ellos).
async fn a_tres(tamano: usize) -> (u64, u64) {
    let c = Carpeta::nueva();
    let a = Equipo::abrir(config(&c.0, "ana")).await.unwrap();
    let mut rs = Vec::new();
    for n in ["beto", "carla", "dani"] {
        rs.push(Equipo::abrir(config(&c.0, n)).await.unwrap());
    }
    let datos = aleatorio(&c.0.join("grande.bin"), tamano);
    let billetes: Vec<String> = rs.iter().map(Equipo::billete).collect();
    let envios = a
        .ofrecer_varios(
            &billetes,
            vec![(c.0.join("grande.bin"), "grande.bin".into())],
        )
        .unwrap();
    assert_eq!(envios.len(), 3);

    let mut vistas: Vec<Vec<Value>> = vec![Vec::new(); 3];
    // Todos deben haber recibido la oferta antes de medir el reparto simultáneo.
    // Mientras un destinatario todavía no ha respondido, los demás pueden recurrir al emisor.
    let mut ofertas = Vec::new();
    for (r, evs) in rs.iter().zip(&mut vistas) {
        let o = esperar(r, evs, |ev| ev["type"] == "oferta").await;
        ofertas.push(o["oferta"].as_u64().unwrap());
    }
    for (k, (r, oferta)) in rs.iter().zip(ofertas).enumerate() {
        r.responder(oferta, vec![0], Some(c.0.join(format!("destino-{k}"))))
            .unwrap();
    }
    let mut va = Vec::new();
    for id in &envios {
        let f = esperar(&a, &mut va, |ev| ev["type"] == "fin" && ev["id"] == *id).await;
        assert_eq!(f["ok"], true, "{f}");
    }
    for k in 0..3 {
        assert_eq!(
            std::fs::read(c.0.join(format!("destino-{k}")).join("grande.bin")).unwrap(),
            datos
        );
    }
    let subidos = a.estado()["subidos"].as_u64().unwrap();
    let de_otros: u64 = rs
        .iter()
        .map(|r| r.estado()["subidos"].as_u64().unwrap())
        .sum();
    a.cerrar().await;
    for r in rs {
        r.cerrar().await;
    }
    (subidos, de_otros)
}

#[tokio::test(flavor = "multi_thread")]
async fn a_tres_a_la_vez_el_emisor_sube_una_vez() {
    const TAMANO: usize = 24 << 20;
    let (subidos, de_otros) = a_tres(TAMANO).await;
    eprintln!("CIFRAS emisor={subidos} receptores={de_otros} tamano={TAMANO}");
    // Lo ideal es 1,0; con margen para algún trozo que se pida dos veces.
    assert!(
        subidos * 10 < TAMANO as u64 * 13,
        "el emisor subió {subidos} bytes de {TAMANO}"
    );
    assert!(
        de_otros > 0,
        "los receptores no se sirvieron nada entre ellos"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_tres_con_un_tamano_que_no_cuadra_con_los_bloques() {
    // El último trozo acaba a mitad de un bloque de 16 KiB: tiene que darse por entero sin esperar los 30 s.
    const TAMANO: usize = (24 << 20) + 777;
    let inicio = Instant::now();
    let (subidos, _) = a_tres(TAMANO).await;
    assert!(
        inicio.elapsed() < Duration::from_secs(25),
        "tardó {:?}",
        inicio.elapsed()
    );
    assert!(
        subidos * 10 < TAMANO as u64 * 13,
        "el emisor subió {subidos} bytes de {TAMANO}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn un_receptor_que_rechaza_no_frena_a_los_demas() {
    let c = Carpeta::nueva();
    let a = Equipo::abrir(config(&c.0, "ana")).await.unwrap();
    let b = Equipo::abrir(config(&c.0, "beto")).await.unwrap();
    let r = Equipo::abrir(config(&c.0, "rosa")).await.unwrap();
    let datos = aleatorio(&c.0.join("grande.bin"), 8 << 20);
    let envios = a
        .ofrecer_varios(
            &[b.billete(), r.billete()],
            vec![(c.0.join("grande.bin"), "grande.bin".into())],
        )
        .unwrap();
    let (mut vb, mut vr) = (Vec::new(), Vec::new());
    let o = esperar(&r, &mut vr, |ev| ev["type"] == "oferta").await;
    r.responder(o["oferta"].as_u64().unwrap(), vec![], None)
        .unwrap();
    let o = esperar(&b, &mut vb, |ev| ev["type"] == "oferta").await;
    let inicio = Instant::now();
    b.responder(
        o["oferta"].as_u64().unwrap(),
        vec![0],
        Some(c.0.join("destino")),
    )
    .unwrap();
    let mut va = Vec::new();
    let f = esperar(&a, &mut va, |ev| {
        ev["type"] == "fin" && ev["id"] == envios[0]
    })
    .await;
    assert_eq!(f["ok"], true, "{f}");
    // Rosa rechazó: su trozo se pide al emisor enseguida, sin esperar los 30 s de un par callado.
    assert!(
        inicio.elapsed() < Duration::from_secs(20),
        "tardó {:?}",
        inicio.elapsed()
    );
    assert_eq!(
        std::fs::read(c.0.join("destino").join("grande.bin")).unwrap(),
        datos
    );
    let f = esperar(&a, &mut va, |ev| {
        ev["type"] == "fin" && ev["id"] == envios[1]
    })
    .await;
    assert_eq!(f["error"], "rechazado");
}

#[tokio::test(flavor = "multi_thread")]
async fn el_limite_de_subida_frena_al_que_envia() {
    let c = Carpeta::nueva();
    let a = Equipo::abrir(Config {
        limite_subida: Some(2 << 20),
        ..config(&c.0, "ana")
    })
    .await
    .unwrap();
    let b = Equipo::abrir(config(&c.0, "beto")).await.unwrap();
    aleatorio(&c.0.join("x.bin"), 4 << 20);
    let id = a
        .ofrecer(&b.billete(), vec![(c.0.join("x.bin"), "x.bin".into())])
        .unwrap();
    let mut vb = Vec::new();
    let o = esperar(&b, &mut vb, |ev| ev["type"] == "oferta").await;
    let inicio = Instant::now();
    b.responder(
        o["oferta"].as_u64().unwrap(),
        vec![0],
        Some(c.0.join("destino")),
    )
    .unwrap();
    let mut va = Vec::new();
    let f = esperar(&a, &mut va, |ev| ev["type"] == "fin" && ev["id"] == id).await;
    assert_eq!(f["ok"], true, "{f}");
    // 4 MiB a 2 MiB/s: al menos ~2 s (sin límite, por loopback, son milisegundos).
    assert!(
        inicio.elapsed() >= Duration::from_millis(1500),
        "tardó {:?}",
        inicio.elapsed()
    );
}

#[tokio::test]
async fn solo_rele_exige_un_rele_propio() {
    let c = Carpeta::nueva();
    for rele in [Rele::N0, Rele::Ninguno] {
        let r = Equipo::abrir(Config {
            rele,
            solo_rele: true,
            ..config(&c.0, "ana")
        })
        .await;
        assert_eq!(r.err().map(|f| f.0), Some("invalid-parameters"));
    }
}

/// Con un relé propio y «solo relé» los datos van por él (no se exige ruta directa). Necesita `--features prueba`.
#[cfg(feature = "prueba")]
#[tokio::test(flavor = "multi_thread")]
async fn solo_rele_por_un_rele_propio() {
    let c = Carpeta::nueva();
    let (_mapa, url, _servidor) = iroh::test_utils::run_relay_server().await.unwrap();
    let por_rele = |alias| Config {
        rele: Rele::Url(url.clone()),
        solo_rele: true,
        inseguro: true,
        ..config(&c.0, alias)
    };
    let a = Equipo::abrir(por_rele("ana")).await.unwrap();
    let b = Equipo::abrir(por_rele("beto")).await.unwrap();
    let datos = aleatorio(&c.0.join("x.bin"), 2 << 20);
    let id = a
        .ofrecer(&b.billete(), vec![(c.0.join("x.bin"), "x.bin".into())])
        .unwrap();
    let mut vb = Vec::new();
    let o = esperar(&b, &mut vb, |ev| ev["type"] == "oferta").await;
    b.responder(
        o["oferta"].as_u64().unwrap(),
        vec![0],
        Some(c.0.join("destino")),
    )
    .unwrap();
    let mut va = Vec::new();
    let f = esperar(&a, &mut va, |ev| ev["type"] == "fin" && ev["id"] == id).await;
    assert_eq!(f["ok"], true, "{f}");
    assert_eq!(
        std::fs::read(c.0.join("destino").join("x.bin")).unwrap(),
        datos
    );
}

/// Un miembro de un grupo solo puede bajar de otro receptor el trozo de ese receptor, no el archivo entero: si no,
/// un emisor hostil podría meter a un cómplice en el grupo para sacarle a un receptor lo que quiera.
#[tokio::test(flavor = "multi_thread")]
async fn un_miembro_del_grupo_solo_baja_el_trozo_del_otro() {
    use iroh_blobs::{
        api::downloader::DownloadProgressItem,
        protocol::{ChunkRanges, ChunkRangesExt, GetRequest},
        store::mem::MemStore,
    };
    use n0_future::StreamExt;

    let c = Carpeta::nueva();
    let a = Equipo::abrir(config(&c.0, "ana")).await.unwrap();
    let r = Equipo::abrir(config(&c.0, "rosa")).await.unwrap();
    // X: un endpoint que no habla beamline/1 (no recibe nada) pero está en el grupo con su clave.
    let memoria = iroh::address_lookup::memory::MemoryLookup::new();
    memoria.add_endpoint_info(beamline_red::billete::leer(&r.billete()).unwrap());
    let x = iroh::Endpoint::builder(iroh::endpoint::presets::Minimal)
        .relay_mode(iroh::RelayMode::Disabled)
        .address_lookup(memoria)
        .bind()
        .await
        .unwrap();
    let billete_x = {
        use base64::Engine;
        format!(
            "beam1{}",
            base64::engine::general_purpose::URL_SAFE_NO_PAD
                .encode(serde_json::to_vec(&x.addr()).unwrap())
        )
    };
    const TAMANO: usize = 8 << 20;
    aleatorio(&c.0.join("grande.bin"), TAMANO);
    a.ofrecer_varios(
        &[r.billete(), billete_x],
        vec![(c.0.join("grande.bin"), "grande.bin".into())],
    )
    .unwrap();
    let mut vr = Vec::new();
    let o = esperar(&r, &mut vr, |ev| ev["type"] == "oferta").await;
    let id = o["oferta"].as_u64().unwrap();
    r.responder(id, vec![0], Some(c.0.join("destino"))).unwrap();
    let f = esperar(&r, &mut vr, |ev| ev["type"] == "fin" && ev["id"] == id).await;
    assert_eq!(f["ok"], true, "{f}");

    // Rosa es la 0 de 2: su trozo es la primera mitad.
    let hash = iroh_blobs::Hash::new(std::fs::read(c.0.join("grande.bin")).unwrap());
    let mitad = (TAMANO as u64).div_ceil(16 << 10) / 2 * 16;
    let x_store = MemStore::new();
    let bajar = |rango: ChunkRanges| {
        let s = x_store
            .downloader(&x)
            .download(GetRequest::blob_ranges(hash, rango), Some(r.clave()));
        async move {
            let mut s = s.stream().await.unwrap();
            let mut bien = true;
            while let Some(i) = s.next().await {
                if matches!(
                    i,
                    DownloadProgressItem::Error(_) | DownloadProgressItem::DownloadError
                ) {
                    bien = false;
                }
            }
            bien
        }
    };
    // Primero lo permitido: así lo que falla después es por permisos, no porque X no llegue a Rosa.
    assert!(
        bajar(ChunkRanges::chunks(0..mitad)).await,
        "X no pudo bajar el trozo de Rosa"
    );
    assert!(
        !bajar(ChunkRanges::all()).await,
        "X bajó el archivo entero de Rosa"
    );
    assert!(
        !bajar(ChunkRanges::chunks(mitad..)).await,
        "X bajó de Rosa un trozo que no es el suyo"
    );
}
