mod scanner;
mod steam;

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Duration;
use tauri::{
    menu::{CheckMenuItem, CheckMenuItemBuilder, IsMenuItem, Menu, MenuItem, Submenu},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    Emitter, LogicalPosition, LogicalSize, Manager, WebviewWindow, WebviewWindowBuilder,
    WebviewUrl, Wry,
};

/// Un juego listo para mostrarse en el carrusel.
#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Game {
    /// Id estable interno (p. ej. `steam-1245620`, `reg-GUID`, `dir-hash`).
    pub id: String,
    pub name: String,
    /// AppId de Steam si el juego viene de la librería (o demo). Para arte CDN.
    pub app_id: Option<String>,
    /// "steam" | "demo" | "registry" | "folder"
    pub source: String,
    /// Ruta directa al .exe (juegos no-Steam: registry, folder).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub launch_path: Option<String>,
    /// Carpeta de instalación (para current_dir al lanzar).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub install_dir: Option<String>,
}

/// Un juego agregado manualmente desde el botón “Agregar” del gadget.
/// Se persiste en `GadgetConfig.manual_games`; `list_games()` lo convierte en `Game`.
#[derive(Serialize, Deserialize, Clone)]
struct ManualGame {
    /// Id determinístico (`manual-<hash(exe)>`): re-agregar el mismo .exe
    /// actualiza el nombre en vez de duplicar la tarjeta.
    id: String,
    /// Nombre visible elegido por el usuario (editable desde Detalles).
    name: String,
    /// Ruta completa al .exe del juego.
    exe: String,
}

/// Hash estable y simple (djb2) sobre los bytes de `s`. No necesita semilla:
/// solo sirve para un id corto y determinístico por ruta de ejecutable.
fn stable_hash(s: &str) -> u32 {
    let mut h: u32 = 5381;
    for b in s.bytes() {
        h = h.wrapping_mul(33) ^ (b as u32);
    }
    h
}

// ---------------------------------------------------------------------------
// Commands expuestos al frontend (ui/app.js)
// ---------------------------------------------------------------------------

/// Lista TODOS los juegos detectados: Steam + registro Windows + directorios.
/// Si no hay Steam (o está vacía), incluye una lista demo como respaldo.
#[tauri::command]
fn list_games(app: tauri::AppHandle) -> Vec<Game> {
    let cfg = load_config(&app);

    // 1) Steam.
    let mut games = steam::scan();
    if games.is_empty() {
        eprintln!("[cardynox] No se detectó librería de Steam; usando juegos demo.");
        games = steam::demo_games();
    } else {
        eprintln!("[cardynox] Steam: {} juegos.", games.len());
    }

    // 2) Launchers específicos (Epic, GOG, EA, Ubisoft, Battle.net, Rockstar,
    //    Amazon, itch, Heroic, Xbox, carpetas Games).
    let ext_games = scanner::scan_all_external();
    if !ext_games.is_empty() {
        eprintln!("[cardynox] Launchers: {} juegos adicionales.", ext_games.len());
        games.extend(ext_games);
    }

    // 3) Scan de directorios configurados (juegos sueltos, emuladores).
    if !cfg.scan_roots.is_empty() {
        let dir_games = scanner::scan_directories(&cfg.scan_roots);
        if !dir_games.is_empty() {
            eprintln!("[cardynox] Directorios: {} juegos.", dir_games.len());
            games.extend(dir_games);
        }
    }

    // 4) Cross-dedup: si un juego externo coincide con uno de Steam por nombre
    //    normalizado, se prioriza el de Steam (tiene datos más ricos + steam://).
    let steam_names: std::collections::HashSet<String> = games.iter()
        .filter(|g| g.source.to_lowercase() == "steam" || g.source.to_lowercase() == "demo")
        .map(|g| normalize_name(&g.name))
        .collect();
    games.retain(|g| {
        let is_external = g.app_id.is_none();
        if !is_external { return true; }
        // Descartar juegos en carpetas steamapps (Steam ya los cubre).
        if let Some(dir) = &g.install_dir {
            if dir.to_lowercase().contains("steamapps\\common") || dir.to_lowercase().contains("steamapps/common") {
                return false;
            }
        }
        // Descartar por nombre duplicado con Steam.
        !steam_names.contains(&normalize_name(&g.name))
    });

    // 4b) Juegos agregados manualmente. Se descartan si ya hay un juego
    //     detectado con el mismo .exe o el mismo nombre normalizado (se gana
    //     el detectado, para no duplicar la tarjeta).
    for m in &cfg.manual_games {
        let exe_lower = m.exe.to_lowercase();
        let name_n = normalize_name(&m.name);
        let conflict = games.iter().any(|g| {
            g.launch_path
                .as_ref()
                .map(|p| p.to_lowercase() == exe_lower)
                .unwrap_or(false)
                || normalize_name(&g.name) == name_n
        });
        if conflict {
            continue;
        }
        games.push(Game {
            id: m.id.clone(),
            name: m.name.clone(),
            app_id: None,
            source: "manual".into(),
            launch_path: Some(m.exe.clone()),
            install_dir: std::path::Path::new(&m.exe)
                .parent()
                .map(|p| p.to_string_lossy().to_string()),
        });
    }

    // 5) Aplicar renombres personalizados y overrides de ejecutable.
    //    Steam: clave = app_id ("1245620"). No-Steam: clave = id ("epic-a1b2c3d4").
    for game in &mut games {
        let key = match &game.app_id {
            Some(ref aid) => aid.clone(),
            None => game.id.clone(),
        };
        if let Some(custom) = cfg.renames.get(&key) {
            game.name = custom.clone();
        }
        // No-Steam: permite cambiar el .exe al que apunta (clave = id).
        // No aplica a Steam: ahí el lanzamiento siempre va por steam://.
        if game.app_id.is_none() {
            if let Some(exe) = cfg.launch_overrides.get(&game.id) {
                game.launch_path = Some(exe.clone());
            }
        }
    }

    // 6) Quitar los juegos que el usuario sacó del launcher (blocklist persistente).
    if !cfg.blocked_games.is_empty() {
        let blocked: std::collections::HashSet<&str> =
            cfg.blocked_games.iter().map(|s| s.as_str()).collect();
        games.retain(|g| !blocked.contains(g.id.as_str()));
    }

    eprintln!("[cardynox] Total: {} juegos en el carrusel.", games.len());
    games
}

