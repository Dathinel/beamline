//! Un equipo de BeamLine: endpoint de iroh, almacén de iroh-blobs, ofertas y transferencias (docs/protocolo.md).

use std::{
    collections::{HashMap, HashSet, VecDeque},
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use dathinel_red::{marco, modulo::Fallo};
use iroh::{
    Endpoint, EndpointAddr, EndpointId, RelayMode, RelayUrl, SecretKey,
    endpoint::{Connection, RecvStream, SendStream, VarInt, presets},
    protocol::{AcceptError, ProtocolHandler, Router},
};
use iroh_blobs::{
    BlobFormat, BlobsProtocol, Hash,
    api::{
        TempTag,
        blobs::{AddPathOptions, ExportMode, ExportOptions, ImportMode},
        downloader::DownloadProgressItem,
    },
    provider::events::{
        AbortReason, ConnectMode, EventMask, EventSender, ObserveMode, ProviderMessage,
        RequestMode, RequestUpdate,
    },
    store::{
        GcConfig,
        fs::{FsStore, options::Options},
    },
};
use n0_future::StreamExt;
use serde_json::{Value, json};
use tokio::sync::{Notify, Semaphore, oneshot};
use tokio_util::sync::CancellationToken;

use crate::{billete, nombres};

pub const ALPN: &[u8] = b"beamline/1";
pub const MAX_ARCHIVOS: usize = 4096;
const MAX_PENDIENTES: usize = 4;
const OFERTAS_POR_MINUTO: usize = 10;
const TRANSFERENCIAS: usize = 4;
const AVANCE: Duration = Duration::from_millis(250);
const MAX_EVENTOS: usize = 4096;
const ESPERA_CONEXION: Duration = Duration::from_secs(30);
const RESERVA_DIAS: u64 = 7;
/// Una transferencia sin un byte servido en este tiempo se da por perdida y suelta su turno.
const INACTIVIDAD: Duration = Duration::from_secs(600);

/// Códigos de cierre de la conexión `beamline/1`.
mod cierre {
    pub const BIEN: u32 = 0;
    pub const CANCELADO: u32 = 1;
    pub const MARCO: u32 = 2;
    pub const RITMO: u32 = 3;
    pub const CERRADO: u32 = 4;
}

pub enum Rele {
    N0,
    Ninguno,
    Url(RelayUrl),
}

pub struct Config {
    pub identidad: SecretKey,
    pub alias: String,
    pub almacen: PathBuf,
    pub rele: Rele,
    pub solo_rele: bool,
    pub mdns: bool,
    pub tamano_max: u64,
    /// Sin verificar el certificado del relé (solo el relé local de pruebas).
    pub inseguro: bool,
    /// Cada cuánto se limpia el almacén.
    pub limpieza: Duration,
    /// Cuánto espera una oferta su respuesta.
    pub espera_respuesta: Duration,
}

struct Archivo {
    nombre: String,
    tamano: u64,
    hash: Hash,
}

enum Respuesta {
    Rechazar,
    Aceptar(Vec<usize>, PathBuf),
}

struct Pendiente {
    responder: oneshot::Sender<Respuesta>,
    /// Pasa a `activos` al aceptar, para que `cancelar` funcione desde ese instante.
    cancelar: CancellationToken,
    archivos: Vec<(String, u64)>,
}

#[derive(Clone)]
pub struct Equipo(Arc<Interno>);

struct Interno {
    router: Router,
    store: FsStore,
    almacen: PathBuf,
    alias: String,
    tamano_max: u64,
    espera_respuesta: Duration,
    eventos: Mutex<VecDeque<Value>>,
    hay_eventos: Notify,
    siguiente: AtomicU64,
    /// Envíos y recepciones en curso, y ofertas recibidas sin responder.
    activos: Mutex<HashMap<u64, CancellationToken>>,
    pendientes: Mutex<HashMap<u64, Pendiente>>,
    /// Qué huellas puede pedir cada equipo, y para qué envío (docs/protocolo.md §3).
    permisos: Mutex<HashMap<EndpointId, HashMap<Hash, u64>>>,
    /// Bytes servidos por envío y huella, del registro de iroh-blobs.
    servidos: Mutex<HashMap<u64, HashMap<Hash, u64>>>,
    cercanos: Mutex<HashMap<EndpointId, String>>,
    ritmo: Mutex<HashMap<EndpointId, VecDeque<Instant>>>,
    turnos_envio: Semaphore,
    turnos_recepcion: Semaphore,
    cerrar: CancellationToken,
}

fn fallo<T>(code: &'static str, msg: impl Into<String>) -> Result<T, Fallo> {
    Err(Fallo(code, msg.into()))
}

fn ahora() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

impl Equipo {
    pub async fn abrir(c: Config) -> Result<Self, Fallo> {
        if c.alias.trim().is_empty() || c.alias.chars().count() > 64 || c.alias.len() > 240 {
            return fallo(
                "invalid-parameters",
                "El alias tiene que tener de 1 a 64 caracteres.",
            );
        }
        if !c.almacen.is_absolute() {
            return fallo(
                "invalid-parameters",
                "«almacen» tiene que ser una ruta absoluta.",
            );
        }
        let mut gc = Options::new(&c.almacen);
        gc.gc = Some(GcConfig {
            interval: c.limpieza,
            add_protected: None,
        });
        let store = FsStore::load_with_opts(c.almacen.join("blobs.db"), gc)
            .await
            .map_err(|e| Fallo("disco", format!("No se pudo abrir el almacén: {e}")))?;

        let mut b = match &c.rele {
            Rele::N0 => Endpoint::builder(presets::N0),
            Rele::Ninguno => Endpoint::builder(presets::Minimal).relay_mode(RelayMode::Disabled),
            Rele::Url(u) => {
                Endpoint::builder(presets::Minimal).relay_mode(RelayMode::Custom(u.clone().into()))
            }
        };
        if c.solo_rele {
            if matches!(c.rele, Rele::Ninguno) {
                return fallo("invalid-parameters", "«soloRele» necesita un relé.");
            }
            b = b.clear_ip_transports();
        }
        #[cfg(feature = "prueba")]
        if c.inseguro {
            b = b.ca_tls_config(iroh::tls::CaTlsConfig::insecure_skip_verify());
        }
        #[cfg(not(feature = "prueba"))]
        if c.inseguro {
            return fallo(
                "invalid-parameters",
                "«releInseguro» solo existe en el binario de pruebas.",
            );
        }
        let mdns = if c.mdns {
            let m = iroh_mdns_address_lookup::MdnsAddressLookup::builder()
                .service_name("beamline")
                .build(c.identidad.public())
                .map_err(|e| Fallo("sin-conexion", format!("No se pudo abrir mDNS: {e}")))?;
            b = b.address_lookup(m.clone());
            Some(m)
        } else {
            None
        };
        if let Ok(datos) = c.alias.parse() {
            b = b.user_data_for_address_lookup(datos);
        }
        let ep = b
            .secret_key(c.identidad)
            .bind()
            .await
            .map_err(|e| Fallo("sin-conexion", format!("No se pudo abrir la red: {e}")))?;
        if !matches!(c.rele, Rele::Ninguno) {
            let _ = tokio::time::timeout(Duration::from_secs(5), ep.online()).await;
        }

        let (eventos_blobs, registro) = EventSender::channel(
            64,
            EventMask {
                connected: ConnectMode::Intercept,
                get: RequestMode::InterceptLog,
                get_many: RequestMode::Disabled,
                push: RequestMode::Disabled,
                observe: ObserveMode::Disabled,
                ..EventMask::DEFAULT
            },
        );
        let blobs = BlobsProtocol::new(&store, Some(eventos_blobs));
        let equipo = Arc::new_cyclic(|debil: &std::sync::Weak<Interno>| {
            let router = Router::builder(ep)
                .accept(ALPN, Ofertas(debil.clone()))
                .accept(iroh_blobs::ALPN, blobs)
                .spawn();
            Interno {
                router,
                store,
                almacen: c.almacen.clone(),
                alias: c.alias,
                tamano_max: c.tamano_max,
                espera_respuesta: c.espera_respuesta,
                eventos: Mutex::default(),
                hay_eventos: Notify::new(),
                siguiente: AtomicU64::new(1),
                activos: Mutex::default(),
                pendientes: Mutex::default(),
                permisos: Mutex::default(),
                servidos: Mutex::default(),
                cercanos: Mutex::default(),
                ritmo: Mutex::default(),
                turnos_envio: Semaphore::new(TRANSFERENCIAS),
                turnos_recepcion: Semaphore::new(TRANSFERENCIAS),
                cerrar: CancellationToken::new(),
            }
        });
        let e = Equipo(equipo);
        tokio::spawn(e.clone().vigilar_blobs(registro));
        if let Some(m) = mdns {
            tokio::spawn(e.clone().vigilar_cercanos(m));
        }
        e.limpiar_reservas_viejas().await;
        Ok(e)
    }

    pub fn clave(&self) -> EndpointId {
        self.0.router.endpoint().id()
    }

    pub fn billete(&self) -> String {
        billete::crear(&self.0.router.endpoint().addr())
    }

    pub fn estado(&self) -> Value {
        let cercanos: Vec<Value> = self
            .0
            .cercanos
            .lock()
            .unwrap()
            .iter()
            .map(|(k, a)| json!({"clave": k.to_string(), "alias": a}))
            .collect();
        let rele = self
            .0
            .router
            .endpoint()
            .addr()
            .relay_urls()
            .next()
            .is_some();
        json!({"clave": self.clave().to_string(), "alias": self.0.alias, "billete": self.billete(),
               "cercanos": cercanos, "releConectado": rele})
    }

    pub async fn cerrar(&self) {
        self.0.cerrar.cancel();
        for t in self.0.activos.lock().unwrap().values() {
            t.cancel();
        }
        self.0.pendientes.lock().unwrap().clear();
        let _ = self.0.router.shutdown().await;
        let _ = self.0.store.shutdown().await;
    }

    fn emitir(&self, ev: Value) {
        let mut cola = self.0.eventos.lock().unwrap();
        // Con la cola llena se pierde antes un avance que un fin.
        if cola.len() >= MAX_EVENTOS && ev["type"] == "avance" {
            return;
        }
        cola.push_back(ev);
        drop(cola);
        self.0.hay_eventos.notify_one();
    }

    /// Espera eventos hasta `espera` y devuelve los que haya.
    pub async fn recibir(&self, espera: Duration, cancelar: &Notify) -> Vec<Value> {
        let hasta = tokio::time::Instant::now() + espera;
        loop {
            let aviso = self.0.hay_eventos.notified();
            {
                let mut cola = self.0.eventos.lock().unwrap();
                if !cola.is_empty() {
                    return cola.drain(..).collect();
                }
            }
            tokio::select! {
                _ = aviso => {}
                _ = tokio::time::sleep_until(hasta) => return Vec::new(),
                _ = cancelar.notified() => return Vec::new(),
                _ = self.0.cerrar.cancelled() => return Vec::new(),
            }
        }
    }

    fn nuevo_id(&self) -> u64 {
        self.0.siguiente.fetch_add(1, Ordering::Relaxed)
    }

    // ---------- Enviar ----------

    /// Valida la oferta y la manda en segundo plano; devuelve el id del envío.
    pub fn ofrecer(&self, a: &str, archivos: Vec<(PathBuf, String)>) -> Result<u64, Fallo> {
        let destino = if a.trim().starts_with("beam1") {
            billete::leer(a).ok_or(Fallo("billete-invalido", "El billete no es válido.".into()))?
        } else {
            let id: EndpointId = a.parse().map_err(|_| {
                Fallo(
                    "invalid-parameters",
                    "«a» tiene que ser una clave o un billete.".into(),
                )
            })?;
            EndpointAddr::new(id)
        };
        if destino.id == self.clave() {
            return fallo(
                "invalid-parameters",
                "No se puede enviar a este mismo equipo.",
            );
        }
        if archivos.is_empty() || archivos.len() > MAX_ARCHIVOS {
            return fallo(
                "demasiado-grande",
                format!("Una oferta lleva de 1 a {MAX_ARCHIVOS} archivos."),
            );
        }
        let mut vistos = HashSet::new();
        for (i, (ruta, nombre)) in archivos.iter().enumerate() {
            if !nombres::valido(nombre) || !vistos.insert(nombre.to_lowercase()) {
                return Err(Fallo(
                    "nombre-invalido",
                    format!("El nombre {i} no es válido o está repetido."),
                ));
            }
            if !ruta.is_absolute() || !ruta.is_file() {
                return Err(Fallo(
                    "invalid-parameters",
                    format!("El archivo {i} no existe o no es una ruta absoluta."),
                ));
            }
        }
        let id = self.nuevo_id();
        let cancelar = self.0.cerrar.child_token();
        self.0.activos.lock().unwrap().insert(id, cancelar.clone());
        let e = self.clone();
        tokio::spawn(async move {
            let r = e.enviar(id, destino, archivos, &cancelar).await;
            e.0.activos.lock().unwrap().remove(&id);
            e.0.servidos.lock().unwrap().remove(&id);
            for hashes in e.0.permisos.lock().unwrap().values_mut() {
                hashes.retain(|_, envio| *envio != id);
            }
            e.emitir(match r {
                Ok(()) => json!({"type": "fin", "id": id, "ok": true}),
                Err(error) => json!({"type": "fin", "id": id, "ok": false, "error": error}),
            });
        });
        Ok(id)
    }

    async fn enviar(
        &self,
        id: u64,
        destino: EndpointAddr,
        rutas: Vec<(PathBuf, String)>,
        cancelar: &CancellationToken,
    ) -> Result<(), &'static str> {
        // Las etiquetas temporales protegen los blobs de la limpieza mientras dure el envío.
        let mut etiquetas: Vec<TempTag> = Vec::new();
        let mut archivos = Vec::new();
        let mut firmas = Vec::new();
        for (ruta, nombre) in &rutas {
            let meta = std::fs::metadata(ruta).map_err(|_| "disco")?;
            firmas.push((meta.len(), meta.modified().ok()));
            let importar = self.0.store.blobs().add_path_with_opts(AddPathOptions {
                path: ruta.clone(),
                format: BlobFormat::Raw,
                mode: ImportMode::TryReference,
            });
            let tt = tokio::select! {
                t = importar.temp_tag() => t.map_err(|_| "disco")?,
                _ = cancelar.cancelled() => return Err("cancelado"),
            };
            archivos.push(Archivo {
                nombre: nombre.clone(),
                tamano: meta.len(),
                hash: tt.hash(),
            });
            etiquetas.push(tt);
        }

        let ep = self.0.router.endpoint();
        let conn = tokio::select! {
            c = tokio::time::timeout(ESPERA_CONEXION, ep.connect(destino.clone(), ALPN)) => match c {
                Ok(Ok(c)) => c,
                _ => return Err("sin-conexion"),
            },
            _ = cancelar.cancelled() => return Err("cancelado"),
        };
        let r = self.ofrecer_por(&conn, id, &archivos, cancelar).await;
        // Si se cierra la app, el otro ve que este equipo se fue (`red`), no una cancelación.
        let codigo = match r {
            _ if self.0.cerrar.is_cancelled() => cierre::CERRADO,
            Err("cancelado") => cierre::CANCELADO,
            _ => cierre::BIEN,
        };
        conn.close(VarInt::from_u32(codigo), b"");
        // Si el otro no pudo bajar algo y el archivo cambió desde que se leyó, la causa es esa.
        if r.is_err() && r != Err("cancelado") {
            let cambio = rutas
                .iter()
                .zip(&firmas)
                .any(|((ruta, _), (largo, fecha))| {
                    std::fs::metadata(ruta)
                        .map(|m| m.len() != *largo || m.modified().ok() != *fecha)
                        .unwrap_or(true)
                });
            if cambio {
                return Err("huella");
            }
        }
        drop(etiquetas);
        r
    }

    async fn ofrecer_por(
        &self,
        conn: &Connection,
        id: u64,
        archivos: &[Archivo],
        cancelar: &CancellationToken,
    ) -> Result<(), &'static str> {
        let (mut tx, rx) = conn.open_bi().await.map_err(|_| "sin-conexion")?;
        let lista: Vec<Value> = archivos
            .iter()
            .map(|a| json!({"nombre": a.nombre, "tamano": a.tamano, "hash": a.hash.to_hex()}))
            .collect();
        let oferta =
            json!({"type": "oferta", "version": 1, "alias": self.0.alias, "archivos": lista});
        let bytes = marco::armar(&oferta);
        if bytes.len() - 4 > marco::MAX_PAR {
            return Err("demasiado-grande");
        }
        tx.write_all(&bytes).await.map_err(|_| "sin-conexion")?;

        let mut lector = leer_marcos(rx);
        let respuesta = tokio::select! {
            m = lector.recv() => m.ok_or("red")?,
            _ = cancelar.cancelled() => return Err("cancelado"),
        };
        match respuesta["type"].as_str() {
            Some("ocupado") => return Err("ocupado"),
            Some("version") => return Err("version"),
            Some("respuesta") => {}
            _ => return Err("red"),
        }
        let mut aceptados = Vec::new();
        for i in respuesta["aceptados"].as_array().ok_or("red")? {
            let i = i
                .as_u64()
                .filter(|i| (*i as usize) < archivos.len())
                .ok_or("red")? as usize;
            if !aceptados.contains(&i) {
                aceptados.push(i);
            }
        }
        self.emitir(json!({"type": "respuesta", "envio": id, "aceptados": aceptados}));
        if aceptados.is_empty() {
            return Err("rechazado");
        }
        // El turno cuenta desde que el otro acepta: esperar respuesta no ocupa transferencia.
        let _turno = tokio::select! {
            t = self.0.turnos_envio.acquire() => t.map_err(|_| "cancelado")?,
            _ = cancelar.cancelled() => return Err("cancelado"),
        };
        let total: u64 = aceptados.iter().map(|&i| archivos[i].tamano).sum();
        {
            let mut p = self.0.permisos.lock().unwrap();
            let de = p.entry(conn.remote_id()).or_default();
            for &i in &aceptados {
                de.insert(archivos[i].hash, id);
            }
            self.0.servidos.lock().unwrap().insert(id, HashMap::new());
        }
        // Con turno y permisos listos, el otro ya puede pedir los datos.
        escribir(&mut tx, &json!({"type": "listo"}))
            .await
            .map_err(|_| "red")?;
        let mut reloj = tokio::time::interval(AVANCE);
        let mut ultimo = u64::MAX;
        let mut quieto = Instant::now();
        loop {
            tokio::select! {
                m = lector.recv() => {
                    let fin = m.ok_or("red")?;
                    if fin["type"] != "fin" {
                        return Err("red");
                    }
                    self.emitir(json!({"type": "avance", "id": id, "bytes": total, "total": total}));
                    return if fin["ok"] == true { Ok(()) } else {
                        Err(match fin["error"].as_str() {
                            Some("cancelado") => "cancelado",
                            Some("disco") => "disco",
                            Some("huella") => "huella",
                            _ => "red",
                        })
                    };
                }
                _ = reloj.tick() => {
                    let hecho: u64 = self.0.servidos.lock().unwrap().get(&id).map(|m| m.values().sum()).unwrap_or(0);
                    if hecho != ultimo {
                        ultimo = hecho;
                        quieto = Instant::now();
                        self.emitir(json!({"type": "avance", "id": id, "bytes": hecho.min(total), "total": total}));
                    } else if quieto.elapsed() > INACTIVIDAD {
                        return Err("red");
                    }
                }
                _ = cancelar.cancelled() => return Err("cancelado"),
            }
        }
    }

    /// Solo se sirve lo aceptado a quien lo aceptó; y se cuenta lo servido para el avance.
    async fn vigilar_blobs(self, mut registro: tokio::sync::mpsc::Receiver<ProviderMessage>) {
        let mut conexiones: HashMap<u64, EndpointId> = HashMap::new();
        while let Some(m) = registro.recv().await {
            match m {
                ProviderMessage::ClientConnected(m) => {
                    let ok = m.endpoint_id.filter(|id| {
                        self.0
                            .permisos
                            .lock()
                            .unwrap()
                            .get(id)
                            .is_some_and(|h| !h.is_empty())
                    });
                    if let Some(id) = ok {
                        conexiones.insert(m.connection_id, id);
                    }
                    m.tx.send(ok.map(|_| ()).ok_or(AbortReason::Permission))
                        .await
                        .ok();
                }
                ProviderMessage::ConnectionClosed(m) => {
                    conexiones.remove(&m.connection_id);
                }
                ProviderMessage::GetRequestReceived(mut m) => {
                    let hash = m.request.hash;
                    let envio = conexiones
                        .get(&m.connection_id)
                        .filter(|_| m.request.ranges.is_blob())
                        .and_then(|id| {
                            self.0
                                .permisos
                                .lock()
                                .unwrap()
                                .get(id)
                                .and_then(|h| h.get(&hash).copied())
                        });
                    m.tx.send(envio.map(|_| ()).ok_or(AbortReason::Permission))
                        .await
                        .ok();
                    if let Some(envio) = envio {
                        let e = self.clone();
                        tokio::spawn(async move {
                            while let Ok(Some(u)) = m.rx.recv().await {
                                if let RequestUpdate::Progress(p) = u
                                    && let Some(s) = e.0.servidos.lock().unwrap().get_mut(&envio)
                                {
                                    s.insert(hash, p.end_offset);
                                }
                            }
                        });
                    }
                }
                _ => {}
            }
        }
    }

    // ---------- Recibir ----------

    /// Responde a una oferta recibida: aceptar índices hacia `destino` o rechazar.
    pub fn responder(
        &self,
        oferta: u64,
        aceptar: Vec<usize>,
        destino: Option<PathBuf>,
    ) -> Result<(), Fallo> {
        let mut pend = self.0.pendientes.lock().unwrap();
        let Some(p) = pend.get(&oferta) else {
            return fallo("desconocido", "Esa oferta no existe o ya tiene respuesta.");
        };
        let respuesta = if aceptar.is_empty() {
            Respuesta::Rechazar
        } else {
            let Some(destino) = destino.filter(|d| d.is_absolute()) else {
                return fallo(
                    "invalid-parameters",
                    "«destino» tiene que ser una carpeta absoluta.",
                );
            };
            let mut indices = Vec::new();
            for i in aceptar {
                if i >= p.archivos.len() {
                    return fallo(
                        "invalid-parameters",
                        format!("La oferta no tiene el archivo {i}."),
                    );
                }
                if !indices.contains(&i) {
                    indices.push(i);
                }
            }
            let total: u64 = indices.iter().map(|&i| p.archivos[i].1).sum();
            std::fs::create_dir_all(&destino)
                .map_err(|e| Fallo("disco", format!("No se pudo crear la carpeta: {e}")))?;
            // Exportar con Copy deja una segunda copia en el almacén hasta la próxima limpieza.
            // shortcut: «mismo volumen» es la misma raíz de ruta; en Linux siempre lo es y pide el doble aunque sean discos distintos.
            let mismo = destino.components().next() == self.0.almacen.components().next();
            for (carpeta, hace_falta) in [
                (
                    destino.as_path(),
                    if mismo {
                        total.saturating_mul(2)
                    } else {
                        total
                    },
                ),
                (self.0.almacen.as_path(), total),
            ] {
                let libre =
                    fs4::available_space(carpeta).map_err(|e| Fallo("disco", e.to_string()))?;
                if libre < hace_falta {
                    return Err(Fallo(
                        "espacio",
                        format!(
                            "Faltan {} bytes libres en {}.",
                            hace_falta - libre,
                            carpeta.display()
                        ),
                    ));
                }
            }
            Respuesta::Aceptar(indices, destino)
        };
        let p = pend.remove(&oferta).expect("comprobada arriba");
        if matches!(respuesta, Respuesta::Aceptar(..)) {
            self.0
                .activos
                .lock()
                .unwrap()
                .insert(oferta, p.cancelar.clone());
        }
        let _ = p.responder.send(respuesta);
        Ok(())
    }

    pub fn cancelar(&self, id: u64) -> Result<(), Fallo> {
        if let Some(t) = self.0.activos.lock().unwrap().get(&id) {
            t.cancel();
            return Ok(());
        }
        if self.0.pendientes.lock().unwrap().contains_key(&id) {
            return self.responder(id, Vec::new(), None);
        }
        fallo("desconocido", "No hay ningún envío ni oferta con ese id.")
    }

    fn admitir_ritmo(&self, de: EndpointId) -> bool {
        let mut r = self.0.ritmo.lock().unwrap();
        let ahora = Instant::now();
        r.retain(|_, v| {
            while v
                .front()
                .is_some_and(|t| ahora.duration_since(*t) > Duration::from_secs(60))
            {
                v.pop_front();
            }
            !v.is_empty()
        });
        let v = r.entry(de).or_default();
        if v.len() >= OFERTAS_POR_MINUTO {
            return false;
        }
        v.push_back(ahora);
        true
    }

    async fn atender_oferta(&self, conn: Connection) {
        let de = conn.remote_id();
        if !self.admitir_ritmo(de) {
            conn.close(VarInt::from_u32(cierre::RITMO), b"");
            return;
        }
        let Ok(Ok((mut tx, mut rx))) =
            tokio::time::timeout(ESPERA_CONEXION, conn.accept_bi()).await
        else {
            conn.close(VarInt::from_u32(cierre::MARCO), b"");
            return;
        };
        let leido = tokio::time::timeout(
            ESPERA_CONEXION,
            marco::leer(&mut rx, marco::MAX_PAR, marco::MAX_PROFUNDIDAD_PAR),
        )
        .await;
        let Some((alias, archivos)) = leido
            .ok()
            .and_then(Result::ok)
            .as_ref()
            .and_then(leer_oferta)
        else {
            conn.close(VarInt::from_u32(cierre::MARCO), b"");
            return;
        };
        if archivos.is_empty() {
            let _ = escribir(&mut tx, &json!({"type": "version"})).await;
            let _ = tokio::time::timeout(Duration::from_secs(5), conn.closed()).await;
            return;
        }
        let total: u64 = archivos.iter().map(|a| a.tamano).sum();
        if total > self.0.tamano_max {
            let _ = escribir(&mut tx, &json!({"type": "respuesta", "aceptados": []})).await;
            let _ = tokio::time::timeout(Duration::from_secs(5), conn.closed()).await;
            return;
        }
        let id = self.nuevo_id();
        let (responder, respuesta) = oneshot::channel();
        let cancelar = self.0.cerrar.child_token();
        let lleno = {
            let mut pend = self.0.pendientes.lock().unwrap();
            let lleno = pend.len() >= MAX_PENDIENTES;
            if !lleno {
                pend.insert(
                    id,
                    Pendiente {
                        responder,
                        cancelar: cancelar.clone(),
                        archivos: archivos
                            .iter()
                            .map(|a| (a.nombre.clone(), a.tamano))
                            .collect(),
                    },
                );
            }
            lleno
        };
        if lleno {
            let _ = escribir(&mut tx, &json!({"type": "ocupado"})).await;
            let _ = tokio::time::timeout(Duration::from_secs(5), conn.closed()).await;
            return;
        }
        let lista: Vec<Value> = archivos
            .iter()
            .map(|a| json!({"nombre": a.nombre, "tamano": a.tamano}))
            .collect();
        self.emitir(json!({"type": "oferta", "oferta": id, "de": de.to_string(), "alias": alias, "archivos": lista, "total": total}));

        let mut uno = [0u8; 1];
        let respuesta = tokio::select! {
            r = respuesta => r.ok(),
            _ = tokio::time::sleep(self.0.espera_respuesta) => None,
            _ = tokio::io::AsyncReadExt::read(&mut rx, &mut uno) => None,
            _ = self.0.cerrar.cancelled() => None,
        };
        let Some(respuesta) = respuesta else {
            if self.0.pendientes.lock().unwrap().remove(&id).is_some() {
                self.emitir(json!({"type": "oferta-retirada", "oferta": id}));
            }
            let _ = escribir(&mut tx, &json!({"type": "respuesta", "aceptados": []})).await;
            conn.close(VarInt::from_u32(cierre::CERRADO), b"");
            return;
        };
        let Respuesta::Aceptar(indices, destino) = respuesta else {
            let _ = escribir(&mut tx, &json!({"type": "respuesta", "aceptados": []})).await;
            let _ = tokio::time::timeout(Duration::from_secs(5), conn.closed()).await;
            return;
        };
        let elegidos: Vec<&Archivo> = indices.iter().map(|&i| &archivos[i]).collect();
        let r = tokio::select! {
            r = self.recibir_aceptada(id, de, &mut tx, &mut rx, &indices, &elegidos, &destino, &cancelar) => r,
            _ = conn.closed() => Err(if conn.close_reason().is_some_and(|r| es_cancelado(&r)) { "cancelado" } else { "red" }),
        };
        self.0.activos.lock().unwrap().remove(&id);
        let fin = match &r {
            Ok(_) => json!({"type": "fin", "ok": true}),
            Err(e) => json!({"type": "fin", "ok": false, "error": e}),
        };
        let _ = escribir(&mut tx, &fin).await;
        let _ = tx.finish();
        self.emitir(match r {
            Ok(rutas) => json!({"type": "fin", "id": id, "ok": true, "rutas": rutas}),
            Err(e) => json!({"type": "fin", "id": id, "ok": false, "error": e}),
        });
        let _ = tokio::time::timeout(Duration::from_secs(5), conn.closed()).await;
    }

    /// Con la oferta aceptada: turno, respuesta, esperar «listo» del que envía y bajar.
    #[allow(clippy::too_many_arguments)]
    async fn recibir_aceptada(
        &self,
        id: u64,
        de: EndpointId,
        tx: &mut SendStream,
        rx: &mut RecvStream,
        indices: &[usize],
        elegidos: &[&Archivo],
        destino: &Path,
        cancelar: &CancellationToken,
    ) -> Result<Vec<String>, &'static str> {
        let _turno = tokio::select! {
            t = self.0.turnos_recepcion.acquire() => t.map_err(|_| "cancelado")?,
            _ = cancelar.cancelled() => return Err("cancelado"),
        };
        escribir(tx, &json!({"type": "respuesta", "aceptados": indices}))
            .await
            .map_err(|_| "red")?;
        let listo = tokio::select! {
            m = marco::leer(rx, 4096, marco::MAX_PROFUNDIDAD_PAR) => m.map_err(|_| "red")?,
            _ = cancelar.cancelled() => return Err("cancelado"),
        };
        if listo["type"] != "listo" {
            return Err("red");
        }
        self.bajar(id, de, elegidos, destino, cancelar).await
    }

    async fn bajar(
        &self,
        id: u64,
        de: EndpointId,
        archivos: &[&Archivo],
        destino: &Path,
        cancelar: &CancellationToken,
    ) -> Result<Vec<String>, &'static str> {
        let total: u64 = archivos.iter().map(|a| a.tamano).sum();
        let mut hecho = 0u64;
        let mut rutas = Vec::new();
        let mut ultimo = Instant::now() - AVANCE;
        for a in archivos {
            let mut ya = destino.to_path_buf();
            ya.extend(a.nombre.split('/'));
            if igual(&ya, a) {
                hecho += a.tamano;
                rutas.push(ya.to_string_lossy().into_owned());
                continue;
            }
            // La reserva protege lo bajado de la limpieza: si se corta la red, la próxima oferta igual reanuda.
            let reserva = format!("rx/{}/{}", a.hash.to_hex(), ahora());
            self.0
                .store
                .tags()
                .set(&reserva, a.hash)
                .await
                .map_err(|_| "disco")?;
            let r = self
                .bajar_uno(id, de, a, hecho, total, &mut ultimo, cancelar)
                .await;
            if r == Err("cancelado") {
                let _ = self.0.store.tags().delete(&reserva).await;
            }
            r?;
            let ruta = nombres::ruta_libre(destino, &a.nombre).map_err(|_| "disco")?;
            let mut parte = ruta.clone().into_os_string();
            parte.push(".beamline-parte");
            let parte = Temporal(Some(PathBuf::from(parte)));
            let ruta_parte = parte.0.clone().expect("recién creada");
            // shortcut: Copy duplica en disco lo bajado hasta la próxima limpieza; con TryReference no, pero el almacén quedaría apuntando al archivo del usuario.
            let exportar = self.0.store.blobs().export_with_opts(ExportOptions {
                hash: a.hash,
                mode: ExportMode::Copy,
                target: ruta_parte.clone(),
            });
            tokio::select! {
                r = exportar.finish() => { r.map_err(|_| "disco")?; }
                _ = cancelar.cancelled() => return Err("cancelado"),
            }
            let ruta = if ruta.exists() {
                nombres::ruta_libre(destino, &a.nombre).map_err(|_| "disco")?
            } else {
                ruta
            };
            std::fs::rename(&ruta_parte, &ruta).map_err(|_| "disco")?;
            parte.soltar();
            let _ = self.0.store.tags().delete(&reserva).await;
            hecho += a.tamano;
            rutas.push(ruta.to_string_lossy().into_owned());
        }
        self.emitir(json!({"type": "avance", "id": id, "bytes": total, "total": total}));
        Ok(rutas)
    }

    #[allow(clippy::too_many_arguments)]
    async fn bajar_uno(
        &self,
        id: u64,
        de: EndpointId,
        a: &Archivo,
        antes: u64,
        total: u64,
        ultimo: &mut Instant,
        cancelar: &CancellationToken,
    ) -> Result<(), &'static str> {
        let descarga = self
            .0
            .store
            .downloader(self.0.router.endpoint())
            .download(a.hash, Some(de));
        let mut s = descarga.stream().await.map_err(|_| "red")?;
        loop {
            let item = tokio::select! {
                i = s.next() => i,
                _ = cancelar.cancelled() => return Err("cancelado"),
            };
            match item {
                None => break,
                Some(DownloadProgressItem::Progress(n)) => {
                    if ultimo.elapsed() >= AVANCE {
                        *ultimo = Instant::now();
                        self.emitir(json!({"type": "avance", "id": id, "bytes": (antes + n).min(total), "total": total}));
                    }
                }
                Some(DownloadProgressItem::Error(_) | DownloadProgressItem::DownloadError) => {
                    return Err("red");
                }
                Some(_) => {}
            }
        }
        let local = self
            .0
            .store
            .remote()
            .local(a.hash)
            .await
            .map_err(|_| "disco")?;
        if !local.is_complete() || local.local_bytes() != a.tamano {
            return Err("red");
        }
        Ok(())
    }

    /// Las reservas de más de una semana ya no se van a reanudar.
    async fn limpiar_reservas_viejas(&self) {
        let Ok(lista) = self.0.store.tags().list_prefix("rx/").await else {
            return;
        };
        let viejas: Vec<_> = lista
            .filter_map(|t| t.ok())
            .filter_map(|t| {
                let nombre = String::from_utf8_lossy(t.name.as_ref()).into_owned();
                let secs: u64 = nombre.rsplit('/').next()?.parse().ok()?;
                (ahora().saturating_sub(secs) > RESERVA_DIAS * 86_400).then_some(nombre)
            })
            .collect()
            .await;
        for n in viejas {
            let _ = self.0.store.tags().delete(n).await;
        }
    }

    async fn vigilar_cercanos(self, mdns: iroh_mdns_address_lookup::MdnsAddressLookup) {
        let mut s = mdns.subscribe().await;
        loop {
            let ev = tokio::select! {
                e = s.next() => match e { Some(e) => e, None => return },
                _ = self.0.cerrar.cancelled() => return,
            };
            match ev {
                iroh_mdns_address_lookup::DiscoveryEvent::Discovered { endpoint_info, .. } => {
                    let id = endpoint_info.endpoint_id;
                    let alias = endpoint_info
                        .data
                        .user_data()
                        .map(|u| u.to_string())
                        .unwrap_or_else(|| id.fmt_short().to_string());
                    let nuevo = self.0.cercanos.lock().unwrap().insert(id, alias.clone())
                        != Some(alias.clone());
                    if nuevo {
                        self.emitir(json!({"type": "cercano", "clave": id.to_string(), "alias": alias, "visto": true}));
                    }
                }
                iroh_mdns_address_lookup::DiscoveryEvent::Expired { endpoint_id } => {
                    if let Some(alias) = self.0.cercanos.lock().unwrap().remove(&endpoint_id) {
                        self.emitir(json!({"type": "cercano", "clave": endpoint_id.to_string(), "alias": alias, "visto": false}));
                    }
                }
                _ => {}
            }
        }
    }
}

