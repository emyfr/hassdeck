//! Libreria delle icone: tutte le immagini nella cartella `icons/`. Ogni
//! upload aggiunge un file alla libreria, da cui lo stesso file si puo'
//! assegnare a piu' tasti senza ricaricarlo.

use serde::Serialize;
use std::path::Path;

use crate::config::Config;

/// Estensioni accettate: i formati che il crate `image` decodifica qui.
const EXTENSIONS: &[&str] = &["png", "jpg", "jpeg"];
const MAX_STEM_LEN: usize = 40;

#[derive(Debug, Serialize)]
pub struct LibraryIcon {
    /// Nome del file nella cartella delle icone.
    pub file: String,
    /// Percorso da salvare in `config.toml` e da cui il browser la scarica.
    pub path: String,
    /// Tasti che la usano.
    pub used_by: Vec<u8>,
}

/// Percorso salvato in `config.toml` per un file della libreria.
pub fn config_path(file: &str) -> String {
    format!("icons/{file}")
}

fn extension(file: &str) -> Option<String> {
    let (_, ext) = file.rsplit_once('.')?;
    let ext = ext.to_lowercase();
    EXTENSIONS.contains(&ext.as_str()).then_some(ext)
}

/// `true` se `file` e' un nome di file valido della libreria: niente
/// percorsi, file nascosti o estensioni non supportate.
pub fn is_library_file(file: &str) -> bool {
    !file.is_empty()
        && !file.starts_with('.')
        && !file.contains(['/', '\\'])
        && extension(file).is_some()
}

/// Nome leggibile e sicuro ricavato dal nome del file caricato: minuscole,
/// cifre, `-` e `_`, con il resto sostituito da `-`.
fn sanitize_stem(original: &str) -> String {
    let stem = original.rsplit(['/', '\\']).next().unwrap_or_default();
    let stem = stem.rsplit_once('.').map_or(stem, |(s, _)| s);
    let mut out = String::new();
    for c in stem.to_lowercase().chars() {
        if c.is_ascii_alphanumeric() || c == '_' {
            out.push(c);
        } else if !out.ends_with('-') {
            out.push('-');
        }
    }
    let out: String = out.trim_matches('-').chars().take(MAX_STEM_LEN).collect();
    if out.is_empty() { "icona".to_string() } else { out }
}

/// Salva nella libreria l'immagine caricata con nome `original` e restituisce
/// il nome del file. Se esiste gia' un file identico riusa quello; se il nome
/// e' gia' preso da un'altra immagine aggiunge un numero (`nome-2.png`).
/// L'estensione segue il formato dell'immagine, se riconosciuto, perche' e'
/// da quella che si capisce come decodificarla.
pub fn save(dir: &Path, original: &str, bytes: &[u8]) -> anyhow::Result<String> {
    let ext = image::guess_format(bytes)
        .ok()
        .and_then(|format| format.extensions_str().first())
        .map(|ext| ext.to_string())
        .filter(|ext| EXTENSIONS.contains(&ext.as_str()))
        .or_else(|| extension(original))
        .unwrap_or_else(|| "png".to_string());
    let stem = sanitize_stem(original);
    std::fs::create_dir_all(dir)?;

    for n in 1.. {
        let file = if n == 1 { format!("{stem}.{ext}") } else { format!("{stem}-{n}.{ext}") };
        let path = dir.join(&file);
        match std::fs::read(&path) {
            Ok(existing) if existing == bytes => return Ok(file),
            Ok(_) => continue,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                std::fs::write(&path, bytes)?;
                return Ok(file);
            }
            Err(e) => return Err(e.into()),
        }
    }
    unreachable!()
}

/// Icone della libreria in ordine di nome, con i tasti che le usano.
pub fn list(dir: &Path, config: &Config) -> anyhow::Result<Vec<LibraryIcon>> {
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e.into()),
    };
    let mut icons: Vec<LibraryIcon> = entries
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_ok_and(|t| t.is_file()))
        .filter_map(|entry| entry.file_name().into_string().ok())
        .filter(|file| is_library_file(file))
        .map(|file| {
            let path = config_path(&file);
            let used_by = config
                .keys
                .iter()
                .filter(|k| k.icon.as_deref() == Some(path.as_str()))
                .map(|k| k.key)
                .collect();
            LibraryIcon { file, path, used_by }
        })
        .collect();
    icons.sort_by(|a, b| a.file.cmp(&b.file));
    Ok(icons)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("hassdeck-test-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn stems_are_readable_and_safe() {
        assert_eq!(sanitize_stem("Lampada Soggiorno.PNG"), "lampada-soggiorno");
        assert_eq!(sanitize_stem("../../etc/passwd"), "passwd");
        assert_eq!(sanitize_stem("C:\\foto\\tv_sala.jpg"), "tv_sala");
        assert_eq!(sanitize_stem("###.png"), "icona");
    }

    #[test]
    fn library_files_have_no_paths() {
        assert!(is_library_file("tv.png"));
        assert!(is_library_file("key5.JPG"));
        assert!(!is_library_file("../config.toml"));
        assert!(!is_library_file("sub/tv.png"));
        assert!(!is_library_file(".nascosto.png"));
        assert!(!is_library_file("script.sh"));
    }

    #[test]
    fn save_reuses_identical_files_and_numbers_the_others() {
        let dir = temp_dir("save");
        assert_eq!(save(&dir, "TV.png", b"uno").unwrap(), "tv.png");
        assert_eq!(save(&dir, "tv.png", b"uno").unwrap(), "tv.png");
        assert_eq!(save(&dir, "tv.png", b"due").unwrap(), "tv-2.png");
        assert_eq!(save(&dir, "tv.png", b"due").unwrap(), "tv-2.png");

        let mut config: Config = toml::from_str("").unwrap();
        crate::config::set_key_icon(&mut config, 4, config_path("tv-2.png"));
        let icons = list(&dir, &config).unwrap();
        let names: Vec<_> = icons.iter().map(|i| (i.file.as_str(), i.used_by.clone())).collect();
        assert_eq!(names, vec![("tv-2.png", vec![4]), ("tv.png", vec![])]);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
