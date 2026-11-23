use super::*;

struct Carpeta(PathBuf);

impl Carpeta {
    fn nueva() -> Self {
        let p = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("target")
            .join(format!("regresion-{}", rand::random::<u64>()));
        std::fs::create_dir_all(&p).unwrap();
        Self(p)
    }
}

impl Drop for Carpeta {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

async fn equipo(c: &Carpeta, alias: &str, plazo: Duration) -> Equipo {
    Equipo::abrir_con_plazo(
        Config {
            identidad: SecretKey::generate(),
            alias: alias.into(),
            almacen: c.0.join(alias),
            rele: Rele::Ninguno,
            solo_rele: false,
            mdns: false,
            tamano_max: u64::MAX,
            inseguro: false,
            limpieza: Duration::from_millis(50),
            espera_respuesta: Duration::from_secs(10),
        },
        plazo,
    )
    .await
    .unwrap()
}

async fn evento(e: &Equipo, tipo: &str) -> Value {
    tokio::time::timeout(Duration::from_secs(15), async {
        loop {
            let eventos = e.recibir(Duration::from_secs(1), &Notify::new()).await;
            if let Some(ev) = eventos.into_iter().find(|ev| ev["type"] == tipo) {
                return ev;
            }
        }
    })
    .await
    .unwrap()
}

async fn ofrecer_manual(
    a: &Endpoint,
    b: &Equipo,
    hash: Hash,
    tamano: u64,
) -> (Connection, SendStream, RecvStream) {
    let conn = a.connect(b.0.router.endpoint().addr(), ALPN).await.unwrap();
    let (mut tx, rx) = conn.open_bi().await.unwrap();
    escribir(
        &mut tx,
        &json!({"type": "oferta", "version": 1, "alias": "par", "archivos": [
            {"nombre": "a.bin", "tamano": tamano, "hash": hash.to_hex()}
        ]}),
    )
    .await
    .unwrap();
    (conn, tx, rx)
}

fn pendiente(e: &Equipo, id: u64, tamano: u64) -> oneshot::Receiver<Respuesta> {
    {
        let mut eventos = e.0.eventos.lock().unwrap();
        if !eventos.reservas.contains_key(&id) {
            assert!(eventos.reservar(id));
        }
    }
    let (tx, rx) = oneshot::channel();
    e.0.pendientes.lock().unwrap().insert(
        id,
        Pendiente {
            responder: tx,
            cancelar: CancellationToken::new(),
            archivos: vec![("a.bin".into(), tamano)],
        },
    );
    rx
}

#[test]
fn rechaza_union_previa_sin_escribir_fuera() {
    let c = Carpeta::nueva();
    let destino = c.0.join("destino");
    let fuera = c.0.join("fuera");
    std::fs::create_dir_all(&destino).unwrap();
    std::fs::create_dir_all(&fuera).unwrap();
    let enlace = destino.join("fotos");
    #[cfg(windows)]
    {
        let salida = std::process::Command::new("powershell")
            .args(["-NoProfile", "-NonInteractive", "-Command", "New-Item -ItemType Junction -Path $env:PRUEBA_ENLACE -Target $env:PRUEBA_FUERA | Out-Null"])
            .env("PRUEBA_ENLACE", &enlace).env("PRUEBA_FUERA", &fuera).output().unwrap();
        assert!(salida.status.success(), "{:?}", salida);
    }
    #[cfg(unix)]
    std::os::unix::fs::symlink(&fuera, &enlace).unwrap();
    assert!(nombres::ruta_libre(&destino, "fotos/sub/a.txt").is_err());
    assert!(nombres::ruta_libre(&enlace, "a.txt").is_err());
    assert!(!fuera.join("sub").exists());
    assert_eq!(std::fs::read_dir(&fuera).unwrap().count(), 0);
    #[cfg(windows)]
    std::fs::remove_dir(enlace).unwrap();
    #[cfg(unix)]
    std::fs::remove_file(enlace).unwrap();
}

#[test]
fn temporales_exclusivos_y_publicacion_no_pisa() {
    use std::io::Write;
    let c = Carpeta::nueva();
    let previo = c.0.join(".beamline-42-0.beamline-parte");
    std::fs::write(&previo, b"previo").unwrap();
    let legado = c.0.join("a.txt.beamline-parte");
    std::fs::write(&legado, b"legitimo").unwrap();
    let (uno, mut f1) = Temporal::crear(&c.0, 42).unwrap();
    let (dos, mut f2) = Temporal::crear(&c.0, 42).unwrap();
    assert_ne!(uno.0, dos.0);
    f1.write_all(b"uno").unwrap();
    f2.write_all(b"dos").unwrap();
    drop((f1, f2));
    std::fs::write(c.0.join("a.txt"), b"aparecio").unwrap();
    let p1 = uno.publicar(&c.0, "a.txt", c.0.join("a.txt")).unwrap();
    let p2 = dos.publicar(&c.0, "a.txt", c.0.join("a.txt")).unwrap();
    assert_ne!(p1, p2);
    assert_eq!(std::fs::read(p1).unwrap(), b"uno");
    assert_eq!(std::fs::read(p2).unwrap(), b"dos");
    assert_eq!(std::fs::read(c.0.join("a.txt")).unwrap(), b"aparecio");
    assert_eq!(std::fs::read(previo).unwrap(), b"previo");
    assert_eq!(std::fs::read(legado).unwrap(), b"legitimo");
    let (fallido, f) = Temporal::crear(&c.0, 42).unwrap();
    let ruta = fallido.0.clone();
    drop(f);
    drop(fallido);
    assert!(!ruta.exists());
}

#[tokio::test]
async fn tamano_falso_corta_y_libera_reservas() {
    let c = Carpeta::nueva();
    let a = equipo(&c, "a", Duration::from_secs(10)).await;
    let b = equipo(&c, "b", Duration::from_secs(10)).await;
    let tag =
        a.0.store
            .blobs()
            .add_slice(vec![7u8; 32 << 20])
            .await
            .unwrap();
    let hash = tag.hash;
    let _conservar_para_medir = b.0.store.tags().temp_tag(hash).await.unwrap();
    a.autorizar(b.clave(), 1, std::iter::once(hash));
    b.0.store
        .tags()
        .set(format!("rx/{}/{}", hash.to_hex(), ahora() - 1), hash)
        .await
        .unwrap();
    let (_conn, mut tx, mut rx) = ofrecer_manual(a.0.router.endpoint(), &b, hash, 1).await;
    let o = evento(&b, "oferta").await;
    b.responder(
        o["oferta"].as_u64().unwrap(),
        vec![0],
        Some(c.0.join("destino")),
    )
    .unwrap();
    marco::leer(&mut rx, 4096, 28).await.unwrap();
    escribir(&mut tx, &json!({"type": "listo"})).await.unwrap();
    let fin = evento(&b, "fin").await;
    assert_eq!(fin["error"], "red");
    assert!(reservas(&b, hash).await.is_empty());
    assert!(!c.0.join("destino/a.bin").exists());
    let local = b.0.store.remote().local(hash).await.unwrap();
    assert!(local.local_bytes() < 32 << 20, "se descargó el blob entero");
    assert_eq!(*b.0.comprometido.lock().unwrap(), 0);
    a.cerrar().await;
    b.cerrar().await;
}

#[tokio::test]
async fn listo_tiene_plazo_y_libera_turno_con_fin_red() {
    let c = Carpeta::nueva();
    let a = equipo(&c, "a", Duration::from_secs(10)).await;
    let b = equipo(&c, "b", Duration::from_millis(150)).await;
    let (_conn, _tx, mut rx) = ofrecer_manual(a.0.router.endpoint(), &b, Hash::new(b"x"), 1).await;
    let o = evento(&b, "oferta").await;
    b.responder(
        o["oferta"].as_u64().unwrap(),
        vec![0],
        Some(c.0.join("destino")),
    )
    .unwrap();
    marco::leer(&mut rx, 4096, 28).await.unwrap();
    assert_eq!(evento(&b, "fin").await["error"], "red");
    assert_eq!(b.0.turnos_recepcion.available_permits(), TRANSFERENCIAS);
    assert_eq!(*b.0.comprometido.lock().unwrap(), 0);
    assert!(b.0.activos.lock().unwrap().is_empty());
    a.cerrar().await;
    b.cerrar().await;
}

#[derive(Debug)]
struct Mudo(Arc<Notify>);

impl ProtocolHandler for Mudo {
    async fn accept(&self, conn: Connection) -> Result<(), AcceptError> {
        self.0.notify_one();
        conn.closed().await;
        Ok(())
    }
}

#[tokio::test]
async fn descarga_inactiva_libera_turno_con_fin_red() {
    let c = Carpeta::nueva();
    let b = equipo(&c, "b", Duration::from_secs(1)).await;
    let ep = Endpoint::builder(presets::Minimal)
        .relay_mode(RelayMode::Disabled)
        .bind()
        .await
        .unwrap();
    let iniciado = Arc::new(Notify::new());
    let router = Router::builder(ep)
        .accept(iroh_blobs::ALPN, Mudo(iniciado.clone()))
        .spawn();
    let (_conn, mut tx, mut rx) = ofrecer_manual(router.endpoint(), &b, Hash::new(b"x"), 1).await;
    let o = evento(&b, "oferta").await;
    b.responder(
        o["oferta"].as_u64().unwrap(),
        vec![0],
        Some(c.0.join("destino")),
    )
    .unwrap();
    marco::leer(&mut rx, 4096, 28).await.unwrap();
    escribir(&mut tx, &json!({"type": "listo"})).await.unwrap();
    tokio::time::timeout(Duration::from_secs(3), iniciado.notified())
        .await
        .unwrap();
    assert_eq!(evento(&b, "fin").await["error"], "red");
    assert_eq!(b.0.turnos_recepcion.available_permits(), TRANSFERENCIAS);
    assert_eq!(*b.0.comprometido.lock().unwrap(), 0);
    b.cerrar().await;
    router.shutdown().await.unwrap();
}

#[tokio::test]
async fn admision_por_par_antes_del_cuerpo() {
    let c = Carpeta::nueva();
    let a = equipo(&c, "a", Duration::from_secs(10)).await;
    let b = equipo(&c, "b", Duration::from_secs(10)).await;
    let otro = equipo(&c, "otro", Duration::from_secs(10)).await;
    let mut abiertos = Vec::new();
    for _ in 0..2 {
        abiertos.push(ofrecer_manual(a.0.router.endpoint(), &b, Hash::new(b"x"), 1).await);
        evento(&b, "oferta").await;
    }
    let conn =
        a.0.router
            .endpoint()
            .connect(b.0.router.endpoint().addr(), ALPN)
            .await
            .unwrap();
    let (mut tx, mut rx) = conn.open_bi().await.unwrap();
    tx.write_all(&[0]).await.unwrap();
    let r = tokio::time::timeout(Duration::from_secs(2), marco::leer(&mut rx, 4096, 28))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(r["type"], "ocupado");
    abiertos.push(ofrecer_manual(otro.0.router.endpoint(), &b, Hash::new(b"x"), 1).await);
    assert_eq!(evento(&b, "oferta").await["de"], otro.clave().to_string());
    a.cerrar().await;
    otro.cerrar().await;
    b.cerrar().await;
}

#[test]
fn cola_acotada_conserva_eventos_obligatorios() {
    let mut cola = ColaEventos::default();
    for id in 0..(MAX_EVENTOS / 2) as u64 {
        assert!(cola.reservar(id));
    }
    assert!(!cola.reservar(u64::MAX));
    for _ in 0..100 {
        cola.emitir(json!({"type": "avance"}));
    }
    assert!(cola.cola.is_empty());
    for id in 0..(MAX_EVENTOS / 2) as u64 {
        if id % 2 == 0 {
            cola.emitir(json!({"type": "oferta", "oferta": id}));
            cola.emitir(json!({"type": "oferta-retirada", "oferta": id}));
        } else {
            cola.emitir(json!({"type": "respuesta", "envio": id}));
            cola.emitir(json!({"type": "fin", "id": id}));
        }
    }
    for _ in 0..100 {
        cola.emitir(json!({"type": "cercano"}));
    }
    assert_eq!(cola.cola.len(), MAX_EVENTOS);
    for tipo in ["fin", "oferta", "respuesta", "oferta-retirada"] {
        assert_eq!(
            cola.cola.iter().filter(|e| e["type"] == tipo).count(),
            MAX_EVENTOS / 4
        );
    }
    cola.cola.clear();
    assert!(cola.reservar(u64::MAX));
}

#[tokio::test]
async fn aceptar_descuenta_espacio_comprometido_y_lo_libera() {
    let c = Carpeta::nueva();
    let e = equipo(&c, "e", Duration::from_secs(10)).await;
    let libre = fs4::available_space(&c.0).unwrap();
    let rx1 = pendiente(&e, 1, libre / 3);
    let rx2 = pendiente(&e, 2, libre / 3);
    e.responder(1, vec![0], Some(c.0.join("destino"))).unwrap();
    assert_eq!(
        e.responder(2, vec![0], Some(c.0.join("destino")))
            .unwrap_err()
            .0,
        "espacio"
    );
    assert!(*e.0.comprometido.lock().unwrap() > 0);
    drop(rx1.await.unwrap());
    assert_eq!(*e.0.comprometido.lock().unwrap(), 0);
    e.responder(2, vec![0], Some(c.0.join("destino"))).unwrap();
    drop(rx2.await.unwrap());
    assert_eq!(*e.0.comprometido.lock().unwrap(), 0);
    e.cerrar().await;
}

#[tokio::test]
async fn permisos_compartidos_revocan_solo_un_envio_y_reparten_progreso() {
    let c = Carpeta::nueva();
    let e = equipo(&c, "e", Duration::from_secs(10)).await;
    let de = SecretKey::generate().public();
    let h = Hash::new(b"x");
    e.autorizar(de, 1, std::iter::once(h));
    e.autorizar(de, 2, std::iter::once(h));
    e.progreso_servido(de, h, 100);
    for id in [1, 2] {
        assert_eq!(e.0.servidos.lock().unwrap()[&id][&h], 100);
    }
    e.revocar(2);
    assert_eq!(e.0.permisos.lock().unwrap()[&de][&h], HashSet::from([1]));
    e.progreso_servido(de, h, 200);
    assert_eq!(e.0.servidos.lock().unwrap()[&1][&h], 200);
    assert_eq!(e.0.servidos.lock().unwrap()[&2][&h], 100);
    e.revocar(1);
    assert!(!e.0.permisos.lock().unwrap().contains_key(&de));
    e.cerrar().await;
}

#[tokio::test]
async fn aceptar_gana_a_retirar_sin_perder_la_respuesta() {
    let c = Carpeta::nueva();
    let e = equipo(&c, "e", Duration::from_secs(10)).await;
    let _eventos = e.reservar_eventos(1).unwrap();
    let mut rx = pendiente(&e, 1, 1);
    e.responder(1, vec![0], Some(c.0.join("destino"))).unwrap();
    // La caducidad ya fue elegida por select!, pero aceptar gana el bloqueo de pendientes.
    let respuesta = e.retirar(1, &mut rx);
    assert!(matches!(respuesta, Some(Respuesta::Aceptar(..))));
    assert!(e.0.eventos.lock().unwrap().cola.is_empty());
    drop(respuesta);
    let _eventos = e.reservar_eventos(2).unwrap();
    let mut rx = pendiente(&e, 2, 1);
    assert!(e.retirar(2, &mut rx).is_none());
    assert_eq!(
        e.responder(2, vec![0], Some(c.0.join("destino")))
            .unwrap_err()
            .0,
        "desconocido"
    );
    let rx = pendiente(&e, 3, 1);
    drop(rx);
    assert_eq!(
        e.responder(3, vec![0], Some(c.0.join("destino")))
            .unwrap_err()
            .0,
        "desconocido"
    );
    assert!(!e.0.activos.lock().unwrap().contains_key(&3));
    assert_eq!(*e.0.comprometido.lock().unwrap(), 0);
    e.cerrar().await;
}

#[tokio::test]
async fn aceptacion_confirmada_da_fin_aunque_se_abandone_la_tarea() {
    let c = Carpeta::nueva();
    let e = equipo(&c, "e", Duration::from_secs(10)).await;
    let rx = pendiente(&e, 1, 1);
    e.responder(1, vec![0], Some(c.0.join("destino"))).unwrap();
    drop(rx);
    let fin = evento(&e, "fin").await;
    assert_eq!(fin["id"], 1);
    assert_eq!(fin["error"], "cancelado");
    assert!(e.0.activos.lock().unwrap().is_empty());
    assert_eq!(*e.0.comprometido.lock().unwrap(), 0);
    assert!(e.0.eventos.lock().unwrap().cola.is_empty());
    e.cerrar().await;
}

async fn reservas(e: &Equipo, hash: Hash) -> Vec<String> {
    e.0.store
        .tags()
        .list_prefix(format!("rx/{}/", hash.to_hex()))
        .await
        .unwrap()
        .map(|t| String::from_utf8_lossy(t.unwrap().name.as_ref()).into_owned())
        .collect()
        .await
}

#[tokio::test]
async fn completar_y_cancelar_liberan_todas_las_reservas() {
    let c = Carpeta::nueva();
    let e = equipo(&c, "e", Duration::from_secs(10)).await;
    let tag = e.0.store.blobs().add_slice(b"contenido").await.unwrap();
    let h = tag.hash;
    let a = Archivo {
        nombre: "a.bin".into(),
        tamano: 9,
        hash: h,
    };
    for n in [1, 2] {
        e.0.store
            .tags()
            .set(format!("rx/{}/{}", h.to_hex(), ahora() - n), h)
            .await
            .unwrap();
    }
    e.bajar(
        1,
        e.clave(),
        &[&a],
        &c.0.join("destino"),
        &CancellationToken::new(),
    )
    .await
    .unwrap();
    assert!(reservas(&e, h).await.is_empty());
    let h2 = Hash::new(b"pendiente");
    for n in [1, 2] {
        e.0.store
            .tags()
            .set(format!("rx/{}/{}", h2.to_hex(), ahora() - n), h2)
            .await
            .unwrap();
    }
    let cancelado = CancellationToken::new();
    cancelado.cancel();
    let a = Archivo {
        nombre: "b.bin".into(),
        tamano: 9,
        hash: h2,
    };
    assert_eq!(
        e.bajar(2, e.clave(), &[&a], &c.0.join("destino"), &cancelado)
            .await,
        Err("cancelado")
    );
    assert!(reservas(&e, h2).await.is_empty());
    e.cerrar().await;
}

#[tokio::test]
async fn reservas_viejas_caducan_sin_reabrir() {
    let c = Carpeta::nueva();
    let e = equipo(&c, "e", Duration::from_secs(10)).await;
    let h = Hash::new(b"viejo");
    let vieja = format!("rx/{}/{}", h.to_hex(), ahora() - RESERVA_DIAS * 86_400 - 10);
    e.0.store.tags().set(&vieja, h).await.unwrap();
    tokio::time::timeout(Duration::from_secs(2), async {
        while !reservas(&e, h).await.is_empty() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    e.cerrar().await;
}

#[tokio::test]
async fn reanudar_reutiliza_sufijo_incluso_con_huecos() {
    let c = Carpeta::nueva();
    let e = equipo(&c, "e", Duration::from_secs(10)).await;
    let destino = c.0.join("destino");
    std::fs::create_dir_all(&destino).unwrap();
    std::fs::write(destino.join("a.txt"), b"otro").unwrap();
    std::fs::write(destino.join("a (3).txt"), b"correcto").unwrap();
    let a = Archivo {
        nombre: "a.txt".into(),
        tamano: 8,
        hash: Hash::new(b"correcto"),
    };
    let rutas = e
        .bajar(1, e.clave(), &[&a], &destino, &CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(rutas, [destino.join("a (3).txt").to_string_lossy()]);
    assert_eq!(std::fs::read_dir(destino).unwrap().count(), 2);
    e.cerrar().await;
}
