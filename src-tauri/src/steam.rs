//! Escáner de la librería local de Steam.
//!
//! Estrategia: buscar los instaladores comunes de Steam, leer
//! `steamapps/libraryfolders.vdf` (rutas de todas las librerías) y luego
//! parsear cada `appmanifest_*.acf` para extraer appid / nombre / estado.
//! No requiere la API de Steam ni token: todo es lectura de archivos locales.

use std::collections::HashSet;
use std::fs;
use std::path::PathBuf;

use crate::Game;

/// Escanea todas las librerías de Steam instaladas y devuelve los juegos
/// marcados como instalados, ordenados alfabéticamente.
pub fn scan() -> Vec<Game> {
    let mut games: Vec<Game> = Vec::new();

    for library in library_paths() {
        let steamapps = library.join("steamapps");
        let Ok(entries) = fs::read_dir(&steamapps) else {
            continue;
        };

        for entry in entries.flatten() {
            let path = entry.path();
            let Some(file_name) = path.file_name().and_then(|n| n.to_str()) else {
                continue;
            };
            if !file_name.starts_with("appmanifest_") || !file_name.ends_with(".acf") {
                continue;
            }

            let Ok(content) = fs::read_to_string(&path) else {
                continue;
            };
            if let Some((app_id, name, installed)) = parse_acf(&content) {
                if installed && !name.trim().is_empty() {
                    games.push(Game {
                        id: format!("steam-{app_id}"),
                        app_id: Some(app_id),
                        name,
                        source: "steam".into(),
                        launch_path: None,
                        install_dir: None,
                    });
                }
            }
        }
    }

    games.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    games.dedup_by(|a, b| a.id == b.id);
    games
}

/// Lista de demostración (con AppIds reales para que el arte cargue del CDN).
pub fn demo_games() -> Vec<Game> {
    const DEMO: &[(&str, &str)] = &[
        ("1245620", "Elden Ring"),
        ("1593500", "God of War"),
        ("292030", "The Witcher 3: Wild Hunt"),
        ("1174180", "Horizon Zero Dawn"),
        ("1086940", "Baldur's Gate 3"),
    ];
    DEMO.iter()
        .map(|(app_id, name)| Game {
            id: format!("steam-{app_id}"),
            app_id: Some((*app_id).to_string()),
            name: (*name).to_string(),
            source: "demo".into(),
            launch_path: None,
            install_dir: None,
        })
        .collect()
}

/// Rutas raíz de las librerías de Steam (instalación + carpetas adicionales
/// declaradas en `libraryfolders.vdf`), sin duplicados.
fn library_paths() -> Vec<PathBuf> {
    let mut bases: Vec<PathBuf> = Vec::new();
    // Ubicaciones estándar de Windows. No depender solo de variables de
    // entorno: en algunos sistemas PROGRAMFILES(X86) no está presente y el
    // escáner se quedaría ciego, aunque Steam esté instalado ahí.
    bases.push(PathBuf::from(r"C:\Program Files (x86)\Steam"));
    bases.push(PathBuf::from(r"C:\Program Files\Steam"));
    if let Ok(pf86) = std::env::var("PROGRAMFILES(X86)") {
        bases.push(PathBuf::from(pf86).join("Steam"));
    }
    if let Ok(pf) = std::env::var("PROGRAMFILES") {
        bases.push(PathBuf::from(pf).join("Steam"));
    }
    if let Ok(local) = std::env::var("LOCALAPPDATA") {
        // Suele ser solo la carpeta de datos (userdata/cef), pero a veces
        // ahí vive un Steam portátil.
        bases.push(PathBuf::from(local).join("Steam"));
    }

    let mut libs: Vec<PathBuf> = Vec::new();
    for base in &bases {
        if !base.is_dir() {
            continue;
        }
        // El propio instalador es una librería.
        if base.join("steamapps").is_dir() {
            libs.push(base.clone());
        }
        // Librerías adicionales declaradas por el cliente.
        let vdf = base.join("steamapps").join("libraryfolders.vdf");
        if let Ok(content) = fs::read_to_string(&vdf) {
            libs.extend(parse_library_folders(&content));
        }
    }

    // Deduplicar (normalizando rutas).
    let mut seen: HashSet<PathBuf> = HashSet::new();
    libs.retain(|p| {
        let canonical = p.canonicalize().unwrap_or_else(|_| p.clone());
        seen.insert(canonical)
    });
    libs
}

// ---------------------------------------------------------------------------
// Parsing de VDF/ACF (formato clave-valor simple, una línea por par)
// ---------------------------------------------------------------------------

/// Extrae `(appid, name, instalado)` de un manifiesto ACF.
fn parse_acf(content: &str) -> Option<(String, String, bool)> {
    let map = kv_pairs(content);
    let app_id = map.get("appid")?.to_string();
    let name = map.get("name").cloned().unwrap_or_default();
    // Dos formatos según la versión de Steam:
    //  * antiguo: línea `"state" "3"` (3 = instalado)
    //  * moderno: bloque `"InstalledDepots" { ... }` presente solo si está instalado
    let installed = map.get("state").is_some_and(|s| s == "3")
        || quoted_strings(content).iter().any(|t| t == "InstalledDepots");
    Some((app_id, name, installed))
}

