//! Envío a varios a la vez (docs/protocolo.md §7): cada receptor baja su trozo de cada archivo del emisor y el
//! resto de los otros receptores, así el emisor sube cada byte más o menos una vez.

use super::*;
use iroh_blobs::protocol::{ChunkRanges, ChunkRangesExt, GetRequest, ObserveRequest, RangeSpec};

/// Como mucho tantos receptores en un envío a varios.
pub const MAX_GRUPO: usize = 8;
/// Por debajo de esto no compensa repartir: se baja entero del emisor.
pub const MIN_REPARTO: u64 = 4 << 20;
/// Sin novedades del trozo de otro receptor en este tiempo, se pide al emisor.
const ESPERA_TROZO: Duration = Duration::from_secs(30);
/// Si otro receptor no tiene ni un trozo en este tiempo (por ejemplo, porque ya tenía el archivo y se lo saltó),
/// se pide al emisor sin esperar más.
const ESPERA_PRIMERO: Duration = Duration::from_secs(10);
/// Lo que se espera a que la conexión con otro receptor vaya directa cuando el relé es el de n0.
const ESPERA_DIRECTA_PAR: Duration = Duration::from_secs(5);
/// Ritmo mínimo que se le aguanta a otro receptor; más lento, su trozo se pide al emisor.
const RITMO_MINIMO: u64 = 512 << 10;
/// Lo que un receptor sigue sirviendo a los demás después de terminar.
pub const COMPARTIR: Duration = Duration::from_secs(120);
/// Trozos alineados a los bloques de 16 KiB del almacén.
const BLOQUE: u64 = 16;

pub struct Grupo {
    pub miembros: Vec<EndpointId>,
    pub yo: usize,
}

impl Grupo {
    /// Lee el «grupo» de una oferta: billetes de todos los receptores, en orden, con este equipo una vez y sin el
    /// que envía. Las direcciones se dan a la memoria del endpoint solo si se acepta (`atender_oferta`).
    #[allow(clippy::type_complexity)]
    pub fn leer(
        m: &Value,
        yo: EndpointId,
        de: EndpointId,
    ) -> Result<Option<(Self, Vec<EndpointAddr>)>, ()> {
        let Some(lista) = m.get("grupo") else {
            return Ok(None);
        };
        let lista = lista
            .as_array()
            .filter(|l| (2..=MAX_GRUPO).contains(&l.len()))
            .ok_or(())?;
        let mut addrs = Vec::with_capacity(lista.len());
        for b in lista {
            addrs.push(b.as_str().and_then(billete::leer).ok_or(())?);
        }
        let miembros: Vec<EndpointId> = addrs.iter().map(|a| a.id).collect();
        let unicos: HashSet<_> = miembros.iter().collect();
        if unicos.len() != miembros.len() || unicos.contains(&de) {
            return Err(());
        }
        let yo = miembros.iter().position(|m| *m == yo).ok_or(())?;
        Ok(Some((Grupo { miembros, yo }, addrs)))
    }

    /// El trozo `k` de un blob de `tamano` bytes, en chunks de 1 KiB alineados a bloques. El último acaba justo en
    /// el último chunk del blob, para que «lo tiene entero» se cumpla aunque el tamaño no sea múltiplo de 16 KiB.
    pub fn trozo(&self, k: usize, tamano: u64) -> ChunkRanges {
        let bloques = tamano.div_ceil(1024 * BLOQUE);
        let n = self.miembros.len() as u64;
        let (a, b) = (bloques * k as u64 / n, bloques * (k as u64 + 1) / n);
        let fin = (b * BLOQUE).min(tamano.div_ceil(1024));
        ChunkRanges::chunks(a * BLOQUE..fin)
    }
}

/// Cuenta novedades (trozos que llegan, avisos de `observe`) para que la inactividad no salte mientras el grupo avanza.
#[derive(Default)]
struct Senal(AtomicU64);

impl Senal {
    fn hay(&self) {
        self.0.fetch_add(1, Ordering::Relaxed);
    }
}