/// Lanza un juego. Soporta Steam (steam://rungameid) y .exe directo.
#[tauri::command]
fn launch_game(app_id: Option<String>, name: String, launch_path: Option<String>, install_dir: Option<String>, args: Option<String>) -> Result<(), String> {
    // 1) Juego con .exe directo (registry o folder). `args` (solo "Otros") se
    //    tokeniza respetando comillas y se añade al command line del .exe.
    if let Some(exe) = &launch_path {
        let mut cmd = std::process::Command::new(exe);
        if let Some(dir) = &install_dir {
            cmd.current_dir(dir);
        }
        if let Some(a) = &args {
            let tokens = split_args(a);
            if !tokens.is_empty() {
                cmd.args(tokens);
            }
        }
        cmd.spawn()
            .map_err(|e| format!("No se pudo lanzar \"{name}\": {e}"))?;
        return Ok(());
    }
    // 2) Juego de Steam (steam:// URI).
    if let Some(id) = &app_id {
        let uri = format!("steam://rungameid/{id}");
        std::process::Command::new("cmd")
            .args(["/C", "start", "", &uri])
            .spawn()
            .map_err(|e| format!("No se pudo abrir Steam para \"{name}\": {e}"))?;
        return Ok(());
    }
    Err(format!("\"{name}\": sin método de lanzamiento configurado."))
}

/// Minimiza la ventana principal.
#[tauri::command]
fn win_minimize(window: WebviewWindow) {
    let _ = window.minimize();
}

/// Oculta la ventana (el programa sigue vivo en la bandeja del sistema).
#[tauri::command]
fn win_hide(window: WebviewWindow) {
    let _ = window.hide();
}

// ---------------------------------------------------------------------------
// Ventana de detalles
// ---------------------------------------------------------------------------

/// Devuelve los detalles locales de un juego (ruta, tamaño, logros, tiempo).
#[tauri::command]
fn get_game_details(app_id: String) -> steam::GameDetails {
    steam::get_details(&app_id)
}

/// Abre el Explorador de Windows en la carpeta de instalación del juego.
#[tauri::command]
fn open_install_folder(path: String) -> Result<(), String> {
    use std::process::Command;
    // `cmd /C start "" <path>` abre Explorer en esa carpeta.
    Command::new("cmd")
        .args(["/C", "start", "", &path])
        .spawn()
        .map(|_| ())
        .map_err(|e| format!("No se pudo abrir la carpeta: {e}"))
}

/// Cambia el nombre visible de un juego. Persiste en config.
#[tauri::command]
fn rename_game(app_id: String, new_name: String, app: tauri::AppHandle) -> Result<(), String> {
    let name = new_name.trim().to_string();
    if name.is_empty() {
        return Err("El nombre no puede estar vacío".into());
    }
    let mut cfg = load_config(&app);
    cfg.renames.insert(app_id, name);
    save_config(&app, &cfg);
    Ok(())
}

/// Cambia el ejecutable al que apunta un juego no-Steam ("Otros").
/// Persiste en config. Valida que el archivo exista para no dejar un enlace roto.
#[tauri::command]
fn set_launch_exe(id: String, exe_path: String, app: tauri::AppHandle) -> Result<(), String> {
    let path = exe_path.trim().to_string();
    if path.is_empty() {
        return Err("La ruta del ejecutable no puede estar vacía".into());
    }
    if !std::path::Path::new(&path).is_file() {
        return Err(format!("No existe ese archivo: {path}"));
    }
    let mut cfg = load_config(&app);
    cfg.launch_overrides.insert(id, path);
    save_config(&app, &cfg);
    Ok(())
}

/// Abre un diálogo nativo de Windows para elegir el .exe de un juego "Otros".
/// Devuelve la ruta elegida o None si se cancela. Sin dependencias nuevas: reutiliza
/// PowerShell + System.Windows.Forms (mismo patrón que open_external con explorer).
#[tauri::command]
fn pick_exe() -> Result<Option<String>, String> {
    // Script de una sola línea: PowerShell + System.Windows.Forms abre un file dialog
    // nativo (filtro .exe por defecto). Devuelve la ruta por stdout; vacío si se cancela.
    let script = "[Console]::OutputEncoding=[System.Text.Encoding]::UTF8; Add-Type -AssemblyName System.Windows.Forms; $d=New-Object System.Windows.Forms.OpenFileDialog; $d.Filter='Ejecutables (*.exe)|*.exe|Todos los archivos (*.*)|*.*'; $d.Title='Elegir ejecutable'; if($d.ShowDialog() -eq [System.Windows.Forms.DialogResult]::OK){Write-Output $d.FileName}";
    let out = std::process::Command::new("powershell")
        .args(["-NoProfile", "-WindowStyle", "Hidden", "-Command", script])
        .output()
        .map_err(|e| format!("No se pudo abrir el selector: {e}"))?;
    let stdout = String::from_utf8_lossy(&out.stdout).trim().to_string();
    Ok(if stdout.is_empty() { None } else { Some(stdout) })
}

/// Guarda los argumentos de lanzamiento de un juego "Otros" (game.id → args).
/// Vacío → elimina la clave (quedaría sin args). Solo afecta al spawn del .exe.
#[tauri::command]
fn set_launch_args(id: String, args: String, app: tauri::AppHandle) -> Result<(), String> {
    let mut cfg = load_config(&app);
    let a = args.trim().to_string();
    if a.is_empty() {
        cfg.launch_args.remove(&id);
    } else {
        cfg.launch_args.insert(id, a);
    }
    save_config(&app, &cfg);
    Ok(())
}

/// Devuelve el mapa game.id → argumentos de lanzamiento guardados (para
/// prellenar el campo del overlay Detalles). Simétrico con get_custom_covers.
#[tauri::command]
fn get_launch_args(app: tauri::AppHandle) -> HashMap<String, String> {
    load_config(&app).launch_args
}

