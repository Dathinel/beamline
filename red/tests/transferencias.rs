//! Dos equipos de verdad (iroh sin relé, por loopback) en un proceso: docs/protocolo.md de punta a punta.

use std::{
    path::{Path, PathBuf},
    time::Duration,
};

use beamline_red::equipo::{ALPN, Config, Equipo, Rele};
use dathinel_red::marco;
use iroh::{Endpoint, RelayMode, endpoint::presets};
use serde_json::{Value, json};
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

async fn equipo(dir: &Path, alias: &str, tamano_max: u64) -> Equipo {
    equipo_con(dir, alias, tamano_max, false).await
}

async fn equipo_con(dir: &Path, alias: &str, tamano_max: u64, mdns: bool) -> Equipo {
    Equipo::abrir(Config {
        identidad: iroh::SecretKey::generate(),
        alias: alias.into(),
        almacen: dir.join(format!("almacen-{alias}")),
        rele: Rele::Ninguno,
        solo_rele: false,
        mdns,
        tamano_max,
        inseguro: false,
        limpieza: Duration::from_secs(1),
        espera_respuesta: Duration::from_secs(20),
    })
    .await
    .unwrap()
}

/// Espera un evento que cumpla `f`, guardando los demás en `vistos`.
async fn esperar(e: &Equipo, vistos: &mut Vec<Value>, f: impl Fn(&Value) -> bool) -> Value {
    if let Some(i) = vistos.iter().position(&f) {
        return vistos.remove(i);
    }
    let nada = Notify::new();
    let hasta = tokio::time::Instant::now() + Duration::from_secs(60);
    while tokio::time::Instant::now() < hasta {
        // Todo el lote a `vistos` antes de buscar: lo que venga detrás del buscado no se pierde.
        vistos.extend(e.recibir(Duration::from_millis(500), &nada).await);
        if let Some(i) = vistos.iter().position(&f) {
            return vistos.remove(i);
        }
    }
    panic!("no llegó el evento; vistos: {vistos:?}");
}

fn tipo<'a>(t: &'a str) -> impl Fn(&Value) -> bool + 'a {
    move |ev| ev["type"] == t
}

fn fin_de(id: u64) -> impl Fn(&Value) -> bool {
    move |ev| ev["type"] == "fin" && ev["id"] == id
}

fn aleatorio(ruta: &Path, n: usize) {
    if let Some(p) = ruta.parent() {
        std::fs::create_dir_all(p).unwrap();
    }
    let datos: Vec<u8> = (0..n).map(|_| rand::random::<u8>()).collect();
    std::fs::write(ruta, datos).unwrap();
}

fn archivos_en(dir: &Path) -> Vec<String> {
    let mut v = Vec::new();
    fn rec(base: &Path, d: &Path, v: &mut Vec<String>) {
        for e in std::fs::read_dir(d).unwrap().flatten() {
            let p = e.path();
            if p.is_dir() {
                rec(base, &p, v);
            } else {
                v.push(
                    p.strip_prefix(base)
                        .unwrap()
                        .to_string_lossy()
                        .replace('\\', "/"),
                );
            }
        }
    }
    if dir.exists() {
        rec(dir, dir, &mut v);
    }
    v.sort();
    v
}