/// Extrae las rutas de librería de `libraryfolders.vdf`. El formato real es
/// anidado: cada índice numérico contiene un bloque con su propia línea
/// `"path" "D:\Juegos\Steam"`, así que simplemente recolectamos todas las
/// líneas cuyo valor corresponde a la clave `path`.
fn parse_library_folders(content: &str) -> Vec<PathBuf> {
    content
        .lines()
        .filter_map(|line| {
            // OJO: no usar `cond.then_some(f(x))`: Rust evalúa f(x) antes
            // de aplicar la condición y explotaría en líneas sin valor.
            let tokens = quoted_strings(line);
            match tokens.as_slice() {
                [key, value, ..] if key == "path" => Some(PathBuf::from(value.clone())),
                _ => None,
            }
        })
        .collect()
}

/// VDF/ACF son de hecho archivos línea por línea: cada línea contiene a lo
/// sumo un par `"clave" "valor"`. Parsear línea a línea evita el problema de
/// la clave raíz ("AppState", "libraryfolders") que desalinearía una lectura
/// plana del archivo.
fn kv_pairs(content: &str) -> std::collections::HashMap<String, String> {
    let mut map = std::collections::HashMap::new();
    for line in content.lines() {
        let tokens = quoted_strings(line);
        if tokens.len() >= 2 {
            map.insert(tokens[0].clone(), tokens[1].clone());
        }
    }
    map
}

/// Devuelve todas las cadenas entre comillas, en orden, respetando
/// escapes `\"` y `\\`.
fn quoted_strings(s: &str) -> Vec<String> {
    let bytes = s.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] != b'"' {
            i += 1;
            continue;
        }
        let start = i + 1;
        let mut j = start;
        while j < bytes.len() && bytes[j] != b'"' {
            if bytes[j] == b'\\' && j + 1 < bytes.len() && (bytes[j + 1] == b'"' || bytes[j + 1] == b'\\')
            {
                j += 2;
            } else {
                j += 1;
            }
        }
        if j < bytes.len() {
            let raw = &s[start..j];
            out.push(raw.replace("\\\"", "\"").replace("\\\\", "\\"));
            i = j + 1;
        } else {
            break; // comilla sin cerrar: descartar el resto
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Detalles de un juego instalado (para la ventana de detalles)
// ---------------------------------------------------------------------------

/// Datos extraídos de archivos locales para un juego específico.
#[derive(serde::Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct GameDetails {
    /// Ruta completa del directorio de instalación (ej: `D:\Juegos\Steam\steamapps\common\EldenRing`).
    pub install_path: Option<String>,
    /// Tamaño en disco (bytes). None si no está disponible.
    pub disk_size_bytes: Option<u64>,
    /// Cantidad de logros desbloqueados (solo Steam).
    pub achievements_unlocked: Option<u32>,
    /// Horas totales jugadas (solo Steam, si se pudo leer).
    pub hours_played: Option<f64>,
    /// Último timestamp jugado (epoch seconds) — opcional.
    pub last_played: Option<i64>,
}

/// Obtiene detalles de un juego por su AppId de Steam, leyendo archivos locales.
pub fn get_details(app_id: &str) -> GameDetails {
    let mut install_path = None;
    let mut disk_size_bytes = None;

    // 1) Buscar en todas las librerías el appmanifest correspondiente.
    for library in library_paths() {
        let acf_path = library.join("steamapps").join(format!("appmanifest_{app_id}.acf"));
        let Ok(content) = fs::read_to_string(&acf_path) else { continue; };
        let map = kv_pairs(&content);

        if let Some(installdir) = map.get("installdir").map(|s| s.clone()) {
            let full = library.join("steamapps").join("common").join(&installdir);
            install_path = Some(full.to_string_lossy().to_string());
        }
        if let Some(size_str) = map.get("SizeOnDisk").map(|s| s.clone()) {
            disk_size_bytes = size_str.parse::<u64>().ok();
        }
        break; // primera librería con el manifiesto
    }

    // 2) Datos de Steam: logros + tiempo jugado (requiere steamid).
    let mut achievements_unlocked = None;
    let mut hours_played = None;
    let mut last_played = None;

    if let Some(steamid) = find_steam_id() {
        // Logros: userdata/{steamid}/steam/apps/{appid}/stats.txt
        let stats_path = steam_base_dir()
            .map(|base| base.join("userdata").join(&steamid).join("steam").join("apps").join(app_id).join("stats.txt"));
        if let Some(sp) = &stats_path {
            if let Ok(content) = fs::read_to_string(sp) {
                let count = content.lines()
                    .filter(|l| l.starts_with("achieved "))
                    .count() as u32;
                if count > 0 { achievements_unlocked = Some(count); }
            }
        }

        // Tiempo jugado: userdata/{steamid}/config/localconfig.vdf
        let lc_path = steam_base_dir()
            .map(|base| base.join("userdata").join(&steamid).join("config").join("localconfig.vdf"));
        if let Some(lcp) = &lc_path {
            if let Ok(content) = fs::read_to_string(lcp) {
                hours_played = parse_localconfig_playtime(&content, app_id);
                last_played = parse_localconfig_lastplayed(&content, app_id);
            }
        }
    }

    GameDetails {
        install_path,
        disk_size_bytes,
        achievements_unlocked,
        hours_played,
        last_played,
    }
}

