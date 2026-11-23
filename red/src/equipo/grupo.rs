//! Envío a varios a la vez (docs/protocolo.md §7): cada receptor baja su trozo de cada archivo del emisor y el
//! resto de los otros receptores, así el emisor sube cada byte más o menos una vez.

use super::*;
use iroh_blobs::protocol::{ChunkRanges, ChunkRangesExt, GetRequest, ObserveRequest, RangeSpec};

/// Como mucho tantos receptores en un envío a varios.
pub const MAX_GRUPO: usize = 8;
/// Por debajo de esto no compensa repartir: se baja entero del emisor.
pub const MIN_REPARTO: u64 = 4 << 20;
/// Lo que espera un receptor sin novedades del trozo de otro antes de pedírselo al emisor.
const ESPERA_TROZO: Duration = Duration::from_secs(30);
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
    /// que envía. Las direcciones van a la memoria del endpoint para que el descargador las encuentre.
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

    /// El trozo `k` de un blob de `tamano` bytes, en chunks de 1 KiB alineados a bloques.
    pub fn trozo(&self, k: usize, tamano: u64) -> ChunkRanges {
        let bloques = tamano.div_ceil(1024 * BLOQUE);
        let n = self.miembros.len() as u64;
        let (a, b) = (bloques * k as u64 / n, bloques * (k as u64 + 1) / n);
        ChunkRanges::chunks(a * BLOQUE..b * BLOQUE)
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
        let trabajo = async {
            let partes = (0..grupo.miembros.len()).map(|k| async move {
                let rango = grupo.trozo(k, a.tamano);
                let fuentes = if k == grupo.yo {
                    vec![de]
                } else if self.esperar_trozo(grupo.miembros[k], a.hash, &rango).await {
                    vec![grupo.miembros[k], de]
                } else {
                    vec![de]
                };
                self.bajar_rango(a.hash, rango, fuentes).await
            });
            for r in n0_future::join_all(partes).await {
                r?;
            }
            self.bajar_rango(a.hash, ChunkRanges::all(), vec![de]).await
        };
        tokio::pin!(trabajo);
        let mut reloj = tokio::time::interval(AVANCE);
        let mut recibido = 0;
        let mut quieto = tokio::time::Instant::now();
        loop {
            tokio::select! {
                r = &mut trabajo => { r?; break; }
                _ = reloj.tick() => {
                    let n = self.0.store.remote().local(a.hash).await.map_err(|_| "disco")?.local_bytes();
                    if n > a.tamano {
                        return Err("tamano");
                    }
                    if n > recibido {
                        recibido = n;
                        quieto = tokio::time::Instant::now();
                        self.emitir(json!({"type": "avance", "id": id, "bytes": (antes + n).min(total), "total": total}));
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
    ) -> Result<(), &'static str> {
        if rango.is_empty() {
            return Ok(());
        }
        let mut s = self
            .0
            .store
            .downloader(self.0.router.endpoint())
            .download(GetRequest::blob_ranges(hash, rango), fuentes)
            .stream()
            .await
            .map_err(|_| "red")?;
        while let Some(i) = s.next().await {
            if matches!(
                i,
                DownloadProgressItem::Error(_) | DownloadProgressItem::DownloadError
            ) {
                return Err("red");
            }
        }
        Ok(())
    }

    /// Mira con `observe` si `par` ya tiene `rango`; da por perdido al par si se niega, se va o no avanza.
    async fn esperar_trozo(&self, par: EndpointId, hash: Hash, rango: &ChunkRanges) -> bool {
        let ep = self.0.router.endpoint();
        let Ok(Ok(conn)) =
            tokio::time::timeout(ESPERA_CONEXION, ep.connect(par, iroh_blobs::ALPN)).await
        else {
            return false;
        };
        let pedido = ObserveRequest {
            hash,
            ranges: RangeSpec::new(rango.clone()),
        };
        let s = self.0.store.remote().observe(conn, pedido);
        tokio::pin!(s);
        let mut tiene = ChunkRanges::empty();
        loop {
            match tokio::time::timeout(ESPERA_TROZO, s.next()).await {
                Ok(Some(Ok(b))) => {
                    tiene |= b.ranges;
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
                let fin = tamano.div_ceil(1024 * BLOQUE) * BLOQUE;
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