impl Equipo {
    /// Baja `a` repartido con el grupo: su trozo del emisor, los demás de quien los tiene (o del emisor si no
    /// llegan) y lo que falte al final, del emisor.
    #[allow(clippy::too_many_arguments)]
    pub(super) async fn bajar_en_grupo(
        &self,
        id: u64,
        de: EndpointId,
        a: &Archivo,
        grupo: &Grupo,
        antes: u64,
        total: u64,
        cancelar: &CancellationToken,
    ) -> Result<(), &'static str> {
        let senal = Senal::default();
        let senal = &senal;
        let trabajo = async {
            let partes = (0..grupo.miembros.len()).map(|k| async move {
                let rango = grupo.trozo(k, a.tamano);
                if k == grupo.yo {
                    return self.bajar_rango(a.hash, rango, vec![de], None, senal).await;
                }
                let par = grupo.miembros[k];
                if self.esperar_trozo(par, a.hash, &rango, senal).await {
                    // Con tope: un par que sirve a goteo no frena; lo que no dio, lo da el emisor.
                    let bytes = a.tamano.div_ceil(grupo.miembros.len() as u64);
                    let tope = ESPERA_TROZO.max(Duration::from_secs(bytes / RITMO_MINIMO));
                    if self
                        .bajar_rango(a.hash, rango.clone(), vec![par], Some(tope), senal)
                        .await
                        .is_ok()
                    {
                        return Ok(());
                    }
                }
                self.bajar_rango(a.hash, rango, vec![de], None, senal).await
            });
            for r in n0_future::join_all(partes).await {
                r?;
            }
            self.bajar_rango(a.hash, ChunkRanges::all(), vec![de], None, senal)
                .await
        };
        tokio::pin!(trabajo);
        let mut reloj = tokio::time::interval(AVANCE);
        let mut recibido = 0;
        let mut novedades = 0;
        let mut quieto = tokio::time::Instant::now();
        loop {
            tokio::select! {
                r = &mut trabajo => { r?; break; }
                _ = reloj.tick() => {
                    let n = self.0.store.remote().local(a.hash).await.map_err(|_| "disco")?.local_bytes();
                    if n > a.tamano {
                        return Err("tamano");
                    }
                    let s = senal.0.load(Ordering::Relaxed);
                    if n > recibido || s != novedades {
                        if n > recibido {
                            recibido = n;
                            self.emitir(json!({"type": "avance", "id": id, "bytes": (antes + n).min(total), "total": total}));
                        }
                        novedades = s;
                        quieto = tokio::time::Instant::now();
                    } else if quieto.elapsed() > self.0.inactividad {
                        return Err("red");
                    }
                }
                _ = cancelar.cancelled() => return Err("cancelado"),
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

    async fn bajar_rango(
        &self,
        hash: Hash,
        rango: ChunkRanges,
        fuentes: Vec<EndpointId>,
        tope: Option<Duration>,
        senal: &Senal,
    ) -> Result<(), &'static str> {
        if rango.is_empty() {
            return Ok(());
        }
        let bajar = async {
            let mut s = self
                .0
                .store
                .downloader(self.0.router.endpoint())
                .download(GetRequest::blob_ranges(hash, rango), fuentes)
                .stream()
                .await
                .map_err(|_| "red")?;
            while let Some(i) = s.next().await {
                match i {
                    DownloadProgressItem::Error(_) | DownloadProgressItem::DownloadError => {
                        return Err("red");
                    }
                    DownloadProgressItem::Progress(_) => senal.hay(),
                    _ => {}
                }
            }
            Ok(())
        };
        match tope {
            Some(t) => tokio::time::timeout(t, bajar).await.map_err(|_| "red")?,
            None => bajar.await,
        }
    }

    /// Mira con `observe` si `par` ya tiene `rango`. Lo da por perdido si se niega (no aceptó), se va, no tiene nada
    /// en un rato, deja de avanzar o, con el relé de n0, no se llega a él directo.
    async fn esperar_trozo(
        &self,
        par: EndpointId,
        hash: Hash,
        rango: &ChunkRanges,
        senal: &Senal,
    ) -> bool {
        let ep = self.0.router.endpoint();
        let Ok(Ok(conn)) =
            tokio::time::timeout(ESPERA_CONEXION, ep.connect(par, iroh_blobs::ALPN)).await
        else {
            return false;
        };
        if self.0.exigir_directa {
            // shortcut: se mira esta conexión; la del descargador va al mismo par por el mismo socket.
            let hasta = tokio::time::Instant::now() + ESPERA_DIRECTA_PAR;
            while !conn.paths().iter().any(|p| p.is_selected() && p.is_ip()) {
                if tokio::time::Instant::now() >= hasta {
                    return false;
                }
                tokio::time::sleep(Duration::from_millis(200)).await;
            }
        }
        let pedido = ObserveRequest {
            hash,
            ranges: RangeSpec::new(rango.clone()),
        };
        let s = self.0.store.remote().observe(conn, pedido);
        tokio::pin!(s);
        let mut tiene = ChunkRanges::empty();
        let primero = tokio::time::Instant::now() + ESPERA_PRIMERO;
        loop {
            let espera = if tiene.is_empty() {
                primero.saturating_duration_since(tokio::time::Instant::now())
            } else {
                ESPERA_TROZO
            };
            match tokio::time::timeout(espera, s.next()).await {
                Ok(Some(Ok(b))) => {
                    tiene |= b.ranges;
                    senal.hay();
                    if (rango.clone() & tiene.clone()) == *rango {
                        return true;
                    }
                }
                _ => return false,
            }
        }
    }
}

#[cfg(test)]
mod pruebas {
    use super::*;

