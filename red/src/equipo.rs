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
        RequestMode, RequestUpdate, ThrottleMode,
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
use grupo::Grupo;
use iroh::address_lookup::memory::MemoryLookup;

pub const ALPN: &[u8] = b"beamline/1";
pub const MAX_ARCHIVOS: usize = 4096;
const MAX_PENDIENTES: usize = 4;
const MAX_POR_PAR: usize = 2;
const OFERTAS_POR_MINUTO: usize = 10;
const TRANSFERENCIAS: usize = 4;
const AVANCE: Duration = Duration::from_millis(250);
const MAX_EVENTOS: usize = 4096;
const ESPERA_CONEXION: Duration = Duration::from_secs(30);
const RESERVA_DIAS: u64 = 7;
/// Sin avanzar en este tiempo se da la transferencia por perdida y se suelta su turno.
const INACTIVIDAD: Duration = Duration::from_secs(600);
/// Lo que se espera a que la conexión pase a directa cuando el relé es el de n0.
const ESPERA_DIRECTA: Duration = Duration::from_secs(20);

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
    /// Bytes por segundo que sirve como mucho este equipo (None, sin límite).
    pub limite_subida: Option<u64>,
}

/// El turno de envío que comparte un grupo: lo toma el primero que acepta.
type TurnoGrupo = tokio::sync::OnceCell<tokio::sync::OwnedSemaphorePermit>;

/// Tamaño y fecha de un archivo al importarlo: si cambian, el envío termina con `huella`.
type Firma = (u64, Option<SystemTime>);

struct Archivo {
    nombre: String,
    tamano: u64,
    hash: Hash,
}

enum Respuesta {
    Rechazar,
    Aceptar(Vec<usize>, PathBuf, Recepcion),
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
    eventos: Mutex<ColaEventos>,
    hay_eventos: Notify,
    siguiente: AtomicU64,
    /// Envíos y recepciones en curso, y ofertas recibidas sin responder.
    activos: Mutex<HashMap<u64, CancellationToken>>,
    pendientes: Mutex<HashMap<u64, Pendiente>>,
    /// Qué huellas puede pedir cada equipo, y para qué envío (docs/protocolo.md §3).
    permisos: Mutex<HashMap<EndpointId, HashMap<Hash, HashSet<u64>>>>,
    admision: Mutex<HashMap<EndpointId, usize>>,
    comprometido: Arc<Mutex<u64>>,
    inactividad: Duration,
    /// Bytes servidos por envío y huella, del registro de iroh-blobs.
    servidos: Mutex<HashMap<u64, HashMap<Hash, u64>>>,
    cercanos: Mutex<HashMap<EndpointId, String>>,
    ritmo: Mutex<HashMap<EndpointId, VecDeque<Instant>>>,
    turnos_envio: Arc<Semaphore>,
    turnos_recepcion: Semaphore,
    cerrar: CancellationToken,
    /// Direcciones de los billetes, para que el descargador encuentre a los equipos por su clave.
    memoria: MemoryLookup,
    /// Bytes de datos servidos en total (los de todos los envíos y los que se sirven a un grupo).
    subidos: AtomicU64,
    /// Con el relé de n0, los archivos solo van por conexión directa (fase 0: por n0 iban a 0,04 MiB/s).
    exigir_directa: bool,
    limite: Option<Limitador>,
    /// Último avance de cada recepción, para contárselo al emisor en un grupo.
    progreso: Mutex<HashMap<u64, u64>>,
    /// Lo que un receptor deja pedir a los otros del grupo: solo su trozo de cada huella (id de su recepción).
    solo_trozo: Mutex<HashMap<(u64, EndpointId, Hash), iroh_blobs::protocol::ChunkRanges>>,
}

/// Reparte la subida a un ritmo fijo: cada bloque de 16 KiB espera su turno.
struct Limitador {
    bytes_por_segundo: u64,
    siguiente: Mutex<tokio::time::Instant>,
}

