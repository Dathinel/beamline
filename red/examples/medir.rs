//! Medir el núcleo real entre equipos (docs/medir-entre-pc.md). Uno o varios reciben y otro envía:
//!
//!   medir recibir <carpeta> <billete.txt> [--n N] [--rele URL] [--solo-rele] [--n0]
//!   medir enviar <archivo> <billete.txt>... [--rele URL] [--solo-rele] [--n0]
//!
//! El que recibe escribe su billete en el archivo, acepta todo lo que le llegue y sale tras N recepciones.
//! El que envía espera a que existan los billetes, ofrece a todos a la vez y da tiempo, MiB/s y lo que subió.

use std::{
    path::PathBuf,
    time::{Duration, Instant},
};

use beamline_red::equipo::{Config, Equipo, Rele};
use tokio::sync::Notify;

fn opcion(args: &[String], nombre: &str) -> Option<String> {
    args.iter().position(|a| a == nombre).and_then(|i| args.get(i + 1).cloned())
}

fn config(carpeta: &std::path::Path, alias: &str, args: &[String]) -> Config {
    let rele = match opcion(args, "--rele") {
        Some(u) => Rele::Url(u.parse().expect("URL de relé")),
        None if args.iter().any(|a| a == "--n0") => Rele::N0,
        None => Rele::Ninguno,
    };
    Config {
        identidad: iroh::SecretKey::generate(),
        alias: alias.into(),
        almacen: carpeta.join(format!("almacen-{alias}")),
        rele,
        solo_rele: args.iter().any(|a| a == "--solo-rele"),
        mdns: false,
        tamano_max: u64::MAX,
        inseguro: false,
        limpieza: Duration::from_secs(300),
        espera_respuesta: Duration::from_secs(600),
        limite_subida: None,
    }
}

#[tokio::main]
async fn main() {
    let args: Vec<String> = std::env::args().collect();
    let nada = Notify::new();
    match args.get(1).map(String::as_str) {
        Some("recibir") => {
            let carpeta = PathBuf::from(&args[2]);
            let n: usize = opcion(&args, "--n").map(|n| n.parse().unwrap()).unwrap_or(1);
            let e = Equipo::abrir(config(&carpeta, "recibe", &args)).await.unwrap();
            std::fs::write(&args[3], e.billete()).unwrap();
            let mut inicio = Instant::now();
            let mut hechas = 0;
            while hechas < n {
                for ev in e.recibir(Duration::from_secs(1), &nada).await {
                    match ev["type"].as_str() {
                        Some("oferta") => {
                            let todos: Vec<usize> = (0..ev["archivos"].as_array().unwrap().len()).collect();
                            inicio = Instant::now();
                            e.responder(ev["oferta"].as_u64().unwrap(), todos, Some(carpeta.join("recibido"))).unwrap();
                        }
                        Some("fin") => {
                            hechas += 1;
                            let s = inicio.elapsed().as_secs_f64();
                            println!("RECIBIDO ok={} error={} en {s:.1} s; sirvió a otros {} bytes", ev["ok"], ev["error"], e.estado()["subidos"]);
                        }
                        _ => {}
                    }
                }
            }
            // Un rato para servir a los demás del grupo antes de cerrar.
            tokio::time::sleep(Duration::from_secs(5)).await;
            println!("RECEPTOR subidos={}", e.estado()["subidos"]);
            e.cerrar().await;
        }
        Some("enviar") => {
            let archivo = PathBuf::from(&args[2]);
            let billetes: Vec<&String> = args[3..].iter().take_while(|a| !a.starts_with("--")).collect();
            let mut textos = Vec::new();
            for b in &billetes {
                loop {
                    if let Ok(t) = std::fs::read_to_string(b) && t.starts_with("beam1") {
                        textos.push(t.trim().to_string());
                        break;
                    }
                    tokio::time::sleep(Duration::from_millis(200)).await;
                }
            }
            let carpeta = archivo.parent().unwrap().to_path_buf();
            let e = Equipo::abrir(config(&carpeta, "envia", &args)).await.unwrap();
            let tamano = std::fs::metadata(&archivo).unwrap().len();
            let inicio = Instant::now();
            let ids = e.ofrecer_varios(&textos, vec![(archivo.clone(), "grande.bin".into())]).unwrap();
            let mut faltan = ids.len();
            while faltan > 0 {
                for ev in e.recibir(Duration::from_secs(1), &nada).await {
                    if ev["type"] == "fin" {
                        faltan -= 1;
                        println!("ENVIO id={} ok={} error={} a los {:.1} s", ev["id"], ev["ok"], ev["error"], inicio.elapsed().as_secs_f64());
                    }
                }
            }
            let s = inicio.elapsed().as_secs_f64();
            let subidos = e.estado()["subidos"].as_u64().unwrap();
            println!(
                "RESULTADO destinos={} tamano={} MiB tiempo={s:.1} s ritmo_por_destino={:.1} MiB/s subidos={:.2}x",
                ids.len(),
                tamano >> 20,
                (tamano as f64 / (1 << 20) as f64) / s,
                subidos as f64 / tamano as f64
            );
            e.cerrar().await;
        }
        _ => eprintln!("uso: medir recibir <carpeta> <billete.txt> [--n N] | medir enviar <archivo> <billete.txt>..."),
    }
}