/// Agrega un juego manualmente (botón “Agregar” del gadget). Valida nombre y
/// que el .exe exista. El id es determinístico por ruta de exe: re-agregar el
/// mismo ejecutable actualiza el nombre en vez de duplicar. Devuelve el `Game`
/// resultante para que el frontend lo use (id) sin volver a escanear.
#[tauri::command]
fn add_game(name: String, exe: String, app: tauri::AppHandle) -> Result<Game, String> {
    let name = name.trim().to_string();
    let path = exe.trim().to_string();
    if name.is_empty() {
        return Err("El nombre no puede estar vacío".into());
    }
    if !std::path::Path::new(&path).is_file() {
        return Err(format!("No existe ese archivo: {path}"));
    }
    let id = format!("manual-{:08x}", stable_hash(&path));
    let install_dir = std::path::Path::new(&path)
        .parent()
        .map(|p| p.to_string_lossy().to_string());

    let mut cfg = load_config(&app);
    if let Some(existing) = cfg.manual_games.iter_mut().find(|g| g.id == id) {
        existing.name = name.clone(); // mismo exe → solo actualizar el nombre.
    } else {
        cfg.manual_games.push(ManualGame {
            id: id.clone(),
            name: name.clone(),
            exe: path.clone(),
        });
    }
    save_config(&app, &cfg);
    eprintln!("[cardynox] Manual: +{} ({} agregados).", name, cfg.manual_games.len());

    Ok(Game {
        id,
        name,
        app_id: None,
        source: "manual".into(),
        launch_path: Some(path),
        install_dir,
    })
}

/// Quita un juego del launcher sumando su game.id a la lista negra persistente.
/// El escáner lo descarta en cada arranque; NO modifica el juego instalado.
#[tauri::command]
fn remove_game(id: String, app: tauri::AppHandle) -> Result<(), String> {
    let id = id.trim().to_string();
    if id.is_empty() {
        return Err("Id de juego vacío".into());
    }
    let mut cfg = load_config(&app);
    if !cfg.blocked_games.contains(&id) {
        cfg.blocked_games.push(id.clone());
        save_config(&app, &cfg);
        eprintln!("[cardynox] Blocklist: +{id} ({} ocultos).", cfg.blocked_games.len());
    }
    Ok(())
}

/// Divide un string de argumentos en tokens respetando comillas dobles.
/// `-name "John Doe"` → ["-name", "John Doe"]. Sin dependencias externas;
/// suficiente para args de juego (Rust vuelve a escapar al spawn si hace falta).
fn split_args(s: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut cur = String::new();
    let mut in_quotes = false;
    let mut started = false; // token en curso (incluye el caso de "" vacío)
    for c in s.chars() {
        match c {
            '"' => {
                started = true;
                in_quotes = !in_quotes;
            }
            ' ' | '\t' if !in_quotes => {
                if started {
                    tokens.push(std::mem::take(&mut cur));
                    started = false;
                }
            }
            ch => {
                cur.push(ch);
                started = true;
            }
        }
    }
    if started {
        tokens.push(cur);
    }
    tokens
}

// ---------------------------------------------------------------------------
// Config de gadget: opacidad, tamaño, renombres (persistida en app_config_dir)
// ---------------------------------------------------------------------------

/// Config de gadget guardada (opacidad + tamaño + renombres). El frontend
/// la consulta al iniciar para aplicar la opacidad (el tamaño se adapta solo por resize).
#[tauri::command]
fn get_gadget_config(app: tauri::AppHandle) -> GadgetConfig {
    load_config(&app)
}

// ---------------------------------------------------------------------------
// SteamGridDB — gestión de la API key (ventana de configuración desde bandeja)
// La lógica de red ya existe (resolve_covers / sgdb_cover_url, tier 3); acá solo
// se expone cómo ver/guardar/probar la key y cómo abrir la ventana que lo gestiona.
// ---------------------------------------------------------------------------

/// Key de SteamGridDB actual (para prellenar la ventana). `None` si está desactivada.
#[tauri::command]
fn get_sgdb_key(app: tauri::AppHandle) -> Option<String> {
    load_config(&app)
        .sgdb_key
        .clone()
        .filter(|k| !k.trim().is_empty())
}

/// Guarda (o quita, si llega vacía) la key de SteamGridDB. Persiste en config y
/// avisa al gadget para que reintente las portadas pendientes en vivo.
#[tauri::command]
fn set_sgdb_key(key: String, app: tauri::AppHandle) -> Result<(), String> {
    let k = key.trim().to_string();
    let mut cfg = load_config(&app);
    cfg.sgdb_key = if k.is_empty() { None } else { Some(k.clone()) };
    save_config(&app, &cfg);
    eprintln!(
        "[cardynox] SGDB {}.",
        if k.is_empty() { "desactivado" } else { "activado (key guardada localmente)" }
    );
    // El gadget escucha y re-dispara resolve_covers solo para lo que aún falta.
    let _ = app.emit("cardynox:sgdb-changed", ());
    Ok(())
}

/// Prueba la key contra la API reusando el endpoint autocomplete de SGDB.
/// 2xx = válida; 401/403 = inválida. Totalmente defensivo.
#[tauri::command]
async fn check_sgdb_key(key: String) -> Result<String, String> {
    let k = key.trim().to_string();
    if k.is_empty() {
        return Err("Pegá una API key primero".into());
    }
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(8))
        .user_agent("cardynox-launcher/0.1")
        .build()
        .map_err(|e| format!("Sin cliente HTTP: {e}"))?;
    // Término común: solo evalúo el estado HTTP para validar la key.
    let resp = client
        .get("https://www.steamgriddb.com/api/v2/search/autocomplete/elden")
        .header("Authorization", format!("Bearer {k}"))
        .send()
        .await
        .map_err(|e| format!("Sin conexión a SteamGridDB: {e}"))?;
    match resp.status().as_u16() {
        200..=299 => Ok("Key válida: conexión con SteamGridDB OK.".into()),
        401 | 403 => Err("SteamGridDB rechazó la key. Revisala en tu perfil.".into()),
        code => Err(format!("Respuesta inesperada de SteamGridDB (HTTP {code}).")),
    }
}

