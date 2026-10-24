//! Escáner universal de juegos instalados — lógica tipo DLSS5-Autopilot.
//!
//! NO lee el registro genérico de Windows Uninstall (que trae Office, Chrome, etc.).
//! En cambio, cada launcher tiene su scanner específico que solo detecta juegos:
//! Epic, GOG, EA, Ubisoft, Battle.net, Rockstar, Amazon, itch, Heroic, Xbox,
//! y directorios tipo "Games".

use crate::Game;
use std::collections::HashSet;
use std::path::{Path, PathBuf};

// ===========================================================================
// Punto de entrada: escanea todos los launchers conocidos
// ===========================================================================

/// Escanea TODOS los launchers/directorios y devuelve juegos deduplicados.
pub fn scan_all_external() -> Vec<Game> {
    let mut games: Vec<Game> = Vec::new();

    // Cada scanner devuelve solo juegos de su plataforma.
    games.extend(scan_epic());
    games.extend(scan_gog());
    games.extend(scan_ea());
    games.extend(scan_ubisoft());
    games.extend(scan_battlenet());
    games.extend(scan_rockstar());
    games.extend(scan_amazon());
    games.extend(scan_itch());
    games.extend(scan_heroic());
    games.extend(scan_xbox());
    games.extend(scan_game_folders());

    // Log de resumen.
    let mut counts: std::collections::HashMap<&str, usize> = Default::default();
    for g in &games {
        *counts.entry(g.source.as_str()).or_insert(0) += 1;
    }
    if !counts.is_empty() {
        let parts: Vec<String> = counts.iter().map(|(k, v)| format!("{v} {k}")).collect();
        eprintln!("[cardynox] Launchers: {} ({})", games.len(), parts.join(", "));
    }

    // Deduplicar por exe path (lowercase).
    dedup_by_exe(&mut games);

    games
}

/// Deduplica juegos que apuntan al mismo .exe (una plataforma ya lo cubre).
fn dedup_by_exe(games: &mut Vec<Game>) {
    let mut seen: HashSet<String> = HashSet::new();
    games.retain(|g| {
        if let Some(exe) = &g.launch_path {
            let norm = exe.to_lowercase();
            seen.insert(norm)
        } else {
            true // sin launch_path → mantener (Steam games usan steam://)
        }
    });
}

// ===========================================================================
// Epic Games Launcher
// ===========================================================================

/// Lee `%PROGRAMDATA%\Epic\EpicGamesLauncher\Data\Manifests\*.item`
/// Filtro: bIsApplication != false AND bIsExecutable != false.
fn scan_epic() -> Vec<Game> {
    let man_dir = std::env::var("PROGRAMDATA")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from(r"C:\ProgramData"))
        .join("Epic/EpicGamesLauncher/Data/Manifests");

    if !man_dir.is_dir() {
        return Vec::new();
    }

    let mut out = Vec::new();
    if let Ok(entries) = std::fs::read_dir(&man_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("item") {
                continue;
            }
            let Ok(text) = std::fs::read_to_string(&path) else { continue };
            let Ok(data) = serde_json::from_str::<serde_json::Value>(&text) else { continue };

            // Filtro: debe ser aplicación ejecutable.
            if data.get("bIsApplication").and_then(|v| v.as_bool()) == Some(false)
                || data.get("bIsExecutable").and_then(|v| v.as_bool()) == Some(false)
            {
                continue;
            }

            let install_loc = match data.get("InstallLocation").and_then(|v| v.as_str()) {
                Some(loc) if Path::new(loc).is_dir() => loc.to_string(),
                _ => continue,
            };

            let display_name = data
                .get("DisplayName")
                .and_then(|v| v.as_str())
                .map(String::from)
                .or_else(|| {
                    PathBuf::from(&install_loc)
                        .file_name()
                        .map(|n| n.to_string_lossy().to_string())
                })
                .unwrap_or_else(|| "Epic Game".into());

            // LaunchExecutable → ruta al .exe (relativa a InstallLocation).
            let launch_exe = data
                .get("LaunchExecutable")
                .and_then(|v| v.as_str())
                .map(|le| {
                    let full = PathBuf::from(&install_loc).join(le);
                    if full.is_file() { Some(full.to_string_lossy().to_string()) } else { None }
                })
                .flatten();

            // Si no hay LaunchExecutable, buscar el .exe principal.
            let exe_path = launch_exe.or_else(|| find_main_exe(Path::new(&install_loc)));

            let Some(exe) = exe_path else { continue };

            out.push(Game {
                id: format!("epic-{}", path_hash(&exe)),
                name: display_name,
                app_id: None,
                source: "Epic".into(),
                launch_path: Some(exe),
                install_dir: Some(install_loc),
            });
        }
    }
    out
}

