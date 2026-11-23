//! Billete: `beam1` + base64url del JSON de la `EndpointAddr` (docs/protocolo.md §6).

use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use iroh::EndpointAddr;

const PREFIJO: &str = "beam1";

pub fn crear(addr: &EndpointAddr) -> String {
    format!(
        "{PREFIJO}{}",
        URL_SAFE_NO_PAD.encode(serde_json::to_vec(addr).expect("EndpointAddr a JSON"))
    )
}

pub fn leer(texto: &str) -> Option<EndpointAddr> {
    let cuerpo = texto.trim().strip_prefix(PREFIJO)?;
    if cuerpo.len() > 8192 {
        return None;
    }
    serde_json::from_slice(&URL_SAFE_NO_PAD.decode(cuerpo).ok()?).ok()
}

#[cfg(test)]
mod pruebas {
    use super::*;

    #[test]
    fn ida_y_vuelta() {
        let clave = iroh::SecretKey::generate();
        let addr =
            EndpointAddr::new(clave.public()).with_ip_addr("192.168.1.5:4433".parse().unwrap());
        let b = crear(&addr);
        assert!(b.starts_with("beam1"));
        assert_eq!(leer(&format!(" {b}\n")), Some(addr));
    }

    #[test]
    fn rechaza_basura() {
        for b in [
            "",
            "beam1",
            "beam1@@@",
            "beam2abc",
            "blobabc",
            &format!("beam1{}", "A".repeat(9000)),
        ] {
            assert!(leer(b).is_none(), "{b}");
        }
    }
}