/// Busca el SteamID del usuario (el único directorio numérico bajo `userdata/`).
fn find_steam_id() -> Option<String> {
    let base = steam_base_dir()?;
    let userdata = base.join("userdata");
    let Ok(entries) = fs::read_dir(&userdata) else { return None; };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if name.len() >= 17 && name.chars().all(|c| c.is_ascii_digit()) {
            return Some(name);
        }
    }
    None
}

/// Devuelve la base de Steam (el directorio que contiene `steamapps/` + `userdata/`).
fn steam_base_dir() -> Option<PathBuf> {
    for lib in library_paths() {
        // El directorio base es el padre de `steamapps`.
        if lib.join("steamapps").is_dir() {
            return Some(lib);
        }
    }
    None
}

/// Extrae `HoursOnRecord` del bloque de la app en localconfig.vdf.
fn parse_localconfig_playtime(content: &str, app_id: &str) -> Option<f64> {
    // Buscar la línea que contiene el appid y luego HoursOnRecord dentro del bloque.
    let lines: Vec<&str> = content.lines().collect();
    let mut in_app_block = false;
    for line in &lines {
        if line.contains(format!("\"{app_id}\"").as_str()) {
            in_app_block = true;
            continue;
        }
        if in_app_block {
            let tokens = quoted_strings(line);
            match tokens.as_slice() {
                [key, value] if key == "HoursOnRecord" => { return value.parse().ok(); }
                _ => {}
            }
            // Salir del bloque si encontramos el cierre.
            if line.trim_end().ends_with('}') {
                in_app_block = false;
            }
        }
    }
    None
}

/// Extrae `LastPlayed` (epoch) del bloque de la app en localconfig.vdf.
fn parse_localconfig_lastplayed(content: &str, app_id: &str) -> Option<i64> {
    let lines: Vec<&str> = content.lines().collect();
    let mut in_app_block = false;
    for line in &lines {
        if line.contains(format!("\"{app_id}\"").as_str()) {
            in_app_block = true;
            continue;
        }
        if in_app_block {
            let tokens = quoted_strings(line);
            match tokens.as_slice() {
                [key, value] if key == "LastPlayed" => { return value.parse().ok(); }
                _ => {}
            }
            if line.trim_end().ends_with('}') {
                in_app_block = false;
            }
        }
    }
    None
}

// Nota: la URL del arte (CDN de Steam) vive en el frontend (ui/app.js),
// porque ahí se decide el fallback de imágenes.

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parsea_acf_formato_antiguo_state() {
        let acf = r#"
"AppState"
{
	"appid"		"1245620"
	"name"		"Elden Ring"
	"state"		"3"
	"installdir"	"EldenRing"
}
        "#;
        let (app_id, name, installed) = parse_acf(acf).unwrap();
        assert_eq!(app_id, "1245620");
        assert_eq!(name, "Elden Ring");
        assert!(installed);
    }

    #[test]
    fn parsea_acf_formato_moderno_state_flags() {
        // Formato que escribe el Steam actual: StateFlags + bloque InstalledDepots.
        let acf = r#"
"AppState"
{
	"appid"		"292030"
	"name"		"The Witcher 3: Wild Hunt — Remastered"
	"StateFlags"		"4"
	"InstalledDepots"
	{
		"292031"		"58761198014"
	}
}
        "#;
        let (app_id, name, installed) = parse_acf(acf).unwrap();
        assert_eq!(app_id, "292030");
        assert!(name.starts_with("The Witcher 3"));
        assert!(installed);
    }

    #[test]
    fn acf_sin_depots_no_esta_instalado() {
        let acf = r#"
"AppState"
{
	"appid"		"1174180"
	"name"		"Horizon Zero Dawn"
	"StateFlags"		"1"
}
        "#;
        let (_, _, installed) = parse_acf(acf).unwrap();
        assert!(!installed);
    }

    #[test]
    fn parsea_library_folders_formato_anidado() {
        let vdf = r#"
"libraryfolders"
{
	"0"
	{
		"path"		"C:\\Program Files (x86)\\Steam"
		"label"		""
	}
	"1"
	{
		"path"		"D:\\Juegos\\Steam"
		"label"		""
	}
	"2"
	{
		"path"		"M:\\Steam"
		"label"		""
	}
}
        "#;
        let paths = parse_library_folders(vdf);
        assert_eq!(paths.len(), 3);
        assert!(paths[0].ends_with("Steam"));
        assert!(paths[1].ends_with(r"Juegos\Steam"));
    }
}