/// Un `.beamline-parte` que se borra solo si no llega a renombrarse (error, cancelación o corte).
struct Temporal(Option<PathBuf>);

impl Temporal {
    fn soltar(mut self) {
        self.0 = None;
    }
}

impl Drop for Temporal {
    fn drop(&mut self) {
        if let Some(p) = &self.0 {
            let _ = std::fs::remove_file(p);
        }
    }
}

/// El archivo ya está en destino con el mismo contenido (una oferta repetida tras un corte).
fn igual(ruta: &Path, a: &Archivo) -> bool {
    if std::fs::metadata(ruta)
        .map(|m| m.is_file() && m.len() == a.tamano)
        .ok()
        != Some(true)
    {
        return false;
    }
    let mut h = blake3::Hasher::new();
    h.update_mmap(ruta).is_ok() && h.finalize().as_bytes() == a.hash.as_bytes()
}

fn es_cancelado(r: &iroh::endpoint::ConnectionError) -> bool {
    matches!(r, iroh::endpoint::ConnectionError::ApplicationClosed(c) if c.error_code == VarInt::from_u32(cierre::CANCELADO))
}

/// Valida un marco `oferta`. Una versión desconocida da una lista vacía (se contesta `version`).
fn leer_oferta(m: &Value) -> Option<(String, Vec<Archivo>)> {
    if m["type"] != "oferta" {
        return None;
    }
    if m["version"] != 1 {
        return Some((String::new(), Vec::new()));
    }
    let alias = m["alias"]
        .as_str()
        .filter(|a| !a.is_empty() && a.chars().count() <= 64)?
        .to_string();
    let lista = m["archivos"]
        .as_array()
        .filter(|l| (1..=MAX_ARCHIVOS).contains(&l.len()))?;
    let mut vistos = HashSet::new();
    let mut archivos = Vec::with_capacity(lista.len());
    for a in lista {
        let nombre = a["nombre"].as_str().filter(|n| nombres::valido(n))?;
        if !vistos.insert(nombre.to_lowercase()) {
            return None;
        }
        let hash: Hash = a["hash"].as_str().filter(|h| h.len() == 64)?.parse().ok()?;
        archivos.push(Archivo {
            nombre: nombre.to_string(),
            tamano: a["tamano"].as_u64()?,
            hash,
        });
    }
    archivos
        .iter()
        .try_fold(0u64, |s, a| s.checked_add(a.tamano))?;
    Some((alias, archivos))
}

async fn escribir(tx: &mut SendStream, m: &Value) -> Result<(), ()> {
    tx.write_all(&marco::armar(m)).await.map_err(|_| ())
}

/// Lee marcos de un stream en una tarea aparte (leer no es seguro de cancelar a medias).
fn leer_marcos(mut rx: RecvStream) -> tokio::sync::mpsc::Receiver<Value> {
    let (tx, salida) = tokio::sync::mpsc::channel(4);
    tokio::spawn(async move {
        while let Ok(m) = marco::leer(&mut rx, marco::MAX_PAR, marco::MAX_PROFUNDIDAD_PAR).await {
            if tx.send(m).await.is_err() {
                break;
            }
        }
    });
    salida
}

#[derive(Debug, Clone)]
struct Ofertas(std::sync::Weak<Interno>);

impl ProtocolHandler for Ofertas {
    async fn accept(&self, conn: Connection) -> Result<(), AcceptError> {
        if let Some(i) = self.0.upgrade() {
            Equipo(i).atender_oferta(conn).await;
        }
        Ok(())
    }
}