#[tokio::test(flavor = "multi_thread")]
async fn envia_una_carpeta_entera_y_no_toca_los_originales() {
    let c = Carpeta::nueva();
    let (a, b) = (
        equipo(&c.0, "ana", 1 << 40).await,
        equipo(&c.0, "beto", 1 << 40).await,
    );
    let origen = c.0.join("origen");
    aleatorio(&origen.join("grande.bin"), 6 << 20);
    aleatorio(&origen.join("fotos/2026/ñandú.jpg"), 1000);
    aleatorio(&origen.join("vacío.txt"), 0);
    let lista = vec![
        (origen.join("grande.bin"), "grande.bin".into()),
        (
            origen.join("fotos/2026/ñandú.jpg"),
            "fotos/2026/ñandú.jpg".into(),
        ),
        (origen.join("vacío.txt"), "vacío.txt".into()),
    ];
    let envio = a.ofrecer(&b.billete(), lista).unwrap();

    let (mut va, mut vb) = (Vec::new(), Vec::new());
    let oferta = esperar(&b, &mut vb, tipo("oferta")).await;
    assert_eq!(oferta["alias"], "ana");
    assert_eq!(oferta["de"], a.clave().to_string());
    assert_eq!(oferta["total"], (6 << 20) + 1000);
    assert_eq!(oferta["archivos"].as_array().unwrap().len(), 3);
    let destino = c.0.join("destino");
    b.responder(
        oferta["oferta"].as_u64().unwrap(),
        vec![0, 1, 2],
        Some(destino.clone()),
    )
    .unwrap();

    let r = esperar(&a, &mut va, tipo("respuesta")).await;
    assert_eq!(r["aceptados"], json!([0, 1, 2]));
    let fa = esperar(&a, &mut va, fin_de(envio)).await;
    assert_eq!(fa["ok"], true, "{fa}");
    let fb = esperar(&b, &mut vb, fin_de(oferta["oferta"].as_u64().unwrap())).await;
    assert_eq!(fb["ok"], true, "{fb}");
    assert_eq!(fb["rutas"].as_array().unwrap().len(), 3);
    assert!(
        vb.iter().any(|e| e["type"] == "avance"),
        "sin avance en el que recibe"
    );

    assert_eq!(
        archivos_en(&destino),
        ["fotos/2026/ñandú.jpg", "grande.bin", "vacío.txt"]
    );
    for n in ["grande.bin", "fotos/2026/ñandú.jpg", "vacío.txt"] {
        assert_eq!(
            std::fs::read(origen.join(n)).unwrap(),
            std::fs::read(destino.join(n)).unwrap(),
            "{n}"
        );
    }
    // Tras varias limpiezas del almacén, los archivos de quien envía siguen ahí.
    tokio::time::sleep(Duration::from_secs(3)).await;
    assert_eq!(archivos_en(&origen).len(), 3);
    a.cerrar().await;
    b.cerrar().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn rechazar_y_aceptar_una_parte() {
    let c = Carpeta::nueva();
    let (a, b) = (
        equipo(&c.0, "ana", 1 << 40).await,
        equipo(&c.0, "beto", 1 << 40).await,
    );
    aleatorio(&c.0.join("x.bin"), 100);
    aleatorio(&c.0.join("y.bin"), 200);
    let lista = || {
        vec![
            (c.0.join("x.bin"), "x.bin".to_string()),
            (c.0.join("y.bin"), "y.bin".to_string()),
        ]
    };
    let (mut va, mut vb) = (Vec::new(), Vec::new());

    let e1 = a.ofrecer(&b.billete(), lista()).unwrap();
    let o = esperar(&b, &mut vb, tipo("oferta")).await;
    b.responder(o["oferta"].as_u64().unwrap(), vec![], None)
        .unwrap();
    let f = esperar(&a, &mut va, fin_de(e1)).await;
    assert_eq!(
        (f["ok"].clone(), f["error"].clone()),
        (json!(false), json!("rechazado"))
    );

    let e2 = a.ofrecer(&b.billete(), lista()).unwrap();
    let o = esperar(&b, &mut vb, tipo("oferta")).await;
    let destino = c.0.join("destino");
    b.responder(
        o["oferta"].as_u64().unwrap(),
        vec![1],
        Some(destino.clone()),
    )
    .unwrap();
    assert_eq!(esperar(&a, &mut va, fin_de(e2)).await["ok"], true);
    assert_eq!(
        esperar(&b, &mut vb, fin_de(o["oferta"].as_u64().unwrap())).await["ok"],
        true
    );
    assert_eq!(archivos_en(&destino), ["y.bin"]);

    // El mismo archivo otra vez: ya está, no se baja ni se duplica.
    let e3 = a.ofrecer(&b.billete(), lista()).unwrap();
    let o = esperar(&b, &mut vb, tipo("oferta")).await;
    b.responder(
        o["oferta"].as_u64().unwrap(),
        vec![1],
        Some(destino.clone()),
    )
    .unwrap();
    assert_eq!(esperar(&a, &mut va, fin_de(e3)).await["ok"], true);
    esperar(&b, &mut vb, fin_de(o["oferta"].as_u64().unwrap())).await;
    assert_eq!(
        archivos_en(&destino),
        ["y.bin"],
        "el mismo contenido no se duplica"
    );

    // Mismo nombre con otro contenido: no pisa, queda «y (2).bin».
    aleatorio(&c.0.join("y.bin"), 200);
    let e4 = a.ofrecer(&b.billete(), lista()).unwrap();
    let o = esperar(&b, &mut vb, tipo("oferta")).await;
    b.responder(
        o["oferta"].as_u64().unwrap(),
        vec![1],
        Some(destino.clone()),
    )
    .unwrap();
    assert_eq!(esperar(&a, &mut va, fin_de(e4)).await["ok"], true);
    esperar(&b, &mut vb, fin_de(o["oferta"].as_u64().unwrap())).await;
    assert_eq!(archivos_en(&destino), ["y (2).bin", "y.bin"]);
    a.cerrar().await;
    b.cerrar().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn cancelar_antes_y_durante() {
    let c = Carpeta::nueva();
    let (a, b) = (
        equipo(&c.0, "ana", 1 << 40).await,
        equipo(&c.0, "beto", 1 << 40).await,
    );
    aleatorio(&c.0.join("g.bin"), 48 << 20);
    let lista = || vec![(c.0.join("g.bin"), "g.bin".to_string())];
    let (mut va, mut vb) = (Vec::new(), Vec::new());

    // Antes de la respuesta: el que recibe ve la oferta retirada.
    let e1 = a.ofrecer(&b.billete(), lista()).unwrap();
    let o = esperar(&b, &mut vb, tipo("oferta")).await;
    a.cancelar(e1).unwrap();
    let r = esperar(&b, &mut vb, tipo("oferta-retirada")).await;
    assert_eq!(r["oferta"], o["oferta"]);
    assert_eq!(esperar(&a, &mut va, fin_de(e1)).await["error"], "cancelado");
    assert_eq!(
        b.responder(o["oferta"].as_u64().unwrap(), vec![0], Some(c.0.join("d")))
            .unwrap_err()
            .0,
        "desconocido"
    );

    // Durante la descarga, cancela el que recibe: no queda ni el archivo ni la parte.
    let e2 = a.ofrecer(&b.billete(), lista()).unwrap();
    let o = esperar(&b, &mut vb, tipo("oferta")).await;
    let id = o["oferta"].as_u64().unwrap();
    let destino = c.0.join("destino");
    b.responder(id, vec![0], Some(destino.clone())).unwrap();
    esperar(&b, &mut vb, |e| e["type"] == "avance" && e["id"] == id).await;
    b.cancelar(id).unwrap();
    assert_eq!(esperar(&b, &mut vb, fin_de(id)).await["error"], "cancelado");
    assert_eq!(esperar(&a, &mut va, fin_de(e2)).await["error"], "cancelado");
    assert!(
        archivos_en(&destino).is_empty(),
        "{:?}",
        archivos_en(&destino)
    );
    a.cerrar().await;
    b.cerrar().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn reanuda_tras_cortarse_la_red() {
    let c = Carpeta::nueva();
    let b = equipo(&c.0, "beto", 1 << 40).await;
    let total = 64usize << 20;
    aleatorio(&c.0.join("g.bin"), total);
    let lista = || vec![(c.0.join("g.bin"), "g.bin".to_string())];
    let mut vb = Vec::new();

    let a = equipo(&c.0, "ana", 1 << 40).await;
    a.ofrecer(&b.billete(), lista()).unwrap();
    let o = esperar(&b, &mut vb, tipo("oferta")).await;
    let id = o["oferta"].as_u64().unwrap();
    b.responder(id, vec![0], Some(c.0.join("destino"))).unwrap();
    let mut a_medias = 0;
    while a_medias < total as u64 / 8 {
        a_medias = esperar(&b, &mut vb, |e| e["type"] == "avance" && e["id"] == id).await["bytes"]
            .as_u64()
            .unwrap();
    }
    a.cerrar().await; // se corta la red en mitad
    let f = esperar(&b, &mut vb, fin_de(id)).await;
    assert_eq!(f["ok"], false);
    assert_ne!(f["error"], "cancelado");

    // Otro proceso del que envía ofrece lo mismo: el primer avance ya arranca desde lo bajado.
    let a2 = equipo(&c.0, "ana2", 1 << 40).await;
    let e = a2.ofrecer(&b.billete(), lista()).unwrap();
    let o = esperar(&b, &mut vb, tipo("oferta")).await;
    let id = o["oferta"].as_u64().unwrap();
    b.responder(id, vec![0], Some(c.0.join("destino"))).unwrap();
    let primero = esperar(&b, &mut vb, |e| e["type"] == "avance" && e["id"] == id).await["bytes"]
        .as_u64()
        .unwrap();
    assert!(
        primero >= a_medias,
        "reanudó desde {primero}, ya había {a_medias}"
    );
    let mut va = Vec::new();
    assert_eq!(esperar(&a2, &mut va, fin_de(e)).await["ok"], true);
    assert_eq!(esperar(&b, &mut vb, fin_de(id)).await["ok"], true);
    assert_eq!(
        std::fs::read(c.0.join("g.bin")).unwrap(),
        std::fs::read(c.0.join("destino/g.bin")).unwrap()
    );
    a2.cerrar().await;
    b.cerrar().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn archivo_cambiado_mientras_se_envia() {
    let c = Carpeta::nueva();
    let (a, b) = (
        equipo(&c.0, "ana", 1 << 40).await,
        equipo(&c.0, "beto", 1 << 40).await,
    );
    aleatorio(&c.0.join("g.bin"), 8 << 20);
    let (mut va, mut vb) = (Vec::new(), Vec::new());
    let e = a
        .ofrecer(&b.billete(), vec![(c.0.join("g.bin"), "g.bin".into())])
        .unwrap();
    let o = esperar(&b, &mut vb, tipo("oferta")).await;
    aleatorio(&c.0.join("g.bin"), 8 << 20); // mismo tamaño, otro contenido
    let destino = c.0.join("destino");
    b.responder(
        o["oferta"].as_u64().unwrap(),
        vec![0],
        Some(destino.clone()),
    )
    .unwrap();
    let fb = esperar(&b, &mut vb, fin_de(o["oferta"].as_u64().unwrap())).await;
    assert_eq!(fb["ok"], false, "{fb}");
    assert_eq!(esperar(&a, &mut va, fin_de(e)).await["error"], "huella");
    assert!(archivos_en(&destino).is_empty());
    a.cerrar().await;
    b.cerrar().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn limites_del_que_recibe() {
    let c = Carpeta::nueva();
    let (a, b) = (
        equipo(&c.0, "ana", 1 << 40).await,
        equipo(&c.0, "beto", 1000).await,
    );
    aleatorio(&c.0.join("x.bin"), 2000);
    let (mut va, mut vb): (Vec<Value>, Vec<Value>) = (Vec::new(), Vec::new());

    // Más grande que el tope: se rechaza sola, sin preguntar.
    let e = a
        .ofrecer(&b.billete(), vec![(c.0.join("x.bin"), "x.bin".into())])
        .unwrap();
    assert_eq!(esperar(&a, &mut va, fin_de(e)).await["error"], "rechazado");
    assert!(!vb.iter().any(|e| e["type"] == "oferta"));

    // Cuatro ofertas pendientes como mucho: la quinta vuelve «ocupado».
    aleatorio(&c.0.join("y.bin"), 10);
    let mut ids = Vec::new();
    for _ in 0..4 {
        ids.push(
            a.ofrecer(&b.billete(), vec![(c.0.join("y.bin"), "y.bin".into())])
                .unwrap(),
        );
        esperar(&b, &mut vb, tipo("oferta")).await;
    }
    let quinta = a
        .ofrecer(&b.billete(), vec![(c.0.join("y.bin"), "y.bin".into())])
        .unwrap();
    assert_eq!(
        esperar(&a, &mut va, fin_de(quinta)).await["error"],
        "ocupado"
    );

    // Nombres que no valen ni salen del que envía.
    let mal = a
        .ofrecer(
            &b.billete(),
            vec![(c.0.join("y.bin"), "../fuera.bin".into())],
        )
        .unwrap_err();
    assert_eq!(mal.0, "nombre-invalido");
    let dup = a
        .ofrecer(
            &b.billete(),
            vec![
                (c.0.join("y.bin"), "A".into()),
                (c.0.join("y.bin"), "a".into()),
            ],
        )
        .unwrap_err();
    assert_eq!(dup.0, "nombre-invalido");
    assert_eq!(
        a.ofrecer("beam1basura", vec![(c.0.join("y.bin"), "y".into())])
            .unwrap_err()
            .0,
        "billete-invalido"
    );
    a.cerrar().await;
    b.cerrar().await;
}

/// Un equipo que habla el protocolo a mano, como lo haría uno malicioso.
async fn hostil() -> Endpoint {
    Endpoint::builder(presets::Minimal)
        .relay_mode(RelayMode::Disabled)
        .bind()
        .await
        .unwrap()
}

#[tokio::test(flavor = "multi_thread")]
async fn ofertas_hostiles_no_llegan_a_la_app() {
    let c = Carpeta::nueva();
    let b = equipo(&c.0, "beto", u64::MAX).await;
    let addr = beamline_red::billete::leer(&b.billete()).unwrap();
    let malo = hostil().await;
    let h = iroh_blobs::Hash::new(b"x").to_hex();
    let malas = [
        json!({"type": "oferta", "version": 1, "alias": "m", "archivos": [{"nombre": "../../Windows/x.dll", "tamano": 1, "hash": h}]}),
        json!({"type": "oferta", "version": 1, "alias": "m", "archivos": [{"nombre": "C:/x", "tamano": 1, "hash": h}]}),
        json!({"type": "oferta", "version": 1, "alias": "m", "archivos": [{"nombre": "CON", "tamano": 1, "hash": h}]}),
        json!({"type": "oferta", "version": 1, "alias": "m", "archivos": [{"nombre": "a", "tamano": 1, "hash": "zz"}]}),
        json!({"type": "oferta", "version": 1, "alias": "m", "archivos": [{"nombre": "a", "tamano": u64::MAX, "hash": h}, {"nombre": "b", "tamano": 1, "hash": h}]}),
        json!({"type": "oferta", "version": 1, "alias": "m", "archivos": []}),
        json!({"type": "otra"}),
    ];
    for m in &malas {
        let conn = malo.connect(addr.clone(), ALPN).await.unwrap();
        let (mut tx, mut rx) = conn.open_bi().await.unwrap();
        tx.write_all(&marco::armar(m)).await.unwrap();
        let r = marco::leer(&mut rx, 4096, 28).await;
        assert!(r.is_err(), "contestó a {m}: {r:?}");
    }
    // Versión desconocida: contesta «version» para que el otro avise de actualizar.
    let conn = malo.connect(addr.clone(), ALPN).await.unwrap();
    let (mut tx, mut rx) = conn.open_bi().await.unwrap();
    tx.write_all(&marco::armar(&json!({"type": "oferta", "version": 2})))
        .await
        .unwrap();
    assert_eq!(
        marco::leer(&mut rx, 4096, 28).await.unwrap()["type"],
        "version"
    );

    // Tamaño enorme pero bajo el tope: llega, y aceptarla falla por espacio.
    let conn = malo.connect(addr.clone(), ALPN).await.unwrap();
    let (mut tx, _rx) = conn.open_bi().await.unwrap();
    tx.write_all(&marco::armar(&json!({"type": "oferta", "version": 1, "alias": "m", "archivos": [{"nombre": "a", "tamano": 1u64 << 60, "hash": h}]}))).await.unwrap();
    let mut vb = Vec::new();
    let o = esperar(&b, &mut vb, tipo("oferta")).await;
    assert_eq!(
        b.responder(o["oferta"].as_u64().unwrap(), vec![0], Some(c.0.join("d")))
            .unwrap_err()
            .0,
        "espacio"
    );
    assert_eq!(
        vb.iter().filter(|e| e["type"] == "oferta").count(),
        0,
        "llegaron ofertas inválidas: {vb:?}"
    );

    // Ritmo: más de 10 ofertas por minuto desde la misma clave se cortan sin leerlas.
    let mut cortadas = 0;
    for _ in 0..12 {
        let conn = malo.connect(addr.clone(), ALPN).await.unwrap();
        if let Ok((mut tx, mut rx)) = conn.open_bi().await {
            let _ = tx.write_all(&marco::armar(&json!({"type": "otra"}))).await;
            let _ = marco::leer(&mut rx, 4096, 28).await;
        }
        if matches!(conn.close_reason(), Some(iroh::endpoint::ConnectionError::ApplicationClosed(c)) if c.error_code == iroh::endpoint::VarInt::from_u32(3))
        {
            cortadas += 1;
        }
    }
    assert!(cortadas >= 1, "el ritmo no cortó ninguna");
    b.cerrar().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn nadie_mas_puede_bajar_lo_ofrecido() {
    let c = Carpeta::nueva();
    let (a, b) = (
        equipo(&c.0, "ana", 1 << 40).await,
        equipo(&c.0, "beto", 1 << 40).await,
    );
    aleatorio(&c.0.join("secreto.bin"), 4096);
    let datos = std::fs::read(c.0.join("secreto.bin")).unwrap();
    let hash = iroh_blobs::Hash::new(&datos);
    let mut vb = Vec::new();
    a.ofrecer(
        &b.billete(),
        vec![(c.0.join("secreto.bin"), "secreto.bin".into())],
    )
    .unwrap();
    let o = esperar(&b, &mut vb, tipo("oferta")).await;

    // Un tercero que sabe la huella, antes y después de que el otro acepte: nada.
    let malo = hostil().await;
    let addr = beamline_red::billete::leer(&a.billete()).unwrap();
    let intentar = || async {
        let conn = malo.connect(addr.clone(), iroh_blobs::ALPN).await.ok()?;
        iroh_blobs::get::request::get_blob(conn, hash)
            .bytes()
            .await
            .ok()
    };
    assert!(intentar().await.is_none(), "bajó sin oferta aceptada");
    b.responder(o["oferta"].as_u64().unwrap(), vec![0], Some(c.0.join("d")))
        .unwrap();
    assert!(intentar().await.is_none(), "bajó la oferta de otro");
    esperar(&b, &mut vb, fin_de(o["oferta"].as_u64().unwrap())).await;
    a.cerrar().await;
    b.cerrar().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn los_cercanos_aparecen_con_su_alias_y_se_envia_por_clave() {
    let c = Carpeta::nueva();
    let a = equipo_con(&c.0, "ana", 1 << 40, true).await;
    let b = equipo_con(&c.0, "beto con ñ", 1 << 40, true).await;
    let (mut va, mut vb) = (Vec::new(), Vec::new());
    let clave_b = b.clave().to_string();
    let visto = esperar(&a, &mut va, |e| {
        e["type"] == "cercano" && e["clave"] == clave_b
    })
    .await;
    assert_eq!(visto["alias"], "beto con ñ");
    assert_eq!(visto["visto"], true);
    assert!(
        a.estado()["cercanos"]
            .as_array()
            .unwrap()
            .iter()
            .any(|x| x["clave"] == clave_b)
    );

    // Sin billete: solo con la clave, que mDNS ya resolvió a una dirección.
    aleatorio(&c.0.join("x.bin"), 1000);
    let e = a
        .ofrecer(&clave_b, vec![(c.0.join("x.bin"), "x.bin".into())])
        .unwrap();
    let o = esperar(&b, &mut vb, tipo("oferta")).await;
    b.responder(o["oferta"].as_u64().unwrap(), vec![0], Some(c.0.join("d")))
        .unwrap();
    assert_eq!(esperar(&a, &mut va, fin_de(e)).await["ok"], true);
    a.cerrar().await;
    b.cerrar().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn mil_archivos_en_una_oferta() {
    let c = Carpeta::nueva();
    let (a, b) = (
        equipo(&c.0, "ana", 1 << 40).await,
        equipo(&c.0, "beto", 1 << 40).await,
    );
    let origen = c.0.join("muchos");
    let mut lista = Vec::new();
    for i in 0..1000 {
        let r = origen.join(format!("f{i:04}.txt"));
        aleatorio(&r, 16);
        lista.push((r, format!("muchos/f{i:04}.txt")));
    }
    let (mut va, mut vb) = (Vec::new(), Vec::new());
    let e = a.ofrecer(&b.billete(), lista).unwrap();
    let o = esperar(&b, &mut vb, tipo("oferta")).await;
    let destino = c.0.join("destino");
    b.responder(
        o["oferta"].as_u64().unwrap(),
        (0..1000).collect(),
        Some(destino.clone()),
    )
    .unwrap();
    assert_eq!(esperar(&a, &mut va, fin_de(e)).await["ok"], true);
    let fb = esperar(&b, &mut vb, fin_de(o["oferta"].as_u64().unwrap())).await;
    assert_eq!(fb["rutas"].as_array().unwrap().len(), 1000);
    assert_eq!(archivos_en(&destino).len(), 1000);
    a.cerrar().await;
    b.cerrar().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn cinco_transferencias_a_la_vez_esperan_turno_sin_fallar() {
    let c = Carpeta::nueva();
    let (a, b) = (
        equipo(&c.0, "ana", 1 << 40).await,
        equipo(&c.0, "beto", 1 << 40).await,
    );
    let (mut va, mut vb) = (Vec::new(), Vec::new());
    let mut envios = Vec::new();
    for i in 0..5 {
        aleatorio(&c.0.join(format!("g{i}.bin")), 64 << 20);
    }
    let ofrecer = |i: usize| {
        a.ofrecer(
            &b.billete(),
            vec![(c.0.join(format!("g{i}.bin")), format!("g{i}.bin"))],
        )
        .unwrap()
    };
    // Cuatro pendientes (el tope), las cuatro aceptadas a la vez, y la quinta mientras corren.
    for i in 0..4 {
        envios.push(ofrecer(i));
    }
    let mut ids = Vec::new();
    for _ in 0..4 {
        ids.push(
            esperar(&b, &mut vb, tipo("oferta")).await["oferta"]
                .as_u64()
                .unwrap(),
        );
    }
    for &id in &ids {
        b.responder(id, vec![0], Some(c.0.join("destino"))).unwrap();
    }
    envios.push(ofrecer(4));
    let id = esperar(&b, &mut vb, tipo("oferta")).await["oferta"]
        .as_u64()
        .unwrap();
    b.responder(id, vec![0], Some(c.0.join("destino"))).unwrap();
    ids.push(id);
    for (e, id) in envios.into_iter().zip(ids) {
        let fa = esperar(&a, &mut va, fin_de(e)).await;
        assert_eq!(fa["ok"], true, "{fa}");
        assert_eq!(esperar(&b, &mut vb, fin_de(id)).await["ok"], true);
    }
    assert_eq!(archivos_en(&c.0.join("destino")).len(), 5);
    a.cerrar().await;
    b.cerrar().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn reanudar_varios_no_duplica_lo_que_ya_llego() {
    let c = Carpeta::nueva();
    let b = equipo(&c.0, "beto", 1 << 40).await;
    aleatorio(&c.0.join("a.bin"), 1000);
    aleatorio(&c.0.join("b.bin"), 64 << 20);
    let lista = || {
        vec![
            (c.0.join("a.bin"), "a.bin".to_string()),
            (c.0.join("b.bin"), "b.bin".to_string()),
        ]
    };
    let destino = c.0.join("destino");
    let mut vb = Vec::new();

    let a = equipo(&c.0, "ana", 1 << 40).await;
    a.ofrecer(&b.billete(), lista()).unwrap();
    let id = esperar(&b, &mut vb, tipo("oferta")).await["oferta"]
        .as_u64()
        .unwrap();
    b.responder(id, vec![0, 1], Some(destino.clone())).unwrap();
    let mut bytes = 0;
    while bytes < 1000 + (16 << 20) {
        bytes = esperar(&b, &mut vb, |e| e["type"] == "avance" && e["id"] == id).await["bytes"]
            .as_u64()
            .unwrap();
    }
    a.cerrar().await;
    assert_eq!(esperar(&b, &mut vb, fin_de(id)).await["ok"], false);
    assert_eq!(archivos_en(&destino), ["a.bin"], "sin restos de la parte");

    let a2 = equipo(&c.0, "ana2", 1 << 40).await;
    let e = a2.ofrecer(&b.billete(), lista()).unwrap();
    let id = esperar(&b, &mut vb, tipo("oferta")).await["oferta"]
        .as_u64()
        .unwrap();
    b.responder(id, vec![0, 1], Some(destino.clone())).unwrap();
    let mut va = Vec::new();
    assert_eq!(esperar(&a2, &mut va, fin_de(e)).await["ok"], true);
    let fb = esperar(&b, &mut vb, fin_de(id)).await;
    assert_eq!(fb["ok"], true);
    assert_eq!(archivos_en(&destino), ["a.bin", "b.bin"]);
    assert_eq!(
        std::fs::read(c.0.join("b.bin")).unwrap(),
        std::fs::read(destino.join("b.bin")).unwrap()
    );
    a2.cerrar().await;
    b.cerrar().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn si_cancela_el_que_envia_no_quedan_restos() {
    let c = Carpeta::nueva();
    let (a, b) = (
        equipo(&c.0, "ana", 1 << 40).await,
        equipo(&c.0, "beto", 1 << 40).await,
    );
    aleatorio(&c.0.join("g.bin"), 48 << 20);
    let (mut va, mut vb) = (Vec::new(), Vec::new());
    let e = a
        .ofrecer(&b.billete(), vec![(c.0.join("g.bin"), "g.bin".into())])
        .unwrap();
    let id = esperar(&b, &mut vb, tipo("oferta")).await["oferta"]
        .as_u64()
        .unwrap();
    let destino = c.0.join("destino");
    b.responder(id, vec![0], Some(destino.clone())).unwrap();
    esperar(&b, &mut vb, |ev| ev["type"] == "avance" && ev["id"] == id).await;
    a.cancelar(e).unwrap();
    assert_eq!(esperar(&a, &mut va, fin_de(e)).await["error"], "cancelado");
    assert_eq!(esperar(&b, &mut vb, fin_de(id)).await["error"], "cancelado");
    assert!(
        archivos_en(&destino).is_empty(),
        "{:?}",
        archivos_en(&destino)
    );
    a.cerrar().await;
    b.cerrar().await;
}