// ===========================================================================
// GOG Galaxy / GOG Classic
// ===========================================================================

/// Lee `HKLM\SOFTWARE\{WOW6432Node\}GOG.com\Games\*` → valores `path`, `gameName`.
fn scan_gog() -> Vec<Game> {
    let mut out = Vec::new();
    #[cfg(target_os = "windows")]
    {
        use winreg::enums::HKEY_LOCAL_MACHINE;
        use winreg::RegKey;

        const KEYS: [&str; 2] = [
            r"SOFTWARE\WOW6432Node\GOG.com\Games",
            r"SOFTWARE\GOG.com\Games",
        ];

        let hklm = RegKey::predef(HKEY_LOCAL_MACHINE);
        for key_path in KEYS {
            let Ok(root) = hklm.open_subkey(key_path) else { continue };
            for sub_name in root.enum_keys().flatten() {
                let Ok(sub) = root.open_subkey(&sub_name) else { continue };
                let Ok(game_name) = sub.get_value::<String, _>("gameName") else { continue };
                let Ok(path) = sub.get_value::<String, _>("path") else { continue };
                if !Path::new(&path).is_dir() {
                    continue;
                }
                let exe = find_main_exe(Path::new(&path));
                let Some(exe_path) = exe else { continue };

                out.push(Game {
                    id: format!("gog-{}", path_hash(&exe_path)),
                    name: game_name,
                    app_id: None,
                    source: "GOG".into(),
                    launch_path: Some(exe_path),
                    install_dir: Some(path),
                });
            }
        }
    }
    out
}

// ===========================================================================
// Electronic Arts (EA)
// ===========================================================================

/// Lee `HKLM\SOFTWARE\{WOW6432Node\}Electronic Arts\*` → valor `Install Dir`.
/// También chequea carpetas estándar de EA Games.
fn scan_ea() -> Vec<Game> {
    let mut out = Vec::new();
    #[cfg(target_os = "windows")]
    {
        use winreg::enums::HKEY_LOCAL_MACHINE;
        use winreg::RegKey;

        const KEYS: [&str; 2] = [
            r"SOFTWARE\WOW6432Node\Electronic Arts",
            r"SOFTWARE\Electronic Arts",
        ];

        let hklm = RegKey::predef(HKEY_LOCAL_MACHINE);
        for key_path in KEYS {
            let Ok(root) = hklm.open_subkey(key_path) else { continue };
            for sub_name in root.enum_keys().flatten() {
                let Ok(sub) = root.open_subkey(&sub_name) else { continue };
                // El valor se llama "Install Dir" (con espacio).
                let Ok(install_dir) = sub.get_value::<String, _>("Install Dir") else { continue };
                if !Path::new(&install_dir).is_dir() {
                    continue;
                }
                let exe = find_main_exe(Path::new(&install_dir));
                let Some(exe_path) = exe else { continue };

                out.push(Game {
                    id: format!("ea-{}", path_hash(&exe_path)),
                    name: sub_name,
                    app_id: None,
                    source: "EA".into(),
                    launch_path: Some(exe_path),
                    install_dir: Some(install_dir),
                });
            }
        }
    }

    // Carpetas estándar de EA Games.
    for base in ea_default_dirs() {
        if !base.is_dir() {
            continue;
        }
        if let Ok(entries) = std::fs::read_dir(&base) {
            for entry in entries.flatten() {
                let dir = entry.path();
                if !dir.is_dir() {
                    continue;
                }
                let exe = find_main_exe(&dir);
                let Some(exe_path) = exe else { continue };
                out.push(Game {
                    id: format!("ea-{}", path_hash(&exe_path)),
                    name: dir.file_name()
                        .map(|n| n.to_string_lossy().to_string())
                        .unwrap_or_default(),
                    app_id: None,
                    source: "EA".into(),
                    launch_path: Some(exe_path),
                    install_dir: Some(dir.to_string_lossy().to_string()),
                });
            }
        }
    }
    out
}

