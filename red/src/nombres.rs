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
    let ruta = preparar(destino, nombre)?;
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
        comprobar_enlace(&otra)?;
        if !otra.exists() {
            return Ok(otra);
        }
    }
    unreachable!()
}

pub fn comprobar_enlace(ruta: &Path) -> std::io::Result<()> {
    match std::fs::symlink_metadata(ruta) {
        Ok(m) => {
            let enlace = m.file_type().is_symlink();
            #[cfg(windows)]
            let enlace = {
                use std::os::windows::fs::MetadataExt;
                enlace || m.file_attributes() & 0x400 != 0
            };
            if enlace {
                return Err(std::io::Error::other("La ruta contiene un enlace."));
            }
            Ok(())
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e),
    }
}

pub fn preparar(destino: &Path, nombre: &str) -> std::io::Result<PathBuf> {
    if !valido(nombre) {
        return Err(std::io::Error::other("Nombre inválido."));
    }
    comprobar_enlace(destino)?;
    std::fs::create_dir_all(destino)?;
    let base = destino.canonicalize()?;
    let mut ruta = destino.to_path_buf();
    let partes: Vec<_> = nombre.split('/').collect();
    // shortcut: un atacante local aún puede sustituir componentes entre comprobación y escritura (TOCTOU).
    for (i, parte) in partes.iter().enumerate() {
        ruta.push(parte);
        comprobar_enlace(&ruta)?;
        if i + 1 < partes.len() {
            match std::fs::create_dir(&ruta) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
                Err(e) => return Err(e),
            }
        }
    }
    if !ruta.parent().unwrap().canonicalize()?.starts_with(base) {
        return Err(std::io::Error::other("La ruta sale del destino."));
    }
    Ok(ruta)
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
        let d = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("target")
            .join(format!("beamline-nombres-{}", rand::random::<u64>()));
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
