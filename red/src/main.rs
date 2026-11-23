//! beamline-red: la red de BeamLine. Lo abre la app como un módulo (`--rodaje-pipe <nombre o ruta>`), docs/protocolo.md §1.

use std::{
    path::PathBuf,
    sync::{Arc, Mutex},
    time::Duration,
};

use beamline_red::equipo::{Config, Equipo, Rele};
use dathinel_red::{
    hex,
    modulo::{self, Fallo, Info, Servicio},
};
use serde_json::{Value, json};
use tokio::sync::Notify;

static INFO: Info = Info {
    id: "beamline-red",
    nombre: "Red de BeamLine",
    version: env!("CARGO_PKG_VERSION"),
    funciones: &[
        ("abrir", "Abrir este equipo en la red", false),
        ("ofrecer", "Ofrecer archivos a otro equipo", false),
        ("responder", "Aceptar o rechazar una oferta", false),
        (
            "cancelar",
            "Cancelar un envío, una recepción o una oferta",
            false,
        ),
        ("recibir", "Esperar lo que llega de la red", true),
        ("estado", "Estado de este equipo", false),
        ("cerrar", "Cerrar todo", false),
    ],
};

#[derive(Default)]
struct Red(Mutex<Option<Equipo>>);

fn invalido<T>(msg: impl Into<String>) -> Result<T, Fallo> {
    Err(Fallo("invalid-parameters", msg.into()))
}

fn texto<'a>(p: &'a Value, campo: &str) -> Result<&'a str, Fallo> {
    p[campo]
        .as_str()
        .ok_or_else(|| Fallo("invalid-parameters", format!("Falta «{campo}».")))
}

impl Red {
    fn actual(&self) -> Result<Equipo, Fallo> {
        self.0.lock().unwrap().clone().ok_or(Fallo(
            "no-abierto",
            "Este equipo aún no está abierto.".into(),
        ))
    }

    async fn abrir(&self, p: Value) -> Result<Value, Fallo> {
        if self.0.lock().unwrap().is_some() {
            return Err(Fallo("ya-abierto", "Este equipo ya está abierto.".into()));
        }
        let identidad = texto(&p, "identidad")?;
        let identidad = hex::secreta(identidad).ok_or(Fallo(
            "invalid-parameters",
            "«identidad» tiene que ser 64 hex.".into(),
        ))?;
        let red = &p["red"];
        let rele = match red["rele"].as_str().unwrap_or("n0") {
            "n0" => Rele::N0,
            "ninguno" => Rele::Ninguno,
            u if u.starts_with("https://")
                || (cfg!(feature = "prueba") && u.starts_with("http://")) =>
            {
                Rele::Url(
                    u.parse()
                        .map_err(|_| Fallo("invalid-parameters", "URL de relé inválida.".into()))?,
                )
            }
            _ => return invalido("«rele» tiene que ser n0, ninguno o una URL https."),
        };
        let e = Equipo::abrir(Config {
            identidad,
            alias: texto(&p, "alias")?.to_string(),
            almacen: PathBuf::from(texto(&p, "almacen")?),
            rele,
            solo_rele: red["soloRele"].as_bool().unwrap_or(false),
            mdns: red["mdns"].as_bool().unwrap_or(false),
            tamano_max: p["tamanoMax"].as_u64().unwrap_or(1 << 40),
            inseguro: red["releInseguro"].as_bool().unwrap_or(false),
            limpieza: Duration::from_secs(p["limpiezaS"].as_u64().unwrap_or(300).clamp(1, 86_400)),
            espera_respuesta: Duration::from_secs(
                p["esperaRespuestaS"]
                    .as_u64()
                    .unwrap_or(600)
                    .clamp(1, 86_400),
            ),
        })
        .await?;
        let r = json!({"clave": e.clave().to_string(), "billete": e.billete()});
        let sobra = {
            let mut ranura = self.0.lock().unwrap();
            if ranura.is_some() {
                Some(e)
            } else {
                *ranura = Some(e);
                None
            }
        };
        if let Some(e) = sobra {
            e.cerrar().await;
            return Err(Fallo("ya-abierto", "Este equipo ya está abierto.".into()));
        }
        Ok(r)
    }
}

impl Servicio for Red {
    async fn llamar(
        &self,
        funcion: String,
        p: Value,
        cancelar: Arc<Notify>,
    ) -> Result<Value, Fallo> {
        match funcion.as_str() {
            "abrir" => self.abrir(p).await,
            "estado" => Ok(self.actual()?.estado()),
            "ofrecer" => {
                let e = self.actual()?;
                let Some(lista) = p["archivos"].as_array() else {
                    return invalido("Falta «archivos».");
                };
                let mut archivos = Vec::with_capacity(lista.len());
                for a in lista {
                    archivos.push((
                        PathBuf::from(texto(a, "ruta")?),
                        texto(a, "nombre")?.to_string(),
                    ));
                }
                Ok(json!({"envio": e.ofrecer(texto(&p, "a")?, archivos)?}))
            }
            "responder" => {
                let e = self.actual()?;
                let oferta = p["oferta"]
                    .as_u64()
                    .ok_or(Fallo("invalid-parameters", "Falta «oferta».".into()))?;
                let aceptar: Vec<usize> = match &p["aceptar"] {
                    Value::Null => Vec::new(),
                    Value::Array(l) => l
                        .iter()
                        .map(|i| i.as_u64().map(|i| i as usize))
                        .collect::<Option<_>>()
                        .ok_or(Fallo(
                            "invalid-parameters",
                            "«aceptar» es una lista de índices.".into(),
                        ))?,
                    _ => return invalido("«aceptar» es una lista de índices."),
                };
                e.responder(oferta, aceptar, p["destino"].as_str().map(PathBuf::from))?;
                Ok(Value::Null)
            }
            "cancelar" => {
                let id = p["id"]
                    .as_u64()
                    .ok_or(Fallo("invalid-parameters", "Falta «id».".into()))?;
                self.actual()?.cancelar(id)?;
                Ok(Value::Null)
            }
            "recibir" => {
                let e = self.actual()?;
                let espera =
                    Duration::from_millis(p["esperaMs"].as_u64().unwrap_or(30_000).min(30_000));
                Ok(json!({"eventos": e.recibir(espera, &cancelar).await}))
            }
            "cerrar" => {
                let e = self.0.lock().unwrap().take();
                if let Some(e) = e {
                    e.cerrar().await;
                }
                Ok(Value::Null)
            }
            _ => Err(Fallo(
                "unknown-function",
                format!("No existe la función «{funcion}»."),
            )),
        }
    }

    async fn cerrar(&self) {
        let e = self.0.lock().unwrap().take();
        if let Some(e) = e {
            e.cerrar().await;
        }
    }
}

#[tokio::main]
async fn main() {
    let args: Vec<String> = std::env::args().collect();
    modulo::vigilar_padre();
    std::process::exit(modulo::ejecutar(&args, &INFO, Arc::new(Red::default())).await);
}