fn ea_default_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    for (k, v) in std::env::vars_os() {
        let k_str = k.to_string_lossy().to_uppercase();
        if k_str == "PROGRAMFILES" || k_str == "PROGRAMFILES(X86)" {
            dirs.push(PathBuf::from(v).join("EA Games"));
        }
    }
    dirs
}

// ===========================================================================
// Ubisoft Connect
// ===========================================================================

/// Lee `HKLM\SOFTWARE\{WOW6432Node\}Ubisoft\Launcher\Installs\*` → valor `InstallDir`.
fn scan_ubisoft() -> Vec<Game> {
    let mut out = Vec::new();
    #[cfg(target_os = "windows")]
    {
        use winreg::enums::HKEY_LOCAL_MACHINE;
        use winreg::RegKey;

        const KEYS: [&str; 2] = [
            r"SOFTWARE\WOW6432Node\Ubisoft\Launcher\Installs",
            r"SOFTWARE\Ubisoft\Launcher\Installs",
        ];

        let hklm = RegKey::predef(HKEY_LOCAL_MACHINE);
        for key_path in KEYS {
            let Ok(root) = hklm.open_subkey(key_path) else { continue };
            for sub_name in root.enum_keys().flatten() {
                let Ok(sub) = root.open_subkey(&sub_name) else { continue };
                let Ok(install_dir) = sub.get_value::<String, _>("InstallDir") else { continue };
                if !Path::new(&install_dir).is_dir() {
                    continue;
                }
                let exe = find_main_exe(Path::new(&install_dir));
                let Some(exe_path) = exe else { continue };

                out.push(Game {
                    id: format!("ubi-{}", path_hash(&exe_path)),
                    name: sub_name,
                    app_id: None,
                    source: "Ubisoft".into(),
                    launch_path: Some(exe_path),
                    install_dir: Some(install_dir),
                });
            }
        }
    }
    out
}

// ===========================================================================
// Battle.net
// ===========================================================================

/// Lee el registro genérico Uninstall pero filtra SOLO subkeys cuyo nombre
/// contenga "battle.net" (case-insensitive).
fn scan_battlenet() -> Vec<Game> {
    let mut out = Vec::new();
    #[cfg(target_os = "windows")]
    {
        use winreg::enums::{HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE};
        use winreg::RegKey;

        const UNINSTALL: &str = r"SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall";
        const WOW_UNINSTALL: &str =
            r"SOFTWARE\WOW6432Node\Microsoft\Windows\CurrentVersion\Uninstall";

        let hklm = RegKey::predef(HKEY_LOCAL_MACHINE);
        let hkcu = RegKey::predef(HKEY_CURRENT_USER);

        // Helper closure: procesa una key Uninstall filtrando por "battle.net".
        let mut scan_one = |base: &winreg::RegKey| {
            for sub_name in base.enum_keys().flatten() {
                if !sub_name.to_lowercase().contains("battle.net") {
                    continue;
                }
                let Ok(sub) = base.open_subkey(&sub_name) else { continue };
                let Ok(name) = sub.get_value::<String, _>("DisplayName") else { continue };
                if name.is_empty() { continue; }
                let Ok(loc) = sub.get_value::<String, _>("InstallLocation") else { continue };
                if !Path::new(&loc).is_dir() { continue; }
                let exe = find_main_exe(Path::new(&loc));
                let Some(exe_path) = exe else { continue };

                out.push(Game {
                    id: format!("bn-{}", path_hash(&exe_path)),
                    name,
                    app_id: None,
                    source: "Battle.net".into(),
                    launch_path: Some(exe_path),
                    install_dir: Some(loc),
                });
            }
        };

        if let Ok(k) = hklm.open_subkey(UNINSTALL) { scan_one(&k); }
        if let Ok(k) = hklm.open_subkey(WOW_UNINSTALL) { scan_one(&k); }
        if let Ok(k) = hkcu.open_subkey(UNINSTALL) { scan_one(&k); }
    }
    out
}