/// Abre una URL externa (http/https) en el navegador predeterminado del sistema.
/// Lo usa la ventana de configuración (enlaces a SteamGridDB). Cero deps: en
/// Windows, `explorer.exe "<url>"` delega la apertura al navegador por defecto.
#[tauri::command]
fn open_external(url: String) -> Result<(), String> {
    let u = url.trim();
    if !u.starts_with("http://") && !u.starts_with("https://") {
        return Err("Solo se permiten enlaces http/https".into());
    }
    std::process::Command::new("explorer")
        .arg(u)
        .spawn()
        .map_err(|e| format!("No se pudo abrir el navegador: {e}"))?;
    Ok(())
}

/// Abre (o enfoca, si ya está abierta) la ventana de configuración de SGDB.
fn show_sgdb_window(app: &tauri::AppHandle) -> tauri::Result<()> {
    if let Some(w) = app.get_webview_window("sgdb") {
        let _ = w.show();
        let _ = w.unminimize();
        let _ = w.set_focus();
        return Ok(());
    }
    let window = WebviewWindowBuilder::new(
        app,
        "sgdb",
        WebviewUrl::App("sgdb.html".into()),
    )
    .title("SteamGridDB · Cardynox")
    .center()
    .build()?;
    let _ = window.set_size(LogicalSize::new(540.0, 720.0));
    let _ = window.set_focus();
    Ok(())
}

// ---------------------------------------------------------------------------
// Portadas personalizadas (imagen/GIF local) — persistidas en covers/
// ---------------------------------------------------------------------------

/// Guarda una portada personalizada para un juego.
/// `data_url` es un data URL base64 (ej: "data:image/png;base64,iVBOR...").
/// Devuelve la ruta absoluta donde se guardó el archivo.
#[tauri::command]
fn set_custom_cover(app_id: String, data_url: String, app: tauri::AppHandle) -> Result<String, String> {
    use base64::Engine;

    // Validar formato del data URL.
    let (mime_part, b64_payload) = data_url
        .split_once(',')
        .ok_or("Data URL inválido: falta la coma")?;
    if !mime_part.starts_with("data:") {
        return Err("El string no es un data URL".into());
    }

    // Extraer MIME type (lo que está entre "data:" y ";").
    let mime = mime_part
        .strip_prefix("data:")
        .and_then(|s| s.split(';').next())
        .unwrap_or("image/png");

    // Determinar extensión según el MIME.
    let ext = match mime {
        "image/png" => "png",
        "image/jpeg" | "image/jpg" => "jpg",
        "image/gif" => "gif",
        "image/webp" => "webp",
        _ => return Err(format!("Formato no soportado: {mime}")),
    };

    // Decodificar base64.
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(b64_payload)
        .map_err(|e| format!("Error decodificando base64: {e}"))?;

    // Crear directorio de portadas.
    let covers_dir = app
        .path()
        .app_config_dir()
        .map_err(|e| format!("No se pudo obtener app_config_dir: {e}"))?
        .join("covers");
    std::fs::create_dir_all(&covers_dir)
        .map_err(|e| format!("No se pudo crear carpeta covers: {e}"))?;

    let filename = format!("{app_id}.{ext}");
    let file_path = covers_dir.join(&filename);
    std::fs::write(&file_path, &bytes)
        .map_err(|e| format!("No se pudo escribir la portada: {e}"))?;

    // Persistir en config.
    let mut cfg = load_config(&app);
    cfg.covers.insert(app_id.clone(), filename.clone());
    save_config(&app, &cfg);

    Ok(file_path.to_string_lossy().to_string())
}

/// Devuelve el mapa appid → ruta absoluta de todas las portadas personalizadas guardadas.
#[tauri::command]
fn get_custom_covers(app: tauri::AppHandle) -> HashMap<String, String> {
    let cfg = load_config(&app);
    let covers_dir = match app.path().app_config_dir() {
        Ok(dir) => dir.join("covers"),
        Err(_) => return HashMap::new(),
    };
    cfg.covers
        .into_iter()
        .filter_map(|(appid, filename)| {
            let path = covers_dir.join(&filename);
            if path.exists() {
                Some((appid, path.to_string_lossy().to_string()))
            } else {
                None
            }
        })
        .collect()
}

/// Elimina la portada personalizada de un juego (borra archivo + config).
#[tauri::command]
fn remove_custom_cover(app_id: String, app: tauri::AppHandle) -> Result<(), String> {
    let cfg = load_config(&app);
    let Some(filename) = cfg.covers.get(&app_id) else {
        return Err("No hay portada personalizada para ese juego".into());
    };
    let covers_dir = app
        .path()
        .app_config_dir()
        .map_err(|e| format!("Error: {e}"))?
        .join("covers");
    let file_path = covers_dir.join(filename);
    if file_path.exists() {
        std::fs::remove_file(&file_path)
            .map_err(|e| format!("No se pudo borrar el archivo: {e}"))?;
    }
    let mut cfg2 = load_config(&app);
    cfg2.covers.remove(&app_id);
    save_config(&app, &cfg2);
    Ok(())
}

// ---------------------------------------------------------------------------
// Posición de portada en Detalles (paneo) — persistida en config como 0..1
// ---------------------------------------------------------------------------

/// Todas las posiciones guardadas. Simétrico con get_custom_covers.
#[tauri::command]
fn get_cover_pans(app: tauri::AppHandle) -> HashMap<String, CoverPan> {
    load_config(&app).cover_pans
}

/// Guarda la posición de paneo de un juego. Llegan normalizados (0..1) pero se
/// acotan (clamp) y se rechaza NaN para que la JSON nunca se corrompa por valores
/// raros venidos del JS.
#[tauri::command]
fn save_cover_pan(app_id: String, x: f64, y: f64, app: tauri::AppHandle) -> Result<(), String> {
    if !x.is_finite() || !y.is_finite() {
        return Err("Offset inválido".into());
    }
    let pan = CoverPan { x: x.clamp(0.0, 1.0), y: y.clamp(0.0, 1.0) };
    let mut cfg = load_config(&app);
    cfg.cover_pans.insert(app_id, pan);
    save_config(&app, &cfg);
    Ok(())
}