    fn clave() -> EndpointId {
        SecretKey::generate().public()
    }

    fn oferta(grupo: &[EndpointId]) -> Value {
        json!({"grupo": grupo.iter().map(|k| billete::crear(&EndpointAddr::new(*k))).collect::<Vec<_>>()})
    }

    #[test]
    fn los_trozos_cubren_todo_sin_solaparse() {
        for n in 2..=MAX_GRUPO {
            let g = Grupo {
                miembros: (0..n).map(|_| clave()).collect(),
                yo: 0,
            };
            for tamano in [
                1u64,
                1023,
                1024,
                16 << 10,
                MIN_REPARTO,
                MIN_REPARTO + 777,
                1 << 30,
            ] {
                let mut todo = ChunkRanges::empty();
                for k in 0..n {
                    let t = g.trozo(k, tamano);
                    assert!(
                        (t.clone() & todo.clone()).is_empty(),
                        "solapa en {n}/{tamano}"
                    );
                    todo |= t;
                }
                let fin = tamano.div_ceil(1024);
                assert_eq!(todo, ChunkRanges::chunks(0..fin), "{n}/{tamano}");
            }
        }
    }

    #[test]
    fn un_grupo_hostil_se_rechaza() {
        let (yo, de, otro) = (clave(), clave(), clave());
        assert!(Grupo::leer(&json!({}), yo, de).unwrap().is_none());
        let bien = Grupo::leer(&oferta(&[otro, yo]), yo, de).unwrap().unwrap();
        assert_eq!(bien.0.yo, 1);
        for malo in [
            oferta(&[otro]),           // uno solo no es grupo
            oferta(&[otro, clave()]),  // sin este equipo
            oferta(&[yo, de]),         // con el que envía
            oferta(&[yo, otro, otro]), // repetido
            oferta(
                &(0..=MAX_GRUPO)
                    .map(|_| clave())
                    .chain([yo])
                    .collect::<Vec<_>>(),
            ), // demasiados
            json!({"grupo": [billete::crear(&EndpointAddr::new(yo)), "beam1basura"]}),
            json!({"grupo": "no es una lista"}),
        ] {
            assert!(Grupo::leer(&malo, yo, de).is_err(), "{malo}");
        }
    }
}