// ===========================================================================
// Rockstar Games Launcher
// ===========================================================================

/// Lee `HKLM\SOFTWARE\{WOW6432Node\}Rockstar Games\*` → valor `InstallFolder`.
/// Excluye subkeys del launcher/social club (no son juegos).
fn scan_rockstar() -> Vec<Game> {
    let mut out = Vec::new();
    #[cfg(target_os = "windows")]
    {
        use winreg::enums::HKEY_LOCAL_MACHINE;
        use winreg::RegKey;

        const KEYS: [&str; 2] = [
            r"SOFTWARE\WOW6432Node\Rockstar Games",
            r"SOFTWARE\Rockstar Games",
        ];
        // Subkeys que NO son juegos.
        const NOT_GAMES: [&str; 4] = [
            "launcher",
            "rockstar games launcher",
            "rockstar games social club",
            "social club",
        ];

        let hklm = RegKey::predef(HKEY_LOCAL_MACHINE);
        for key_path in KEYS {
            let Ok(root) = hklm.open_subkey(key_path) else { continue };
            for sub_name in root.enum_keys().flatten() {
                let lower = sub_name.to_lowercase();
                if NOT_GAMES.contains(&lower.as_str()) {
                    continue;
                }
                let Ok(sub) = root.open_subkey(&sub_name) else { continue };
                let Ok(folder) = sub.get_value::<String, _>("InstallFolder") else { continue };
                if !Path::new(&folder).is_dir() {
                    continue;
                }
                let exe = find_main_exe(Path::new(&folder));
                let Some(exe_path) = exe else { continue };

                out.push(Game {
                    id: format!("rstar-{}", path_hash(&exe_path)),
                    name: sub_name,
                    app_id: None,
                    source: "Rockstar".into(),
                    launch_path: Some(exe_path),
                    install_dir: Some(folder),
                });
            }
        }
    }
    out
}

// ===========================================================================
// Amazon Games
// ===========================================================================

/// Chequea `%LOCALAPPDATA%\Amazon Games\Library\*` — cada subcarpeta es un juego.
fn scan_amazon() -> Vec<Game> {
    let mut out = Vec::new();
    let localappdata = std::env::var("LOCALAPPDATA").unwrap_or_default();
    if localappdata.is_empty() {
        return out;
    }

    let base = PathBuf::from(&localappdata).join("Amazon Games/Library");
    scan_game_subfolders(&base, "Amazon", &mut out);
    out
}

// ===========================================================================
// itch.io
// ===========================================================================

/// Chequea `%APPDATA%\itch\apps\*` — cada subcarpeta es un juego.
fn scan_itch() -> Vec<Game> {
    let mut out = Vec::new();
    let appdata = std::env::var("APPDATA").unwrap_or_default();
    if appdata.is_empty() {
        return out;
    }

    let base = PathBuf::from(&appdata).join("itch/apps");
    scan_game_subfolders(&base, "itch", &mut out);
    out
}

// ===========================================================================
// Heroic Games Launcher (Epic/GOG/Amazon vía un launcher)
// ===========================================================================