/// Borra la posición guardada (ej: al cambiar/quitar portada → la imagen
/// nueva arranca centrada). Sin error si no existe.
#[tauri::command]
fn clear_cover_pan(app_id: String, app: tauri::AppHandle) -> Result<(), String> {
    let mut cfg = load_config(&app);
    if cfg.cover_pans.remove(&app_id).is_some() {
        save_config(&app, &cfg);
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Config de gadget: opacidad y tamaño (persistida en app_config_dir)
// ---------------------------------------------------------------------------

const OPACITY_STEPS: [u8; 5] = [20, 40, 60, 80, 100];
const SIZE_NAMES: [&str; 5] = ["Mini", "Pequeño", "Mediano", "Grande", "Enorme"];
/// (ancho, alto) en píxeles lógicos por tamaño — proporcionales al diseño
/// base de 620×370. "Mediano" (índice 2) coincide con tauri.conf.json.
const SIZES: [(f64, f64); 5] = [
    (470.0, 280.0),
    (545.0, 325.0),
    (620.0, 370.0),
    (710.0, 424.0),
    (810.0, 483.0),
];

/// Posición custom de la portada en el overlay Detalles (paneo por arrastre):
/// clave del juego → offset NORMALIZADO (0..1) de la ventana visible dentro del
/// overflow total. Normalizada (no en px) para que siga teniendo sentido si el
/// usuario cambia el tamaño del gadget o el ratio de la imagen cambia.
#[derive(Serialize, Deserialize, Clone, Copy)]
struct CoverPan {
    x: f64,
    y: f64,
}

#[derive(Serialize, Deserialize, Clone)]
struct GadgetConfig {
    /// Opacidad total del gadget en porcentaje (uno de OPACITY_STEPS).
    opacity: u8,
    /// Índice de tamaño dentro de SIZES (0..=4).
    size: u8,
    /// Mapa app_id → nombre visible personalizado.
    #[serde(default)]
    renames: HashMap<String, String>,
    /// Portadas personalizadas: app_id → filename en covers/.
    #[serde(default)]
    covers: HashMap<String, String>,
    /// Directorios raíz a escanear para juegos no-Steam (folder scan).
    /// Ej: ["D:\\Juegos", "E:\\Games"] — se buscan subcarpetas con .exe.
    #[serde(default)]
    scan_roots: Vec<String>,
    /// Portadas auto-resueltas para juegos no-Steam: game.id → cover.
    /// Caché offline; el 2º arranque no toca red. Backward compatible.
    #[serde(default)]
    auto_covers: HashMap<String, AutoCover>,
    /// Key de SteamGridDB (opcional). Vacía/None = tier SGDB desactivado.
    #[serde(default)]
    sgdb_key: Option<String>,
    /// Ejecutables personalizados para juegos no-Steam: game.id → ruta del .exe.
    /// Permite cambiar a qué .exe apunta un juego "Otros" (persistido).
    #[serde(default)]
    launch_overrides: HashMap<String, String>,
    /// Argumentos de lanzamiento para juegos no-Steam "Otros": game.id → string.
    /// Se pasan al .exe al ejecutarlo (persistido). Backward compatible.
    #[serde(default)]
    launch_args: HashMap<String, String>,
    /// Juegos que el usuario sacó del launcher: lista de game.id. El escáner
    /// los descarta en cada arranque para no re-agregarlos. NO toca lo instalado.
    #[serde(default)]
    blocked_games: Vec<String>,
    /// Juegos agregados manualmente desde el botón “Agregar” del gadget.
    /// Cada uno es `{ id, name, exe }`; persiste entre sesiones.
    #[serde(default)]
    manual_games: Vec<ManualGame>,
    /// Posición de portada en Detalles (paneo): clave del juego → offset 0..1.
    #[serde(default)]
    cover_pans: HashMap<String, CoverPan>,
}

impl Default for GadgetConfig {
    fn default() -> Self {
        Self {
            opacity: 100,
            size: 2,
            renames: HashMap::new(),
            covers: HashMap::new(),
            scan_roots: vec![],
            auto_covers: HashMap::new(),
            sgdb_key: None,
            launch_overrides: HashMap::new(),
            launch_args: HashMap::new(),
            blocked_games: vec![],
            manual_games: vec![],
            cover_pans: HashMap::new(),
        }
    }
}


/// Normaliza un nombre para comparación flexible:
/// lowercase, sin puntuación, espacios colapsados.
fn normalize_name(name: &str) -> String {
    name.to_lowercase()
        .chars()
        .filter(|c| c.is_alphanumeric() || *c == ' ')
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn config_path(app: &tauri::AppHandle) -> Option<PathBuf> {
    app.path()
        .app_config_dir()
        .ok()
        .map(|dir| dir.join("cardynox.json"))
}

fn load_config(app: &tauri::AppHandle) -> GadgetConfig {
    let mut cfg = config_path(app)
        .and_then(|p| std::fs::read_to_string(p).ok())
        .and_then(|s| serde_json::from_str::<GadgetConfig>(&s).ok())
        .unwrap_or_default();
    // Sanitiza por si el archivo está corrupto o fue editado a mano.
    if !OPACITY_STEPS.contains(&cfg.opacity) {
        cfg.opacity = 100;
    }
    if cfg.size as usize >= SIZES.len() {
        cfg.size = 2;
    }
    cfg
}

fn save_config(app: &tauri::AppHandle, cfg: &GadgetConfig) {
    let Some(p) = config_path(app) else {
        return;
    };
    if let Some(dir) = p.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    match serde_json::to_string(cfg) {
        Ok(json) => {
            if let Err(e) = std::fs::write(&p, json) {
                eprintln!("[cardynox] No se pudo guardar la config: {e}");
            }
        }
        Err(e) => eprintln!("[cardynox] No se pudo serializar la config: {e}"),
    }
}

// ---------------------------------------------------------------------------
// Portadas automáticas para juegos no-Steam (auto-covers)
// ---------------------------------------------------------------------------

/// Cover resuelto para un juego no-Steam. Tag interno `kind` → el frontend
// ve `{ kind: "steam", appId }` o `{ kind: "url", url }`.
#[derive(Serialize, Deserialize, Clone)]
#[serde(tag = "kind", rename_all = "camelCase")]
enum AutoCover {
    /// Resuelto a un AppId de Steam → el frontend usa el CDN con fallback
    /// vertical→horizontal (reutiliza artUrl).
    Steam { app_id: String },
    /// URL de imagen directa (SGDB u otra fuente) → se carga tal cual.
    Url { url: String },
}

/// Solicitud de portada. `app_id` opcional: si el juego YA es de Steam pero su
/// CDN falló, permite resolver la carátula vía SGDB por appid (más fiable que
/// buscar por nombre). Los juegos no-Steam lo dejan vacío.
#[derive(Deserialize)]
struct CoverRequest {
    id: String,
    name: String,
    #[serde(default)]
    app_id: Option<String>,
}

/// Coincidencia "suficiente" entre la consulta y un candidato: match exacto
/// normalizado o al menos un token (len≥3) compartido. Evita portadas erróneas
/// sin rechazar variaciones de nombre legítimas.
fn cover_name_ok(query: &str, candidate: &str) -> bool {
    let q = normalize_name(query);
    let c = normalize_name(candidate);
    if q.is_empty() || c.is_empty() {
        return false;
    }
    if q == c {
        return true;
    }
    let qt: Vec<&str> = q.split_whitespace().collect();
    let ct: Vec<&str> = c.split_whitespace().collect();
    qt.iter().any(|a| a.len() >= 3 && ct.contains(a))
}

/// Percent-encode mínimo para un segmento de URL (SGDB). Defensivo.
fn pct_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char);
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// Tier 2 — store search de Steam (endpoint público, SIN key). Devuelve el
/// AppId si hay una coincidencia confiable.
async fn store_search_appid(client: &reqwest::Client, name: &str) -> Option<String> {
    let resp = client
        .get("https://store.steampowered.com/api/storesearch/")
        .query(&[("term", name), ("f", "apps")])
        .send()
        .await
        .ok()?;
    if !resp.status().is_success() {
        eprintln!("[cardynox] storesearch \"{name}\": HTTP {}.", resp.status());
        return None;
    }
    #[derive(Deserialize)]
    struct SSItem {
        id: String,
        name: String,
    }
    #[derive(Deserialize)]
    struct SSResp {
        total: u32,
        items: Vec<SSItem>,
    }
    let body: SSResp = resp.json().await.ok()?;
    if body.total == 0 || body.items.is_empty() {
        return None;
    }
    // Prefiero match exacto normalizado; si no, el top del ranking que pase la prueba.
    if let Some(it) = body
        .items
        .iter()
        .find(|i| normalize_name(&i.name) == normalize_name(name))
    {
        return Some(it.id.clone());
    }
    let top = &body.items[0];
    if cover_name_ok(name, &top.name) {
        Some(top.id.clone())
    } else {
        eprintln!(
            "[cardynox] storesearch \"{name}\": top \"{}\" descartado (no coincide).",
            top.name
        );
        None
    }
}

/// Tier 3 — SteamGridDB (opcional, requiere key). Devuelve la URL de una
/// portada vertical. Totalmente defensivo: cualquier fallo → None.
async fn sgdb_cover_url(client: &reqwest::Client, key: &str, name: &str) -> Option<String> {
    // 1) Autocomplete → game_id.
    let search_url = format!(
        "https://www.steamgriddb.com/api/v2/search/autocomplete/{}",
        pct_encode(name)
    );
    let resp = client
        .get(&search_url)
        .header("Authorization", format!("Bearer {key}"))
        .send()
        .await
        .ok()?;
    if !resp.status().is_success() {
        eprintln!("[cardynox] sgdb search \"{name}\": HTTP {}.", resp.status());
        return None;
    }
    #[derive(Deserialize)]
    struct SgdbGame {
        id: u64,
        name: String,
    }
    // SGDB v2 devuelve {"success":bool,"data":[...]}, no un array directo.
    #[derive(Deserialize)]
    struct SgdbListResp {
        #[serde(default)]
        success: bool,
        #[serde(default)]
        data: Vec<SgdbGame>,
    }
    let list: SgdbListResp = resp.json().await.ok()?;
    if !list.success || list.data.is_empty() {
        return None;
    }
    let game_id = if let Some(g) = list
        .data
        .iter()
        .find(|g| normalize_name(&g.name) == normalize_name(name))
    {
        g.id
    } else if let Some(g) = list.data.iter().find(|g| cover_name_ok(name, &g.name)) {
        g.id
    } else {
        return None;
    };

    // 2) Grids verticales 600x900.
    let grids_url = format!(
        "https://www.steamgriddb.com/api/v2/grids/game/{game_id}?dimensions=600x900"
    );
    let resp = client
        .get(&grids_url)
        .header("Authorization", format!("Bearer {key}"))
        .send()
        .await
        .ok()?;
    if !resp.status().is_success() {
        return None;
    }
    // Cada grid tiene el campo `url` directo (cdn*.steamgriddb.com).
    #[derive(Deserialize)]
    struct SgdbGrid {
        #[serde(default)]
        style: String,
        #[serde(default)]
        nsfw: bool,
        #[serde(default)]
        humor: bool,
        url: String,
    }
    #[derive(Deserialize)]
    struct SgdbGridsResp {
        #[serde(default)]
        success: bool,
        #[serde(default)]
        data: Vec<SgdbGrid>,
    }
    let grids: SgdbGridsResp = resp.json().await.ok()?;
    if !grids.success {
        return None;
    }
    // Prefiero la carátula "material" (estándar); si no, la primera apta.
    grids
        .data
        .iter()
        .find(|g| g.style == "material" && !g.nsfw && !g.humor)
        .or_else(|| grids.data.iter().find(|g| !g.nsfw && !g.humor))
        .map(|g| g.url.clone())
        .filter(|u| !u.is_empty())
}

/// Tier 3b — SteamGridDB por AppId de Steam conocido (`grids/steam/{appid}`).
/// Se usa cuando un juego SÍ es de Steam pero su CDN vertical/horizontal falló
/// (p. ej. Metal Gear): el appid ya lo tenemos, así que es más fiable que buscar
/// por nombre. Mismo shape de respuesta que `grids/game/{game_id}`.
async fn sgdb_cover_by_steam_appid(
    client: &reqwest::Client,
    key: &str,
    app_id: &str,
) -> Option<String> {
    let grids_url = format!(
        "https://www.steamgriddb.com/api/v2/grids/steam/{app_id}?dimensions=600x900"
    );
    let resp = client
        .get(&grids_url)
        .header("Authorization", format!("Bearer {key}"))
        .send()
        .await
        .ok()?;
    if !resp.status().is_success() {
        eprintln!("[cardynox] sgdb steam/{app_id}: HTTP {}.", resp.status());
        return None;
    }
    #[derive(Deserialize)]
    struct SgdbGrid {
        #[serde(default)]
        style: String,
        #[serde(default)]
        nsfw: bool,
        #[serde(default)]
        humor: bool,
        url: String,
    }
    #[derive(Deserialize)]
    struct SgdbGridsResp {
        #[serde(default)]
        success: bool,
        #[serde(default)]
        data: Vec<SgdbGrid>,
    }
    let grids: SgdbGridsResp = resp.json().await.ok()?;
    if !grids.success {
        return None;
    }
    // Prefiero la carátula "material" (estándar); si no, la primera apta.
    grids
        .data
        .iter()
        .find(|g| g.style == "material" && !g.nsfw && !g.humor)
        .or_else(|| grids.data.iter().find(|g| !g.nsfw && !g.humor))
        .map(|g| g.url.clone())
        .filter(|u| !u.is_empty())
}

/// Resuelve portadas para juegos no-Steam. Estrategia liviana por tiers:
///   1) Caché local (config.auto_covers) → sin red.
///   2) Store search de Steam (público, sin key) → AppId → CDN.
///   3) SteamGridDB (opcional, solo si hay sgdb_key) → URL directa.
/// Devuelve un mapa game.id → cover para todos los resueltos (caché + nuevos).
#[tauri::command]
async fn resolve_covers(
    app: tauri::AppHandle,
    requests: Vec<CoverRequest>,
) -> HashMap<String, AutoCover> {
    let mut cfg = load_config(&app);
    let client = match reqwest::Client::builder()
        .timeout(Duration::from_secs(8))
        .user_agent("cardynox-launcher/0.1")
        .build()
    {
        Ok(c) => c,
        Err(e) => {
            eprintln!("[cardynox] resolve_covers: sin cliente HTTP ({e}); solo caché.");
            return cfg.auto_covers.clone();
        }
    };

    let sgdb_key = cfg.sgdb_key.clone().filter(|k| !k.trim().is_empty());
    let mut resolved_count = 0;

    for req in &requests {
        // (1) Caché: si ya está resuelto, no toco la red.
        if cfg.auto_covers.contains_key(&req.id) {
            continue;
        }
        // (2) Steam con AppId conocido (su CDN falló): SGDB por appid, lo más fiable.
        //     Sin key no hay vía útil — storesearch re-resolvería el mismo appid cuyo
        //     arte ya falló, así que no tiene sentido intentarlo.
        if let Some(aid) = req.app_id.as_deref() {
            if let Some(key) = &sgdb_key {
                if let Some(url) = sgdb_cover_by_steam_appid(&client, key, aid).await {
                    cfg.auto_covers.insert(req.id.clone(), AutoCover::Url { url });
                    resolved_count += 1;
                }
            }
            continue;
        }
        // (3) Store search de Steam (no-Steam, sin appid).
        if let Some(app_id) = store_search_appid(&client, &req.name).await {
            cfg.auto_covers.insert(req.id.clone(), AutoCover::Steam { app_id });
            resolved_count += 1;
            continue;
        }
        // (4) SGDB opcional por nombre.
        if let Some(key) = &sgdb_key {
            if let Some(url) = sgdb_cover_url(&client, key, &req.name).await {
                cfg.auto_covers.insert(req.id.clone(), AutoCover::Url { url });
                resolved_count += 1;
            }
        }
    }

    if resolved_count > 0 {
        save_config(&app, &cfg);
        eprintln!("[cardynox] resolve_covers: {resolved_count} portadas nuevas resueltas.");
    }

    // Mapa completo para los ids solicitados (caché + nuevos).
    let mut out = HashMap::new();
    for req in &requests {
        if let Some(c) = cfg.auto_covers.get(&req.id) {
            out.insert(req.id.clone(), c.clone());
        }
    }
    out
}

/// Referencias a los ítems chequeables para actualizar sus checkmarks.
struct MenuState {
    opacity: Vec<CheckMenuItem<Wry>>,
    size: Vec<CheckMenuItem<Wry>>,
}

// ---------------------------------------------------------------------------
// Aplicación de opciones (desde el menú de la bandeja)
// ---------------------------------------------------------------------------

fn apply_opacity(app: &tauri::AppHandle, state: &MenuState, idx: usize) {
    let mut cfg = load_config(app);
    cfg.opacity = OPACITY_STEPS[idx];
    save_config(app, &cfg);
    // El frontend escucha este evento y aplica la opacidad al body.
    let _ = app.emit("cardynox:set-opacity", cfg.opacity);
    for (i, item) in state.opacity.iter().enumerate() {
        let _ = item.set_checked(i == idx);
    }
}

fn apply_size(app: &tauri::AppHandle, state: &MenuState, idx: usize) {
    let mut cfg = load_config(app);
    cfg.size = idx as u8;
    save_config(app, &cfg);
    if let Some(window) = app.get_webview_window("main") {
        let (w, h) = SIZES[idx];
        // Sin re-centrado: la ventana crece/se reduce desde su posición actual.
        let _ = window.set_size(LogicalSize::new(w, h));
    }
    for (i, item) in state.size.iter().enumerate() {
        let _ = item.set_checked(i == idx);
    }
}

/// Redimensiona la ventana y la recentra en su monitor (fallback: solo
/// redimensiona). El frontend escala la UI sola vía el evento de resize.
fn resize_centered(window: &WebviewWindow, w: f64, h: f64) {
    if let (Ok(Some(monitor)), Ok(scale)) = (window.current_monitor(), window.scale_factor()) {
        if scale > 0.0 {
            let mx = monitor.position().x as f64 / scale;
            let my = monitor.position().y as f64 / scale;
            let mw = monitor.size().width as f64 / scale;
            let mh = monitor.size().height as f64 / scale;
            let _ = window.set_size(LogicalSize::new(w, h));
            let _ = window.set_position(LogicalPosition::new(
                mx + (mw - w) / 2.0,
                my + (mh - h) / 2.0,
            ));
            return;
        }
    }
    let _ = window.set_size(LogicalSize::new(w, h));
}

// ---------------------------------------------------------------------------
// Bandeja del sistema
// ---------------------------------------------------------------------------

fn setup_tray(app: &tauri::AppHandle) -> tauri::Result<()> {
    let icon = app
        .default_window_icon()
        .cloned()
        .expect("el ícono por defecto debe existir (bundle.icon en tauri.conf.json)");

    let cfg = load_config(app);

    // Submenú Opacidad: 20% / 40% / 60% / 80% / 100%.
    let opacity_items: Vec<CheckMenuItem<Wry>> = OPACITY_STEPS
        .iter()
        .enumerate()
        .map(|(i, pct)| {
            CheckMenuItemBuilder::with_id(format!("op-{i}"), format!("{pct}%"))
                .checked(*pct == cfg.opacity)
                .build(app)
        })
        .collect::<tauri::Result<Vec<_>>>()?;

    // Submenú Tamaño: Mini / Pequeño / Mediano / Grande / Enorme.
    let size_items: Vec<CheckMenuItem<Wry>> = (0..SIZES.len())
        .map(|i| {
            CheckMenuItemBuilder::with_id(format!("sz-{i}"), SIZE_NAMES[i])
                .checked(i as u8 == cfg.size)
                .build(app)
        })
        .collect::<tauri::Result<Vec<_>>>()?;

    let op_submenu = Submenu::with_items(
        app,
        "Opacidad",
        true,
        &opacity_items
            .iter()
            .map(|i| i as &dyn IsMenuItem<Wry>)
            .collect::<Vec<_>>(),
    )?;
    let size_submenu = Submenu::with_items(
        app,
        "Tamaño",
        true,
        &size_items
            .iter()
            .map(|i| i as &dyn IsMenuItem<Wry>)
            .collect::<Vec<_>>(),
    )?;

    let quit = MenuItem::with_id(app, "quit", "Cerrar", true, None::<&str>)?;
    // Opción para configurar la key de SteamGridDB (abre una ventana aparte).
    let sgdb_item = MenuItem::with_id(
        app,
        "sgdb",
        "Activar SGDB",
        true,
        None::<&str>,
    )?;
    let menu = Menu::with_items(
        app,
        &[
            &op_submenu as &dyn IsMenuItem<Wry>,
            &size_submenu as &dyn IsMenuItem<Wry>,
            &sgdb_item as &dyn IsMenuItem<Wry>,
            &quit as &dyn IsMenuItem<Wry>,
        ],
    )?;

    // Estado compartido: el handler lo usa para actualizar los checkmarks.
    app.manage(MenuState {
        opacity: opacity_items,
        size: size_items,
    });

    TrayIconBuilder::new()
        .tooltip("Cardynox Launcher")
        .icon(icon)
        .menu(&menu)
        // Clic izquierdo = mostrar/ocultar ventana; clic derecho = menú.
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| {
            let state = app.state::<MenuState>();
            match event.id.as_ref() {
                "quit" => {
                    eprintln!("[cardynox] Cerrando desde la bandeja.");
                    app.exit(0);
                }
                id if id.starts_with("op-") => {
                    if let Ok(idx) = id[3..].parse::<usize>() {
                        if idx < OPACITY_STEPS.len() {
                            apply_opacity(app, &state, idx);
                        }
                    }
                }
                id if id.starts_with("sz-") => {
                    if let Ok(idx) = id[3..].parse::<usize>() {
                        if idx < SIZES.len() {
                            apply_size(app, &state, idx);
                        }
                    }
                }
                "sgdb" => {
                    if let Err(e) = show_sgdb_window(app) {
                        eprintln!("[cardynox] No se pudo abrir la ventana de SGDB: {e}");
                    }
                }
                other => eprintln!("[cardynox] Ítem de menú sin manejar: {other:?}"),
            }
        })
        .on_tray_icon_event(|tray, event| {
            let is_left_release = matches!(
                event,
                TrayIconEvent::Click {
                    button: MouseButton::Left,
                    button_state: MouseButtonState::Up,
                    ..
                } | TrayIconEvent::DoubleClick {
                    button: MouseButton::Left,
                    ..
                }
            );
            if !is_left_release {
                return;
            }
            let app = tray.app_handle();
            let Some(window) = app.get_webview_window("main") else {
                return;
            };
            // Toggle: visible -> ocultar; oculta/minimizada -> mostrar.
            let visible = window.is_visible().unwrap_or(false);
            let minimized = window.is_minimized().unwrap_or(false);
            if visible && !minimized {
                let _ = window.hide();
            } else {
                let _ = window.show();
                let _ = window.unminimize();
                let _ = window.set_focus();
                // La config siempreOnBottom:tao intercepta WM_WINDOWPOSCHANGING y
                // devuelve la ventana al fondo del Z-order en cada cambio.
            }
        })
        .build(app)?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Punto de entrada
// ---------------------------------------------------------------------------

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![
            list_games,
            launch_game,
            win_minimize,
            win_hide,
            get_gadget_config,
            get_sgdb_key,
            set_sgdb_key,
            check_sgdb_key,
            open_external,
            set_custom_cover,
            get_custom_covers,
            remove_custom_cover,
            get_cover_pans,
            save_cover_pan,
            clear_cover_pan,
            get_game_details,
            open_install_folder,
            rename_game,
            set_launch_exe,
            pick_exe,
            set_launch_args,
            get_launch_args,
            remove_game,
            add_game,
            resolve_covers
        ])
        .setup(|app| {
            setup_tray(app.handle())?;
            // Arranca con visible:false en la config y se muestra acá; el
            // siempreOnBottom de la config (tao) la mantiene debajo de todas
            // las ventanas normales en cada cambio de Z-order.
            if let Some(window) = app.get_webview_window("main") {
                // Aplica el tamaño persistido antes de mostrar (la ventana se
                // crea con 620×370; si la config pide otro tamaño, lo ajusta).
                let cfg = load_config(app.handle());
                if cfg.size != GadgetConfig::default().size {
                    let (w, h) = SIZES[cfg.size as usize];
                    resize_centered(&window, w, h);
                }
                let _ = window.show();
            }
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error al iniciar Cardynox Launcher");
}
