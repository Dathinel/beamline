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
    let datos: Vec<u8> = (0..n).map(|_| rand::random::<u8>()).collect();
    std::fs::write(ruta, &datos).unwrap();
    datos
}

#[tokio::test(flavor = "multi_thread")]
async fn a_tres_a_la_vez_el_emisor_sube_menos_del_doble() {
    let c = Carpeta::nueva();
    let a = Equipo::abrir(config(&c.0, "ana")).await.unwrap();
    let mut rs = Vec::new();
    for n in ["beto", "carla", "dani"] {
        rs.push(Equipo::abrir(config(&c.0, n)).await.unwrap());
    }
    const TAMANO: usize = 24 << 20;
    let datos = aleatorio(&c.0.join("grande.bin"), TAMANO);
    let billetes: Vec<String> = rs.iter().map(Equipo::billete).collect();
    let envios = a
        .ofrecer_varios(
            &billetes,
            vec![(c.0.join("grande.bin"), "grande.bin".into())],
        )
        .unwrap();
    assert_eq!(envios.len(), 3);

    let mut vistas: Vec<Vec<Value>> = vec![Vec::new(); 3];
    for (k, r) in rs.iter().enumerate() {
        let o = esperar(r, &mut vistas[k], |ev| ev["type"] == "oferta").await;
        r.responder(
            o["oferta"].as_u64().unwrap(),
            vec![0],
            Some(c.0.join(format!("destino-{k}"))),
        )
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
    eprintln!("CIFRAS emisor={subidos} receptores={de_otros} tamano={TAMANO}");
    assert!(
        subidos < 2 * TAMANO as u64,
        "el emisor subió {subidos} bytes de {TAMANO}"
    );
    assert!(
        de_otros > 0,
        "los receptores no se sirvieron nada entre ellos"
    );

    a.cerrar().await;
    for r in rs {
        r.cerrar().await;
    }
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