/// Lee JSONs de Heroic: `legendaryConfig/legendary/installed.json`,
/// `gog_store/installed.json`, `nile_config/nile/installed.json`.
fn scan_heroic() -> Vec<Game> {
    let mut out = Vec::new();
    let appdata = std::env::var("APPDATA").unwrap_or_default();
    if appdata.is_empty() {
        return out;
    }

    let heroic_dir = PathBuf::from(&appdata).join("heroic");
    let json_files = [
        "legendaryConfig/legendary/installed.json",
        "gog_store/installed.json",
        "nile_config/nile/installed.json",
    ];

    for rel in &json_files {
        let path = heroic_dir.join(rel);
        if !path.is_file() {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(&path) else { continue };
        let Ok(data) = serde_json::from_str::<serde_json::Value>(&text) else { continue };

        // El JSON puede ser un dict o una lista.
        let items: Vec<&serde_json::Value> = if data.is_object() {
            data.as_object().unwrap().values().collect()
        } else if data.is_array() {
            data.as_array().unwrap().iter().collect()
        } else {
            continue;
        };

        for item in items {
            let loc = item
                .get("install_path")
                .or_else(|| item.get("path"))
                .and_then(|v| v.as_str());
            let name = item
                .get("title")
                .or_else(|| item.get("app_name"))
                .and_then(|v| v.as_str());

            let (Some(loc), Some(name)) = (loc, name) else { continue };
            if !Path::new(loc).is_dir() || name.is_empty() {
                continue;
            }

            let exe = find_main_exe(Path::new(loc));
            let Some(exe_path) = exe else { continue };

            out.push(Game {
                id: format!("heroic-{}", path_hash(&exe_path)),
                name: name.to_string(),
                app_id: None,
                source: "Heroic".into(),
                launch_path: Some(exe_path),
                install_dir: Some(loc.to_string()),
            });
        }
    }
    out
}

// ===========================================================================
// Xbox / Game Pass
// ===========================================================================

/// Chequea `X:\XboxGames\*` y `X:\Program Files\ModifiableWindowsApps\*`
/// para cada drive C-H. Excluye GameSave y Minecraft Launcher.
fn scan_xbox() -> Vec<Game> {
    let mut out = Vec::new();
    const NOT_GAMES: [&str; 2] = ["gamesave", "minecraft launcher"];

    for letter in b'C'..=b'H' {
        let drive = format!("{}:", (letter as char).to_lowercase());

        // X:\XboxGames\*
        let xbox_root = PathBuf::from(format!("{drive}\\XboxGames"));
        if xbox_root.is_dir() {
            if let Ok(entries) = std::fs::read_dir(&xbox_root) {
                for entry in entries.flatten() {
                    let dir = entry.path();
                    if !dir.is_dir() {
                        continue;
                    }
                    let folder_name = dir.file_name().unwrap_or_default().to_string_lossy().to_lowercase();
                    if NOT_GAMES.contains(&folder_name.as_str()) {
                        continue;
                    }
                    // XboxGames pone los archivos reales en Content/ a veces.
                    let content = dir.join("Content");
                    let actual = if content.is_dir() { &content } else { &dir };

                    let exe = find_main_exe(actual);
                    let Some(exe_path) = exe else { continue };

                    out.push(Game {
                        id: format!("xbox-{}", path_hash(&exe_path)),
                        name: dir.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default(),
                        app_id: None,
                        source: "Xbox".into(),
                        launch_path: Some(exe_path),
                        install_dir: Some(dir.to_string_lossy().to_string()),
                    });
                }
            }
        }

        // X:\Program Files\ModifiableWindowsApps\*
        let mwa_root = PathBuf::from(format!("{drive}\\Program Files\\ModifiableWindowsApps"));
        if mwa_root.is_dir() {
            if let Ok(entries) = std::fs::read_dir(&mwa_root) {
                for entry in entries.flatten() {
                    let dir = entry.path();
                    if !dir.is_dir() {
                        continue;
                    }
                    let folder_name = dir.file_name().unwrap_or_default().to_string_lossy().to_lowercase();
                    if NOT_GAMES.contains(&folder_name.as_str()) {
                        continue;
                    }

                    let exe = find_main_exe(&dir);
                    let Some(exe_path) = exe else { continue };

                    out.push(Game {
                        id: format!("xbox-{}", path_hash(&exe_path)),
                        name: dir.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default(),
                        app_id: None,
                        source: "Xbox".into(),
                        launch_path: Some(exe_path),
                        install_dir: Some(dir.to_string_lossy().to_string()),
                    });
                }
            }
        }
    }
    out
}

// ===========================================================================
// Carpetas tipo "Games" / "Juegos" en drives locales
// ===========================================================================

/// Escanea directorios llamados "Games" o "Juegos" en cada drive.
/// Aplica blacklist de carpetas de launcher que no son juegos.
fn scan_game_folders() -> Vec<Game> {
    let mut out = Vec::new();

    // Blacklist: subcarpetas que son librerías de launchers, no juegos.
    const JUNK: [&str; 12] = [
        "steamlibrary", "steamapps", "epic games", "gog galaxy",
        "ea games", "ubisoft", "xboxgames", "amazon games",
        "battle.net", "common", "riot games", "rockstar games",
    ];

    // Nombres de carpetas a buscar.
    const FOLDER_NAMES: [&str; 3] = ["Games", "Juegos", "SteamLibrary"];

    for letter in b'C'..=b'H' {
        let drive = format!("{}:", (letter as char).to_lowercase());

        for folder_name in &FOLDER_NAMES {
            let base = PathBuf::from(format!("{drive}\\{folder_name}"));
            if !base.is_dir() {
                continue;
            }

            if let Ok(entries) = std::fs::read_dir(&base) {
                for entry in entries.flatten() {
                    let dir = entry.path();
                    if !dir.is_dir() {
                        continue;
                    }

                    let name_lower = dir.file_name().unwrap_or_default().to_string_lossy().to_lowercase();
                    if JUNK.contains(&name_lower.as_str()) {
                        continue;
                    }

                    // Buscar .exe en el directorio (nivel 0 y 1).
                    let exe = find_main_exe(&dir).or_else(|| {
                        std::fs::read_dir(&dir)
                            .ok()
                            .and_then(|sub| {
                                sub.flatten().find_map(|se| {
                                    let sp = se.path();
                                    if sp.is_dir() {
                                        find_main_exe(&sp)
                                    } else {
                                        None
                                    }
                                })
                            })
                    });

                    let Some(exe_path) = exe else { continue };

                    // Filtro mínimo: exe debe pesar >5MB (descarta stubs).
                    if std::fs::metadata(&exe_path).map(|m| m.len() < 5_000_000).unwrap_or(true) {
                        continue;
                    }

                    out.push(Game {
                        id: format!("folder-{}", path_hash(&exe_path)),
                        name: dir.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default(),
                        app_id: None,
                        source: "Folder".into(),
                        launch_path: Some(exe_path),
                        install_dir: Some(dir.to_string_lossy().to_string()),
                    });
                }
            }
        }
    }
    out
}

// ===========================================================================
// Scan de directorios configurables por el usuario (scan_roots)
// ===========================================================================

/// Escanea una lista de directorios raíz configurados por el usuario.
pub fn scan_directories(roots: &[String]) -> Vec<Game> {
    let mut games = Vec::new();
    let mut seen = HashSet::new();

    for root in roots {
        let root_path = Path::new(root);
        if !root_path.is_dir() {
            eprintln!("[cardynox] scan_directories: no existe {root}");
            continue;
        }

        let Ok(entries) = std::fs::read_dir(root_path) else { continue };

        for entry in entries.flatten() {
            let dir_path = entry.path();
            if !dir_path.is_dir() {
                continue;
            }

            let exe = find_main_exe(&dir_path).or_else(|| {
                std::fs::read_dir(&dir_path)
                    .ok()
                    .and_then(|sub| {
                        sub.flatten().find_map(|se| {
                            let sp = se.path();
                            if sp.is_dir() {
                                find_main_exe(&sp)
                            } else {
                                None
                            }
                        })
                    })
            });

            let Some(exe_path) = exe else { continue };

            // Filtro >5MB.
            if std::fs::metadata(&exe_path).map(|m| m.len() < 5_000_000).unwrap_or(true) {
                continue;
            }

            let exe_norm = exe_path.to_lowercase();
            if seen.contains(&exe_norm) {
                continue;
            }
            seen.insert(exe_norm);

            let game_name = dir_path
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_else(|| "Juego".into());

            games.push(Game {
                id: format!("dir-{:08x}", path_hash(&exe_path)),
                name: game_name,
                app_id: None,
                source: "Folder".into(),
                launch_path: Some(exe_path),
                install_dir: Some(dir_path.to_string_lossy().to_string()),
            });
        }
    }

    games
}

// ===========================================================================
// Helpers
// ===========================================================================

/// Escanea subcarpetas de un directorio base. Cada subcarpeta con un .exe es un juego.
fn scan_game_subfolders(base: &Path, source: &str, out: &mut Vec<Game>) {
    if !base.is_dir() {
        return;
    }
    if let Ok(entries) = std::fs::read_dir(base) {
        for entry in entries.flatten() {
            let dir = entry.path();
            if !dir.is_dir() {
                continue;
            }
            let exe = find_main_exe(&dir);
            let Some(exe_path) = exe else { continue };

            // Filtro >5MB.
            if std::fs::metadata(&exe_path).map(|m| m.len() < 5_000_000).unwrap_or(true) {
                continue;
            }

            out.push(Game {
                id: format!("{source}-{}", path_hash(&exe_path)),
                name: dir.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default(),
                app_id: None,
                source: source.into(),
                launch_path: Some(exe_path),
                install_dir: Some(dir.to_string_lossy().to_string()),
            });
        }
    }
}

/// Busca un .exe "principal" en una carpeta.
/// Estrategia: preferir el que coincide con el nombre de la carpeta,
/// luego el más grande. Saltear uninstallers/setups.
fn find_main_exe(dir: &Path) -> Option<String> {
    let folder_name = dir.file_name()?.to_string_lossy().to_lowercase();

    let mut candidates: Vec<(u64, String)> = Vec::new();

    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_file() {
                continue;
            }
            let name = path.file_name()?.to_string_lossy().to_lowercase();
            if !name.ends_with(".exe") {
                continue;
            }
            // Saltar conocidos no-juegos.
            if name.starts_with("uninstall")
                || name == "unins000.exe"
                || name.starts_with("setup")
                || name == "launcher.exe"
                || name == "update.exe"
            {
                continue;
            }
            let size = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
            if size < 1_000_000 {
                continue; // <1MB → probablemente no es el principal
            }
            candidates.push((size, path.to_string_lossy().to_string()));
        }
    }

    // Preferir el .exe que coincida con el nombre de la carpeta.
    if let Some((_, path)) = candidates.iter().find(|(_, p)| {
        Path::new(p)
            .file_stem()
            .map(|s| s.to_string_lossy().to_lowercase() == folder_name)
            .unwrap_or(false)
    }) {
        return Some(path.clone());
    }

    // Sino, el más grande.
    candidates.sort_by(|a, b| b.0.cmp(&a.0));
    candidates.first().map(|(_, p)| p.clone())
}

/// Hash simple de una ruta para generar ids estables.
fn path_hash(path: &str) -> u32 {
    let mut hash: u32 = 5381;
    for b in path.bytes() {
        hash = hash.wrapping_mul(31).wrapping_add(b as u32);
    }
    hash
}