impl Limitador {
    fn turno(&self, bytes: u64) -> tokio::time::Instant {
        let mut s = self.siguiente.lock().unwrap();
        let ahora = tokio::time::Instant::now();
        let t = (*s).max(ahora);
        *s = t + Duration::from_secs_f64(bytes as f64 / self.bytes_por_segundo as f64);
        t
    }
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
        Self::abrir_con_plazo(c, INACTIVIDAD).await
    }

    async fn abrir_con_plazo(c: Config, inactividad: Duration) -> Result<Self, Fallo> {
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
            // Solo relé con el de n0 pasaría los archivos por él, y por ahí no van (fase 0).
            if !matches!(c.rele, Rele::Url(_)) {
                return fallo("invalid-parameters", "«soloRele» necesita un relé propio.");
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
        let mdns = if c.mdns && !c.solo_rele {
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
        let memoria = MemoryLookup::new();
        b = b.address_lookup(memoria.clone());
        if c.limite_subida == Some(0) {
            return fallo(
                "invalid-parameters",
                "«limiteSubida» tiene que ser mayor que 0.",
            );
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
                observe: ObserveMode::Intercept,
                // Cada bloque pasa por el gancho solo si hay límite: sin él no se paga ese viaje.
                throttle: if c.limite_subida.is_some() {
                    ThrottleMode::Intercept
                } else {
                    ThrottleMode::None
                },
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
                admision: Mutex::default(),
                comprometido: Arc::default(),
                inactividad,
                hay_eventos: Notify::new(),
                siguiente: AtomicU64::new(1),
                activos: Mutex::default(),
                pendientes: Mutex::default(),
                permisos: Mutex::default(),
                servidos: Mutex::default(),
                cercanos: Mutex::default(),
                ritmo: Mutex::default(),
                turnos_envio: Arc::new(Semaphore::new(TRANSFERENCIAS)),
                turnos_recepcion: Semaphore::new(TRANSFERENCIAS),
                cerrar: CancellationToken::new(),
                memoria,
                subidos: AtomicU64::new(0),
                exigir_directa: matches!(c.rele, Rele::N0),
                progreso: Mutex::default(),
                solo_trozo: Mutex::default(),
                limite: c.limite_subida.map(|b| Limitador {
                    bytes_por_segundo: b,
                    siguiente: Mutex::new(tokio::time::Instant::now()),
                }),
            }
        });
        let e = Equipo(equipo);
        tokio::spawn(e.clone().vigilar_blobs(registro));
        if let Some(m) = mdns {
            tokio::spawn(e.clone().vigilar_cercanos(m));
        }
        e.limpiar_reservas_viejas().await;
        let limpieza = e.clone();
        tokio::spawn(async move {
            let mut reloj = tokio::time::interval(c.limpieza.max(Duration::from_millis(10)));
            loop {
                tokio::select! {
                    _ = reloj.tick() => limpieza.limpiar_reservas_viejas().await,
                    _ = limpieza.0.cerrar.cancelled() => break,
                }
            }
        });
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
               "cercanos": cercanos, "releConectado": rele,
               "subidos": self.0.subidos.load(Ordering::Relaxed)})
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
        if let Some(id) = ev["id"].as_u64() {
            match ev["type"].as_str() {
                Some("avance") => {
                    self.0
                        .progreso
                        .lock()
                        .unwrap()
                        .insert(id, ev["bytes"].as_u64().unwrap_or(0));
                }
                Some("fin") => {
                    self.0.progreso.lock().unwrap().remove(&id);
                }
                _ => {}
            }
        }
        let mut cola = self.0.eventos.lock().unwrap();
        cola.emitir(ev);
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
                if !cola.cola.is_empty() {
                    return cola.cola.drain(..).collect();
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

    fn reservar_eventos(&self, id: u64) -> Option<EventosReservados> {
        self.0
            .eventos
            .lock()
            .unwrap()
            .reservar(id)
            .then(|| EventosReservados(self.clone(), id))
    }

    fn admitir(&self, de: EndpointId) -> Option<Admision> {
        let mut a = self.0.admision.lock().unwrap();
        if a.values().sum::<usize>() >= MAX_PENDIENTES
            || a.get(&de).copied().unwrap_or(0) >= MAX_POR_PAR
        {
            return None;
        }
        *a.entry(de).or_default() += 1;
        Some(Admision(self.clone(), de))
    }

    fn revocar(&self, id: u64) {
        self.0
            .solo_trozo
            .lock()
            .unwrap()
            .retain(|(i, _, _), _| *i != id);
        self.0.permisos.lock().unwrap().retain(|_, hashes| {
            hashes.retain(|_, envios| {
                envios.remove(&id);
                !envios.is_empty()
            });
            !hashes.is_empty()
        });
    }

    fn autorizar(&self, de: EndpointId, id: u64, hashes: impl Iterator<Item = Hash>) {
        let mut p = self.0.permisos.lock().unwrap();
        let permisos = p.entry(de).or_default();
        for hash in hashes {
            permisos.entry(hash).or_default().insert(id);
        }
        self.0.servidos.lock().unwrap().insert(id, HashMap::new());
    }

    fn progreso_servido(&self, de: EndpointId, hash: Hash, bytes: u64) {
        let permisos = self.0.permisos.lock().unwrap();
        if let Some(envios) = permisos.get(&de).and_then(|h| h.get(&hash)) {
            let mut servidos = self.0.servidos.lock().unwrap();
            for id in envios {
                if let Some(s) = servidos.get_mut(id) {
                    let n = s.entry(hash).or_default();
                    *n = (*n).max(bytes);
                }
            }
        }
    }

    fn retirar(&self, id: u64, respuesta: &mut oneshot::Receiver<Respuesta>) -> Option<Respuesta> {
        let retirada = self.0.pendientes.lock().unwrap().remove(&id).is_some();
        if retirada {
            self.emitir(json!({"type": "oferta-retirada", "oferta": id}));
            None
        } else {
            respuesta.try_recv().ok()
        }
    }

    // ---------- Enviar ----------

    /// Valida la oferta y la manda en segundo plano; devuelve el id del envío.
    pub fn ofrecer(&self, a: &str, archivos: Vec<(PathBuf, String)>) -> Result<u64, Fallo> {
        Ok(self.ofrecer_varios(&[a.to_string()], archivos)?[0])
    }

    /// Lo mismo a varios equipos (docs/protocolo.md §7): un envío por destino, con los archivos importados una
    /// vez y, si son más de uno, repartiendo la subida entre ellos.
    pub fn ofrecer_varios(
        &self,
        a: &[String],
        archivos: Vec<(PathBuf, String)>,
    ) -> Result<Vec<u64>, Fallo> {
        if a.is_empty() || a.len() > grupo::MAX_GRUPO {
            return fallo(
                "invalid-parameters",
                format!("Se envía a entre 1 y {} equipos.", grupo::MAX_GRUPO),
            );
        }
        let mut destinos: Vec<EndpointAddr> = Vec::with_capacity(a.len());
        for a in a {
            let destino = if a.trim().starts_with("beam1") {
                billete::leer(a)
                    .ok_or(Fallo("billete-invalido", "El ticket no es válido.".into()))?
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
            if destinos.iter().any(|d| d.id == destino.id) {
                return fallo("invalid-parameters", "Hay un equipo repetido.");
            }
            destinos.push(destino);
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
        let mut envios = Vec::with_capacity(destinos.len());
        for _ in &destinos {
            let id = self.nuevo_id();
            let eventos = self.reservar_eventos(id).ok_or(Fallo(
                "busy",
                "La app debe recoger los eventos pendientes.".into(),
            ))?;
            envios.push((id, eventos, self.0.cerrar.child_token()));
        }
        for d in &destinos {
            self.0.memoria.add_endpoint_info(d.clone());
        }
        let grupo: Option<Vec<String>> =
            (destinos.len() > 1).then(|| destinos.iter().map(billete::crear).collect());
        let ids: Vec<u64> = envios.iter().map(|(id, _, _)| *id).collect();
        {
            let mut activos = self.0.activos.lock().unwrap();
            for (id, _, t) in &envios {
                activos.insert(*id, t.clone());
            }
        }
        // La importación se para si se cancelan todos los envíos (o se cierra).
        let todos = self.0.cerrar.child_token();
        {
            let tokens: Vec<CancellationToken> = envios.iter().map(|(_, _, t)| t.clone()).collect();
            let todos = todos.clone();
            tokio::spawn(async move {
                for t in tokens {
                    t.cancelled().await;
                }
                todos.cancel();
            });
        }
        // Un grupo ocupa un solo turno de envío: si no, de 5 a 8 receptores no se repartirían nada.
        let turno_grupo = (destinos.len() > 1).then(|| Arc::new(tokio::sync::OnceCell::new()));
        let e = self.clone();
        tokio::spawn(async move {
            let importado = e.importar(&archivos, &todos).await;
            let tareas =
                envios
                    .into_iter()
                    .zip(destinos)
                    .map(|((id, eventos, cancelar), destino)| {
                        let e = e.clone();
                        let importado = importado.as_ref();
                        let grupo = grupo.as_deref();
                        let turno_grupo = turno_grupo.clone();
                        let archivos = &archivos;
                        async move {
                            let _eventos = eventos;
                            let r = match importado {
                                Ok((lista, _, firmas)) => {
                                    e.enviar(
                                        id,
                                        destino,
                                        archivos,
                                        lista,
                                        firmas,
                                        grupo,
                                        turno_grupo.as_deref(),
                                        &cancelar,
                                    )
                                    .await
                                }
                                Err(error) => Err(*error),
                            };
                            e.0.activos.lock().unwrap().remove(&id);
                            e.0.servidos.lock().unwrap().remove(&id);
                            e.revocar(id);
                            e.emitir(match r {
                                Ok(()) => json!({"type": "fin", "id": id, "ok": true}),
                                Err(error) => {
                                    json!({"type": "fin", "id": id, "ok": false, "error": error})
                                }
                            });
                        }
                    });
            n0_future::join_all(tareas).await;
            drop(importado);
        });
        Ok(ids)
    }

    /// Mete los archivos en el almacén sin copiarlos; las etiquetas los protegen de la limpieza mientras duren.
    #[allow(clippy::type_complexity)]
    async fn importar(
        &self,
        rutas: &[(PathBuf, String)],
        cancelar: &CancellationToken,
    ) -> Result<(Vec<Archivo>, Vec<TempTag>, Vec<Firma>), &'static str> {
        let mut etiquetas: Vec<TempTag> = Vec::new();
        let mut archivos = Vec::new();
        let mut firmas = Vec::new();
        for (ruta, nombre) in rutas {
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
        Ok((archivos, etiquetas, firmas))
    }

    #[allow(clippy::too_many_arguments)]
    async fn enviar(
        &self,
        id: u64,
        destino: EndpointAddr,
        rutas: &[(PathBuf, String)],
        archivos: &[Archivo],
        firmas: &[Firma],
        grupo: Option<&[String]>,
        turno_grupo: Option<&TurnoGrupo>,
        cancelar: &CancellationToken,
    ) -> Result<(), &'static str> {
        if cancelar.is_cancelled() {
            return Err("cancelado");
        }
        let ep = self.0.router.endpoint();
        let conn = tokio::select! {
            c = tokio::time::timeout(ESPERA_CONEXION, ep.connect(destino.clone(), ALPN)) => match c {
                Ok(Ok(c)) => c,
                _ => return Err("sin-conexion"),
            },
            _ = cancelar.cancelled() => return Err("cancelado"),
        };
        let r = self
            .ofrecer_por(&conn, id, archivos, grupo, turno_grupo, cancelar)
            .await;
        // Si se cierra la app, el otro ve que este equipo se fue (`red`), no una cancelación.
        let codigo = match r {
            _ if self.0.cerrar.is_cancelled() => cierre::CERRADO,
            Err("cancelado") => cierre::CANCELADO,
            _ => cierre::BIEN,
        };
        conn.close(VarInt::from_u32(codigo), b"");
        // Si el otro no pudo bajar algo y el archivo cambió desde que se leyó, la causa es esa.
        if r.is_err() && r != Err("cancelado") {
            let cambio = rutas.iter().zip(firmas).any(|((ruta, _), (largo, fecha))| {
                std::fs::metadata(ruta)
                    .map(|m| m.len() != *largo || m.modified().ok() != *fecha)
                    .unwrap_or(true)
            });
            if cambio {
                return Err("huella");
            }
        }
        r
    }

    async fn ofrecer_por(
        &self,
        conn: &Connection,
        id: u64,
        archivos: &[Archivo],
        grupo: Option<&[String]>,
        turno_grupo: Option<&TurnoGrupo>,
        cancelar: &CancellationToken,
    ) -> Result<(), &'static str> {
        let (mut tx, rx) = conn.open_bi().await.map_err(|_| "sin-conexion")?;
        let lista: Vec<Value> = archivos
            .iter()
            .map(|a| json!({"nombre": a.nombre, "tamano": a.tamano, "hash": a.hash.to_hex()}))
            .collect();
        let mut oferta =
            json!({"type": "oferta", "version": 1, "alias": self.0.alias, "archivos": lista});
        if let Some(g) = grupo {
            oferta["grupo"] = json!(g);
        }
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
        let _turno = match turno_grupo {
            // El primero del grupo que acepta toma el turno para todos; se suelta al acabar el grupo.
            Some(t) => {
                tokio::select! {
                    r = t.get_or_try_init(|| self.0.turnos_envio.clone().acquire_owned()) => { r.map_err(|_| "cancelado")?; }
                    _ = cancelar.cancelled() => return Err("cancelado"),
                }
                None
            }
            None => Some(tokio::select! {
                t = self.0.turnos_envio.clone().acquire_owned() => t.map_err(|_| "cancelado")?,
                _ = cancelar.cancelled() => return Err("cancelado"),
            }),
        };
        let total: u64 = aceptados.iter().map(|&i| archivos[i].tamano).sum();
        self.autorizar(
            conn.remote_id(),
            id,
            aceptados.iter().map(|&i| archivos[i].hash),
        );
        // Con turno y permisos listos, el otro ya puede pedir los datos.
        escribir(&mut tx, &json!({"type": "listo"}))
            .await
            .map_err(|_| "red")?;
        let mut reloj = tokio::time::interval(AVANCE);
        let mut ultimo = u64::MAX;
        let mut quieto = Instant::now();
        // En un grupo, el receptor saca casi todo de los demás: su propio aviso de avance es lo que cuenta aquí.
        let mut de_grupo = 0u64;
        loop {
            tokio::select! {
                m = lector.recv() => {
                    let fin = m.ok_or("red")?;
                    if grupo.is_some() && fin["type"] == "avance" {
                        de_grupo = de_grupo.max(fin["bytes"].as_u64().unwrap_or(0).min(total));
                        continue;
                    }
                    if fin["type"] != "fin" {
                        return Err("red");
                    }
                    self.emitir(json!({"type": "avance", "id": id, "bytes": total, "total": total}));
                    return if fin["ok"] == true { Ok(()) } else {
                        Err(match fin["error"].as_str() {
                            Some("cancelado") => "cancelado",
                            Some("disco") => "disco",
                            Some("huella") => "huella",
                            Some("sin-directa") => "sin-directa",
                            _ => "red",
                        })
                    };
                }
                _ = reloj.tick() => {
                    let hecho = if grupo.is_some() {
                        de_grupo
                    } else {
                        self.0.servidos.lock().unwrap().get(&id).map(|m| m.values().sum()).unwrap_or(0)
                    };
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
                    let par = conexiones.get(&m.connection_id).copied();
                    let envio = conexiones
                        .get(&m.connection_id)
                        .filter(|_| m.request.ranges.is_blob())
                        .and_then(|id| {
                            self.0
                                .permisos
                                .lock()
                                .unwrap()
                                .get(id)
                                .and_then(|h| h.get(&hash).cloned())
                        })
                        .filter(|ids| {
                            // Un miembro de un grupo solo puede pedir el trozo de este equipo, no el blob entero.
                            let pedido = m
                                .request
                                .ranges
                                .as_single()
                                .map(|(_, r)| r.clone())
                                .unwrap_or_default();
                            let trozos = self.0.solo_trozo.lock().unwrap();
                            ids.iter().any(|id| {
                                match par.and_then(|p| trozos.get(&(*id, p, hash))) {
                                    None => true,
                                    Some(t) => (pedido.clone() & t.clone()) == pedido,
                                }
                            })
                        });
                    m.tx.send(envio.as_ref().map(|_| ()).ok_or(AbortReason::Permission))
                        .await
                        .ok();
                    if envio.is_some() {
                        let de = par.expect("petición autorizada");
                        let e = self.clone();
                        tokio::spawn(async move {
                            while let Ok(Some(u)) = m.rx.recv().await {
                                match u {
                                    RequestUpdate::Progress(p) => {
                                        e.progreso_servido(de, hash, p.end_offset)
                                    }
                                    RequestUpdate::Completed(c) => {
                                        e.0.subidos.fetch_add(
                                            c.stats.payload_bytes_sent,
                                            Ordering::Relaxed,
                                        );
                                    }
                                    RequestUpdate::Aborted(c) => {
                                        e.0.subidos.fetch_add(
                                            c.stats.payload_bytes_sent,
                                            Ordering::Relaxed,
                                        );
                                    }
                                    RequestUpdate::Started(_) => {}
                                }
                            }
                        });
                    }
                }
                // Los del grupo preguntan qué trozos hay; solo se contesta a quien puede pedir esa huella.
                ProviderMessage::ObserveRequestReceived(m) => {
                    let ok = conexiones.get(&m.inner.connection_id).is_some_and(|id| {
                        self.0
                            .permisos
                            .lock()
                            .unwrap()
                            .get(id)
                            .is_some_and(|h| h.contains_key(&m.inner.request.hash))
                    });
                    m.tx.send(if ok {
                        Ok(())
                    } else {
                        Err(AbortReason::Permission)
                    })
                    .await
                    .ok();
                }
                ProviderMessage::Throttle(m) => {
                    if let Some(l) = &self.0.limite {
                        let t = l.turno(m.inner.size);
                        let cerrar = self.0.cerrar.clone();
                        tokio::spawn(async move {
                            tokio::select! {
                                _ = tokio::time::sleep_until(t) => { m.tx.send(Ok(())).await.ok(); }
                                _ = cerrar.cancelled() => {}
                            }
                        });
                    } else {
                        m.tx.send(Ok(())).await.ok();
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
            nombres::preparar(&destino, &p.archivos[indices[0]].0)
                .map_err(|e| Fallo("disco", format!("No se pudo crear la carpeta: {e}")))?;
            // Exportar con Copy deja una segunda copia en el almacén hasta la próxima limpieza.
            // shortcut: «mismo volumen» es la misma raíz de ruta; en Linux siempre lo es y pide el doble aunque sean discos distintos.
            let mismo = destino.components().next() == self.0.almacen.components().next();
            let mut comprometido = self.0.comprometido.lock().unwrap();
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
                let libre = libre.saturating_sub(*comprometido);
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
            let bytes = total
                .checked_mul(2)
                .ok_or(Fallo("espacio", "Tamaño fuera de rango.".into()))?;
            *comprometido = comprometido.checked_add(bytes).ok_or(Fallo(
                "espacio",
                "Espacio comprometido fuera de rango.".into(),
            ))?;
            Respuesta::Aceptar(
                indices,
                destino,
                Recepcion {
                    equipo: self.clone(),
                    id: oferta,
                    terminada: false,
                    espacio: Some(Espacio {
                        bytes,
                        total: self.0.comprometido.clone(),
                    }),
                },
            )
        };
        let p = pend.remove(&oferta).expect("comprobada arriba");
        if matches!(respuesta, Respuesta::Aceptar(..)) {
            self.0
                .activos
                .lock()
                .unwrap()
                .insert(oferta, p.cancelar.clone());
        }
        if let Err(mut respuesta) = p.responder.send(respuesta) {
            if let Respuesta::Aceptar(_, _, r) = &mut respuesta {
                r.terminada = true;
            }
            self.0.activos.lock().unwrap().remove(&oferta);
            return fallo("desconocido", "La oferta ya se retiró.");
        }
        Ok(())
    }

    pub fn cancelar(&self, id: u64) -> Result<(), Fallo> {
        let mut pendientes = self.0.pendientes.lock().unwrap();
        if let Some(t) = self.0.activos.lock().unwrap().get(&id) {
            t.cancel();
            return Ok(());
        }
        if let Some(p) = pendientes.remove(&id) {
            let _ = p.responder.send(Respuesta::Rechazar);
            return Ok(());
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
        let admision = self.admitir(de);
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
        let id = self.nuevo_id();
        let eventos = self.reservar_eventos(id);
        let (Some(admision), Some(_eventos)) = (admision, eventos) else {
            let _ = escribir(&mut tx, &json!({"type": "ocupado"})).await;
            let _ = tokio::time::timeout(Duration::from_secs(5), conn.closed()).await;
            return;
        };
        let leido = tokio::time::timeout(
            ESPERA_CONEXION,
            marco::leer(&mut rx, marco::MAX_PAR, marco::MAX_PROFUNDIDAD_PAR),
        )
        .await;
        let Some(marco) = leido.ok().and_then(Result::ok) else {
            conn.close(VarInt::from_u32(cierre::MARCO), b"");
            return;
        };
        let (Some((alias, archivos)), Ok(grupo)) =
            (leer_oferta(&marco), Grupo::leer(&marco, self.clave(), de))
        else {
            conn.close(VarInt::from_u32(cierre::MARCO), b"");
            return;
        };
        let (grupo, addrs_grupo) = match grupo {
            Some((g, a)) => (Some(g), a),
            None => (None, Vec::new()),
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
        let (responder, mut respuesta) = oneshot::channel();
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
        let elegida = tokio::select! {
            r = &mut respuesta => r.ok(),
            _ = tokio::time::sleep(self.0.espera_respuesta) => self.retirar(id, &mut respuesta),
            _ = tokio::io::AsyncReadExt::read(&mut rx, &mut uno) => self.retirar(id, &mut respuesta),
            _ = self.0.cerrar.cancelled() => self.retirar(id, &mut respuesta),
        };
        drop(admision);
        let Some(respuesta) = elegida else {
            let _ = escribir(&mut tx, &json!({"type": "respuesta", "aceptados": []})).await;
            conn.close(VarInt::from_u32(cierre::CERRADO), b"");
            return;
        };
        let Respuesta::Aceptar(indices, destino, mut recepcion) = respuesta else {
            let _ = escribir(&mut tx, &json!({"type": "respuesta", "aceptados": []})).await;
            let _ = tokio::time::timeout(Duration::from_secs(5), conn.closed()).await;
            return;
        };
        // Las direcciones del grupo, solo ahora y solo las que no se conocían: se quitan al dejar de compartir.
        let mut anadidas = Vec::new();
        for a in addrs_grupo {
            if a.id != self.clave() && self.0.memoria.get_endpoint_info(a.id).is_none() {
                anadidas.push(a.id);
                self.0.memoria.add_endpoint_info(a);
            }
        }
        let elegidos: Vec<&Archivo> = indices.iter().map(|&i| &archivos[i]).collect();
        let r = tokio::select! {
            r = self.recibir_aceptada(id, &conn, &mut tx, &mut rx, &indices, &elegidos, &destino, grupo.as_ref(), &cancelar) => r,
            _ = conn.closed() => Err(if conn.close_reason().is_some_and(|r| es_cancelado(&r)) { "cancelado" } else { "red" }),
        };
        if grupo.is_some() {
            // Quien cancela deja de servir al grupo en el acto.
            let rato = if r == Err("cancelado") {
                Duration::ZERO
            } else {
                grupo::COMPARTIR
            };
            self.compartir_un_rato(id, &elegidos, rato, anadidas).await;
        }
        if r == Err("cancelado") || cancelar.is_cancelled() {
            for a in &elegidos {
                self.liberar_reservas(a.hash).await;
            }
        }
        drop(recepcion.espacio.take());
        self.0.activos.lock().unwrap().remove(&id);
        let fin = match &r {
            Ok(_) => json!({"type": "fin", "ok": true}),
            Err(e) => json!({"type": "fin", "ok": false, "error": e}),
        };
        let _ = tokio::time::timeout(ESPERA_CONEXION, escribir(&mut tx, &fin)).await;
        let _ = tx.finish();
        recepcion.terminar(match r {
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
        conn: &Connection,
        tx: &mut SendStream,
        rx: &mut RecvStream,
        indices: &[usize],
        elegidos: &[&Archivo],
        destino: &Path,
        grupo: Option<&Grupo>,
        cancelar: &CancellationToken,
    ) -> Result<Vec<String>, &'static str> {
        let de = conn.remote_id();
        let _turno = tokio::select! {
            t = self.0.turnos_recepcion.acquire() => t.map_err(|_| "cancelado")?,
            _ = cancelar.cancelled() => return Err("cancelado"),
        };
        escribir(tx, &json!({"type": "respuesta", "aceptados": indices}))
            .await
            .map_err(|_| "red")?;
        let listo = tokio::select! {
            m = tokio::time::timeout(self.0.inactividad, marco::leer(rx, 4096, marco::MAX_PROFUNDIDAD_PAR)) => m.map_err(|_| "red")?.map_err(|_| "red")?,
            _ = cancelar.cancelled() => return Err("cancelado"),
        };
        if listo["type"] != "listo" {
            return Err("red");
        }
        if self.0.exigir_directa {
            self.esperar_directa(conn, cancelar).await?;
        }
        let Some(g) = grupo else {
            return self.bajar(id, de, elegidos, destino, None, cancelar).await;
        };
        // Los demás del grupo pueden pedir a este equipo su trozo de estos archivos. No lo que ya tenía entero
        // antes: si no, quien supiera una huella podría sacársela metiéndose en un grupo.
        let mut compartibles = Vec::new();
        for a in elegidos {
            let entero = self
                .0
                .store
                .remote()
                .local(a.hash)
                .await
                .map(|l| l.is_complete())
                .unwrap_or(true);
            if !entero && a.tamano >= grupo::MIN_REPARTO {
                compartibles.push(*a);
            }
        }
        for (k, m) in g.miembros.iter().enumerate() {
            if k == g.yo {
                continue;
            }
            self.autorizar(*m, id, compartibles.iter().map(|a| a.hash));
            let mut trozos = self.0.solo_trozo.lock().unwrap();
            for a in &compartibles {
                trozos.insert((id, *m, a.hash), g.trozo(g.yo, a.tamano));
            }
        }
        let bajada = self.bajar(id, de, elegidos, destino, grupo, cancelar);
        tokio::pin!(bajada);
        let mut reloj = tokio::time::interval(Duration::from_secs(1));
        loop {
            tokio::select! {
                r = &mut bajada => return r,
                _ = reloj.tick() => {
                    let b = self.0.progreso.lock().unwrap().get(&id).copied().unwrap_or(0);
                    let _ = escribir(tx, &json!({"type": "avance", "bytes": b})).await;
                }
            }
        }
    }

    /// Con el relé de n0 los archivos solo van directos: se espera a que la conexión deje de ir por el relé.
    async fn esperar_directa(
        &self,
        conn: &Connection,
        cancelar: &CancellationToken,
    ) -> Result<(), &'static str> {
        // shortcut: se mira la conexión de la oferta; la de iroh-blobs va al mismo equipo y usa la misma ruta.
        let hasta = tokio::time::Instant::now() + ESPERA_DIRECTA;
        loop {
            if conn.paths().iter().any(|p| p.is_selected() && p.is_ip()) {
                return Ok(());
            }
            tokio::select! {
                _ = tokio::time::sleep(Duration::from_millis(200)) => {}
                _ = tokio::time::sleep_until(hasta) => return Err("sin-directa"),
                _ = cancelar.cancelled() => return Err("cancelado"),
            }
        }
    }

    /// Tras terminar, un receptor de un grupo sigue sirviendo a los demás un rato, con los blobs protegidos; después
    /// revoca los permisos y olvida las direcciones del grupo que añadió.
    async fn compartir_un_rato(
        &self,
        id: u64,
        elegidos: &[&Archivo],
        rato: Duration,
        anadidas: Vec<EndpointId>,
    ) {
        let mut etiquetas = Vec::new();
        if !rato.is_zero() {
            for a in elegidos {
                if let Ok(t) = self.0.store.tags().temp_tag(a.hash).await {
                    etiquetas.push(t);
                }
            }
        }
        let e = self.clone();
        tokio::spawn(async move {
            tokio::select! {
                _ = tokio::time::sleep(rato) => {}
                _ = e.0.cerrar.cancelled() => {}
            }
            e.revocar(id);
            e.0.servidos.lock().unwrap().remove(&id);
            for k in anadidas {
                e.0.memoria.remove_endpoint_info(k);
            }
            drop(etiquetas);
        });
    }

    async fn bajar(
        &self,
        id: u64,
        de: EndpointId,
        archivos: &[&Archivo],
        destino: &Path,
        grupo: Option<&Grupo>,
        cancelar: &CancellationToken,
    ) -> Result<Vec<String>, &'static str> {
        let total: u64 = archivos.iter().map(|a| a.tamano).sum();
        let mut hecho = 0u64;
        let mut rutas = Vec::new();
        let mut ultimo = Instant::now() - AVANCE;
        for a in archivos {
            let base = nombres::preparar(destino, &a.nombre).map_err(|_| "disco")?;
            if let Some(ya) = existente(&base, a).map_err(|_| "disco")? {
                self.liberar_reservas(a.hash).await;
                hecho += a.tamano;
                rutas.push(ya.to_string_lossy().into_owned());
                continue;
            }
            // La reserva protege lo bajado de la limpieza: si se corta la red, la próxima oferta igual reanuda.
            let _proteccion = self
                .0
                .store
                .tags()
                .temp_tag(a.hash)
                .await
                .map_err(|_| "disco")?;
            let reserva = format!("rx/{}/{}", a.hash.to_hex(), ahora());
            self.0
                .store
                .tags()
                .set(&reserva, a.hash)
                .await
                .map_err(|_| "disco")?;
            let r = match grupo {
                Some(g) if a.tamano >= grupo::MIN_REPARTO => {
                    self.bajar_en_grupo(id, de, a, g, hecho, total, cancelar)
                        .await
                }
                _ => {
                    self.bajar_uno(id, de, a, hecho, total, &mut ultimo, cancelar)
                        .await
                }
            };
            if matches!(r, Err("cancelado" | "tamano")) {
                self.liberar_reservas(a.hash).await;
            }
            r.map_err(|e| if e == "tamano" { "red" } else { e })?;
            let ruta = nombres::ruta_libre(destino, &a.nombre).map_err(|_| "disco")?;
            // El nombre del temporal se reserva en exclusiva y después iroh-blobs exporta encima (su copia es
            // bastante más rápida que leer el blob a mano: «cinco a la vez» pasaba de 117 s a 171 s).
            let (parte, archivo) =
                Temporal::crear(ruta.parent().unwrap(), id).map_err(|_| "disco")?;
            drop(archivo);
            // shortcut: Copy duplica en disco lo bajado hasta la próxima limpieza del almacén.
            let exportar = self.0.store.blobs().export_with_opts(ExportOptions {
                hash: a.hash,
                mode: ExportMode::Copy,
                target: parte.0.clone(),
            });
            tokio::select! {
                r = exportar.finish() => { r.map_err(|_| "disco")?; }
                _ = cancelar.cancelled() => return Err("cancelado"),
            }
            let ruta = parte
                .publicar(destino, &a.nombre, ruta)
                .map_err(|_| "disco")?;
            self.liberar_reservas(a.hash).await;
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
        if cancelar.is_cancelled() {
            return Err("cancelado");
        }
        let descarga = self
            .0
            .store
            .downloader(self.0.router.endpoint())
            .download(a.hash, Some(de));
        let mut s = tokio::time::timeout(self.0.inactividad, descarga.stream())
            .await
            .map_err(|_| "red")?
            .map_err(|_| "red")?;
        let mut hasta = tokio::time::Instant::now() + self.0.inactividad;
        let mut recibido = 0;
        loop {
            let item = tokio::select! {
                i = s.next() => i,
                _ = tokio::time::sleep_until(hasta) => return Err("red"),
                _ = cancelar.cancelled() => return Err("cancelado"),
            };
            match item {
                None => break,
                Some(DownloadProgressItem::Progress(n)) => {
                    if n > a.tamano {
                        return Err("tamano");
                    }
                    if n > recibido {
                        recibido = n;
                        hasta = tokio::time::Instant::now() + self.0.inactividad;
                    }
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
        if local.local_bytes() > a.tamano {
            return Err("tamano");
        }
        if !local.is_complete() || local.local_bytes() != a.tamano {
            return Err("red");
        }
        Ok(())
    }

    async fn liberar_reservas(&self, hash: Hash) {
        let _ = self
            .0
            .store
            .tags()
            .delete_prefix(format!("rx/{}/", hash.to_hex()))
            .await;
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

#[derive(Default)]
struct ColaEventos {
    cola: VecDeque<Value>,
    reservas: HashMap<u64, usize>,
}

impl ColaEventos {
    fn ocupados(&self) -> usize {
        self.cola.len() + self.reservas.values().sum::<usize>()
    }

    fn reservar(&mut self, id: u64) -> bool {
        if self.ocupados() + 2 > MAX_EVENTOS {
            return false;
        }
        self.reservas.insert(id, 2);
        true
    }

    fn emitir(&mut self, ev: Value) {
        let tipo = ev["type"].as_str().unwrap_or("");
        if matches!(tipo, "fin" | "oferta" | "respuesta" | "oferta-retirada") {
            let id = ev["id"]
                .as_u64()
                .or_else(|| ev["oferta"].as_u64())
                .or_else(|| ev["envio"].as_u64())
                .unwrap();
            let reserva = self.reservas.get_mut(&id).expect("operación admitida");
            assert!(*reserva > 0);
            *reserva -= 1;
        } else if self.ocupados() >= MAX_EVENTOS {
            return;
        }
        self.cola.push_back(ev);
    }
}

struct EventosReservados(Equipo, u64);
impl Drop for EventosReservados {
    fn drop(&mut self) {
        self.0.0.eventos.lock().unwrap().reservas.remove(&self.1);
    }
}

struct Admision(Equipo, EndpointId);
impl Drop for Admision {
    fn drop(&mut self) {
        let mut a = self.0.0.admision.lock().unwrap();
        if let Some(n) = a.get_mut(&self.1) {
            *n -= 1;
            if *n == 0 {
                a.remove(&self.1);
            }
        }
    }
}

struct Espacio {
    bytes: u64,
    total: Arc<Mutex<u64>>,
}

struct Recepcion {
    equipo: Equipo,
    id: u64,
    espacio: Option<Espacio>,
    terminada: bool,
}

impl Recepcion {
    fn terminar(mut self, evento: Value) {
        self.equipo.emitir(evento);
        self.terminada = true;
    }
}

impl Drop for Recepcion {
    fn drop(&mut self) {
        self.equipo.0.activos.lock().unwrap().remove(&self.id);
        if !self.terminada {
            self.equipo
                .emitir(json!({"type": "fin", "id": self.id, "ok": false, "error": "cancelado"}));
        }
    }
}
impl Drop for Espacio {
    fn drop(&mut self) {
        *self.total.lock().unwrap() -= self.bytes;
    }
}

struct Temporal(PathBuf);
impl Temporal {
    fn crear(padre: &Path, id: u64) -> std::io::Result<(Self, std::fs::File)> {
        static SECUENCIA: AtomicU64 = AtomicU64::new(0);
        loop {
            let n = SECUENCIA.fetch_add(1, Ordering::Relaxed);
            let ruta = padre.join(format!(".beamline-{id}-{n}.beamline-parte"));
            match std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&ruta)
            {
                Ok(f) => return Ok((Self(ruta), f)),
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(e) => return Err(e),
            }
        }
    }

    fn publicar(self, destino: &Path, nombre: &str, mut ruta: PathBuf) -> std::io::Result<PathBuf> {
        loop {
            nombres::preparar(destino, nombre)?;
            nombres::comprobar_enlace(&ruta)?;
            match publicar_sin_pisar(&self.0, &ruta) {
                Ok(()) => return Ok(ruta),
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                    ruta = nombres::ruta_libre(destino, nombre)?;
                }
                Err(e) => return Err(e),
            }
        }
    }
}
impl Drop for Temporal {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

#[cfg(windows)]
fn publicar_sin_pisar(origen: &Path, destino: &Path) -> std::io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn MoveFileExW(origen: *const u16, destino: *const u16, flags: u32) -> i32;
    }
    let origen: Vec<u16> = origen.as_os_str().encode_wide().chain(Some(0)).collect();
    let destino: Vec<u16> = destino.as_os_str().encode_wide().chain(Some(0)).collect();
    // Rutas terminadas en cero y sin MOVEFILE_REPLACE_EXISTING, también en FAT/exFAT.
    if unsafe { MoveFileExW(origen.as_ptr(), destino.as_ptr(), 0) } != 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}

#[cfg(not(windows))]
fn publicar_sin_pisar(origen: &Path, destino: &Path) -> std::io::Result<()> {
    std::fs::hard_link(origen, destino)
}

fn existente(base: &Path, a: &Archivo) -> std::io::Result<Option<PathBuf>> {
    if igual(base, a) {
        return Ok(Some(base.to_path_buf()));
    }
    let tallo = base.file_stem().unwrap_or_default().to_string_lossy();
    let ext = base
        .extension()
        .map(|e| format!(".{}", e.to_string_lossy()))
        .unwrap_or_default();
    let prefijo = format!("{tallo} (");
    let sufijo = format!("){ext}");
    let mut candidatos = Vec::new();
    for entrada in std::fs::read_dir(base.parent().unwrap())? {
        let entrada = entrada?;
        let nombre = entrada.file_name();
        let nombre = nombre.to_string_lossy();
        if let Some(n) = nombre
            .strip_prefix(&prefijo)
            .and_then(|s| s.strip_suffix(&sufijo))
            .and_then(|s| s.parse::<u64>().ok())
            .filter(|n| *n >= 2)
        {
            candidatos.push((n, entrada.path()));
        }
    }
    candidatos.sort_by_key(|(n, _)| *n);
    for (_, ruta) in candidatos {
        nombres::comprobar_enlace(&ruta)?;
        if igual(&ruta, a) {
            return Ok(Some(ruta));
        }
    }
    Ok(None)
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

mod grupo;
#[cfg(test)]
mod pruebas;
