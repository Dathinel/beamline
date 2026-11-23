//! Nombres de archivo que vienen del otro equipo (docs/protocolo.md §4): nunca salen de la carpeta de destino.

use std::path::{Path, PathBuf};

const RESERVADOS: &[&str] = &[
    "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8",
    "COM9", "COM¹", "COM²", "COM³", "CONIN$", "CONOUT$", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5",
    "LPT6", "LPT7", "LPT8", "LPT9", "LPT¹", "LPT²", "LPT³",
];

/// Un nombre relativo con `/` es válido en Windows, Linux y macOS y no sube de carpeta.
pub fn valido(nombre: &str) -> bool {
    (1..=1024).contains(&nombre.len()) && nombre.split('/').all(parte_valida)
}

fn parte_valida(p: &str) -> bool {
    if p.is_empty() || p.len() > 255 || p == "." || p == ".." || p.ends_with(['.', ' ']) {
        return false;
    }
    if p.chars()
        .any(|c| c.is_control() || matches!(c, '<' | '>' | ':' | '"' | '\\' | '|' | '?' | '*'))
    {
        return false;
    }
    let base = p.split('.').next().unwrap_or("").trim_end();
    !RESERVADOS.iter().any(|r| r.eq_ignore_ascii_case(base))
}

/// Ruta final dentro de `destino` sin pisar nada: `nombre`, `nombre (2)`, `nombre (3)`… Crea las carpetas intermedias.
pub fn ruta_libre(destino: &Path, nombre: &str) -> std::io::Result<PathBuf> {
    debug_assert!(valido(nombre));
    let mut ruta = destino.to_path_buf();
    ruta.extend(nombre.split('/'));
    if let Some(padre) = ruta.parent() {
        std::fs::create_dir_all(padre)?;
    }
    if !ruta.exists() {
        return Ok(ruta);
    }
    let tallo = ruta
        .file_stem()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned();
    let ext = ruta
        .extension()
        .map(|e| format!(".{}", e.to_string_lossy()))
        .unwrap_or_default();
    for n in 2.. {
        let otra = ruta.with_file_name(format!("{tallo} ({n}){ext}"));
        if !otra.exists() {
            return Ok(otra);
        }
    }
    unreachable!()
}

#[cfg(test)]
mod pruebas {
    use super::*;

    #[test]
    fn validos() {
        for n in [
            "a.txt",
            "fotos/2026/a b.jpg",
            "ñandú.mp4",
            ".oculto",
            "a..b",
        ] {
            assert!(valido(n), "{n}");
        }
    }

    #[test]
    fn invalidos() {
        let largo = "x".repeat(256);
        for n in [
            "",
            "/a",
            "a/",
            "a//b",
            "..",
            "../a",
            "a/../b",
            "./a",
            "C:/a",
            "c:",
            "a\\b",
            "a:b",
            "a\u{0}b",
            "a\nb",
            "CON",
            "con.txt",
            "Lpt1.log",
            "nul ",
            "a.",
            "a ",
            "a?",
            "a*",
            "a|b",
            "a<b",
            "CONIN$",
            "conout$.txt",
            "COM¹",
            &largo,
        ] {
            assert!(!valido(n), "{n:?}");
        }
        assert!(!valido(&"a/".repeat(600)));
    }

    #[test]
    fn no_pisa() {
        let d = std::env::temp_dir().join(format!("beamline-nombres-{}", rand::random::<u64>()));
        std::fs::create_dir_all(&d).unwrap();
        let r1 = ruta_libre(&d, "sub/a.txt").unwrap();
        assert_eq!(r1, d.join("sub").join("a.txt"));
        std::fs::write(&r1, b"1").unwrap();
        let r2 = ruta_libre(&d, "sub/a.txt").unwrap();
        assert_eq!(r2, d.join("sub").join("a (2).txt"));
        std::fs::write(&r2, b"2").unwrap();
        assert_eq!(
            ruta_libre(&d, "sub/a.txt").unwrap(),
            d.join("sub").join("a (3).txt")
        );
        std::fs::remove_dir_all(&d).unwrap();
    }
}
