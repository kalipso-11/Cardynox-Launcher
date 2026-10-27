/* ============================================================
   Cardynox Launcher — lógica de la interfaz
   Sin dependencias: HTML + CSS + JS vanilla.
   Habla con Rust vía window.__TAURI__.core.invoke; si se abre
   en un browser normal (dev visual) usa datos demo locales.
   ============================================================ */

"use strict";

const tauri = window.__TAURI__;
const invoke = tauri ? tauri.core.invoke : null;

// Lista de respaldo para previsualizar la UI fuera de Tauri.
const BROWSER_DEMO = [
  { id: "steam-1245620", name: "Elden Ring", appId: "1245620", source: "demo" },
  { id: "steam-1593500", name: "God of War", appId: "1593500", source: "demo" },
  { id: "steam-292030", name: "The Witcher 3: Wild Hunt", appId: "292030", source: "demo" },
  { id: "steam-1174180", name: "Horizon Zero Dawn", appId: "1174180", source: "demo" },
  { id: "steam-1086940", name: "Baldur's Gate 3", appId: "1086940", source: "demo" },
];

// ---------------------------------------------------------------------------
// Estado
// ---------------------------------------------------------------------------

let games = [];        // lista completa (Rust o demo)
let filtered = [];     // juegos visibles tras el filtro del buscador
let active = 0;        // índice dentro de `filtered`
const cardEls = new Map(); // game.id -> <article class="card">
const argsMap = {};            // game.id -> args de lanzamiento (solo "Otros")
let wheelLockUntil = 0;
let statusTimer = null;
let sortMode = "platform"; // "name" (todos por nombre) | "platform" (agrupado, dentro alfab.)
const hiddenPlatforms = new Set(); // sourceLabels ocultos (Steam, Epic, Otros, ...)
const customCovers = new Map(); // clave (appId|id) → URL (portada personalizada local)
const coverPans = new Map();    // clave (appId|id) → {x,y} offset normalizado 0..1 del paneo de portada (Detalles)
const autoCovers = new Map();  // game.id → {kind:"steam",appId} | {kind:"url",url}
const steamCoverTried = new Set(); // game.id de Steam cuyo CDN falló y ya pidió SGDB (anti-loop)

// Overlay “Agregar juego”: estado transitorio (solo se persiste en Rust al confirmar).
let addExe = null;          // ruta del .exe elegido, o null.
let addCoverDataUrl = null; // data URL de la portada elegida (aún no guardada), o null.

const stage = document.getElementById("stage");
const searchInput = document.getElementById("search");
const activeName = document.getElementById("active-name");
const statusEl = document.getElementById("status");

// ---------------------------------------------------------------------------
// Utilidades
// ---------------------------------------------------------------------------

function artUrl(appId, attempt) {
  const base = "https://cdn.cloudflare.steamstatic.com/steam/apps/" + appId;
  return attempt === 0
    ? base + "/library_600x900.jpg" // vertical 600x900 (como el cliente de Steam)
    : base + "/capsule_616x353.jpg"; // fallback horizontal
}

/** Arte por defecto de un juego (sin portada custom): Steam CDN vertical,
    o null si no hay nada (usar iniciales). */
function defaultArt(game) {
  if (game.appId) return artUrl(game.appId, 0);
  const a = autoCovers.get(game.id);
  if (a) return a.kind === "steam" ? artUrl(a.appId, 0) : a.url;
  return null;
}

function initials(name) {
  return name
    .split(/\s+/)
    .filter((w) => /\p{L}/u.test(w))
    .slice(0, 2)
    .map((w) => [...w][0].toUpperCase())
    .join("");
}

/** Etiqueta legible de origen: Steam / Epic / GOG / … / Otros.
    "Folder", "registry" y cualquier fuente no reconocida → "Otros". */
function sourceLabel(game) {
  const s = (game.source || "").toLowerCase();
  if (s === "steam" || s === "demo") return "Steam";
  if (!game.source || s === "folder" || s === "registry") return "Otros";
  if (s === "manual") return "Manual"; // agregado a mano desde el botón “Agregar”.
  return game.source; // Epic, GOG, EA, Ubisoft, Battle.net, Rockstar, Heroic, Xbox
}

/** ¿El juego es editable en Detalles (cambiar .exe + argumentos)?
    "Otros" y "Manual": sí (el usuario controla el ejecutable). El resto no. */
function isEditable(game) {
  const s = sourceLabel(game);
  return s === "Otros" || s === "Manual";
}

function baseStatus() {
  // Desglose por origen: Steam, Epic, GOG, Battle.net, Otros, etc.
  const counts = {};
  for (const g of games) {
    const key = sourceLabel(g);
    counts[key] = (counts[key] || 0) + 1;
  }
  const parts = Object.entries(counts).map(([src, n]) => `${n} ${src}`);
  return `${games.length} juegos · ${parts.join(" · ")}`;
}

function setStatus(text, flash) {
  if (statusTimer) clearTimeout(statusTimer);
  statusEl.textContent = text;
  statusEl.classList.toggle("flash", !!flash);
  if (flash) statusTimer = setTimeout(() => setStatus(baseStatus(), false), 2600);
}

// ---------------------------------------------------------------------------
// Config de gadget (opacidad / tamaño) — persistida en Rust
// ---------------------------------------------------------------------------

const BASE_W = 620; // ancho del diseño base (tamaño "Mediano")

/** Escala toda la UI con CSS zoom: el ancho de la ventana manda.
    En WebView2 (Chromium) `zoom` escala layout, transform y fuentes
    sin tocar ninguna regla de CSS. */
function applyUiScale() {
  const u = Math.max(0.3, window.innerWidth / BASE_W);
  document.documentElement.style.zoom = u.toFixed(4);
  // El título es un canal de debug/test del gadget (frameless + skipTaskbar:
  // el usuario no lo ve en ninguna parte).
  document.title = `Cardynox Launcher | z=${u.toFixed(2)}`;
}

/** Opacidad del gadget en porcentaje (pasos 20..100 desde la bandeja).
    Solo afecta fondos/bordes/sombras via --fade; el texto y las imágenes
    de portada se mantienen a opacidad completa. */
function applyOpacity(pct) {
  const v = Math.min(100, Math.max(5, Number(pct) || 100));
  document.documentElement.style.setProperty('--fade', String(v / 100));
}

/** Colores de acento del gadget (nombre → triple RGB), mismos 10 que el
    submenú Color de la bandeja. Solo se setea --accent-rgb; todo lo demás
    (--accent, bordes, sombras, glows) deriva de esa variable en styles.css.
    Violeta = valor original del diseño. */
const ACCENTS = {
  violeta: "124, 134, 255",
  rojo: "239, 68, 68",
  verde: "52, 211, 153",
  azul: "96, 165, 250",
  blanco: "255, 255, 255",
  gris: "156, 163, 175",
  naranja: "251, 146, 60",
  amarillo: "250, 204, 21",
  cian: "34, 211, 238",
  rosa: "244, 114, 182",
};

/** Cambia el acento del gadget en vivo. Nombre desconocido → no-op. */
function applyAccent(name) {
  const rgb = ACCENTS[name];
  if (!rgb) return;
  document.documentElement.style.setProperty("--accent-rgb", rgb);
}

/** Elementos de UI ocultables desde la bandeja (submenú "Ocultar UI").
    MISMO ORDEN que HIDDEN_UI_NAMES en lib.rs. display:none se restaura con "".
    [0] manija de arrastre, [1] botón filtro, [2] botones inferiores,
    [3] conteo de juegos (footer). */
const HIDEABLE = ["#drag-handle", "#btn-filter", ".actions", "#status"];

/** Muestra u oculta un elemento del gadget. idx fuera de rango → no-op. */
function applyUiHidden(idx, hidden) {
  const el = document.querySelector(HIDEABLE[idx]);
  if (!el) return;
  el.style.display = hidden ? "none" : "";
}

/** Aplica el estado persistido de todos los elementos (arranque). */
function applyAllUiHidden(list) {
  (list || []).forEach((hidden, idx) => applyUiHidden(idx, !!hidden));
}

/** Carga la config persistida (opacidad) y se suscribe a cambios en vivo.
    El tamaño no necesita IPC: llega solo por el evento de resize de la ventana. */
function initGadgetConfig() {
  if (!invoke) return;
  window.addEventListener("resize", applyUiScale);
  invoke("get_gadget_config")
    .then((cfg) => {
      applyOpacity(cfg.opacity);
      applyAccent(cfg.accent);
      applyAllUiHidden(cfg.hidden_ui);
    })
    .catch((err) => console.error("get_gadget_config falló:", err));
  tauri.event.listen("cardynox:set-opacity", (e) => applyOpacity(e.payload));
  tauri.event.listen("cardynox:set-accent", (e) => applyAccent(e.payload));
  // Payload del toggle de la bandeja: [índice, oculto].
  tauri.event.listen("cardynox:set-ui-hidden", (e) => applyUiHidden(e.payload[0], e.payload[1]));
}

/** Resuelve portadas automáticas para juegos no-Steam que aún no la tienen.
    Corre en segundo plano: primero muestra iniciales y repinta al resolver. */
function requestAutoCovers() {
  if (!invoke) return;
  const needs = games.filter(
    (g) => !g.appId && !customCovers.has(g.appId || g.id) && !autoCovers.has(g.id)
  );
  if (!needs.length) return;
  invoke("resolve_covers", { requests: needs.map((g) => ({ id: g.id, name: g.name })) })
    .then((res) => {
      for (const [id, c] of Object.entries(res)) autoCovers.set(id, c);
      layout(); // re-evalúa el arte de las tarjetas cercanas
      if (detailsGame) showDetails(); // refresca el overlay si está abierto
    })
    .catch((err) => console.warn("resolve_covers falló:", err));
}

// Fallback para juegos de Steam cuyo CDN vertical/horizontal falló (p. ej. Metal Gear):
// pide a SGDB la carátula por appid y pinta la tarjeta en vivo (no toca lazyLoadArt,
// que ya tiene artQueued=1). Una sola vez por juego por sesión (steamCoverTried).
async function requestSteamFallbackCover(game, card) {
  if (!invoke) { card.classList.add("no-art"); return; }
  try {
    const res = await invoke("resolve_covers", {
      requests: [{ id: game.id, name: game.name, app_id: game.appId }],
    });
    const cover = res && res[game.id];
    const img = card.querySelector("img.art");
    if (cover && cover.kind === "url" && img) {
      autoCovers.set(game.id, cover); // coherente en re-layout y en Detalles
      delete card.dataset.artTried;   // no re-intentar CDN
      card.classList.remove("no-art");
      img.src = cover.url;
      img.onerror = () => { card.classList.add("no-art"); img.removeAttribute("src"); };
      if (detailsGame && detailsGame.id === game.id) showDetails();
    } else {
      card.classList.add("no-art");
      if (img) img.removeAttribute("src");
    }
  } catch (err) {
    card.classList.add("no-art");
    console.warn("fallback SGDB falló para", game.name, ":", err);
  }
}

// ---------------------------------------------------------------------------
// Construcción de tarjetas
// ---------------------------------------------------------------------------

function buildCards() {
  stage.querySelectorAll(".card").forEach((el) => el.remove());
  cardEls.clear();

  for (const game of games) {
    const card = document.createElement("article");
    card.className = "card";
    card.dataset.id = game.id;
    card.title = game.name;

    const img = document.createElement("img");
    img.className = "art";
    img.alt = "";
    img.draggable = false;
    // Paneo guardado en Detalles → mismo recorte en la tarjeta (mismo ratio 0.7
    // en ambos marcos: la fracción 0..1 mapea a la misma vista en las dos cajas).
    img.addEventListener("load", () => { applyCardPan(game, card); });
    img.addEventListener("error", () => {
      const appId = game.appId || card.dataset.steamAppId;
      if (appId && !card.dataset.artTried) {
        card.dataset.artTried = "1";
        img.src = artUrl(appId, 1); // fallback horizontal (Steam / auto-Steam)
      } else if (game.appId && !steamCoverTried.has(game.id)) {
        // CDN vertical Y horizontal fallaron (p. ej. Metal Gear): intentar SGDB por appid.
        steamCoverTried.add(game.id);
        requestSteamFallbackCover(game, card);
      } else {
        card.classList.add("no-art");
        img.removeAttribute("src");
      }
    });

    const sheen = document.createElement("div");
    sheen.className = "sheen";

    const fallback = document.createElement("div");
    fallback.className = "fallback";
    fallback.innerHTML = `<span>${initials(game.name) || "?"}</span>`;

    // Etiqueta chica de origen (Steam/Epic/…/Otros) al pie de la portada.
    const srcEl = document.createElement("div");
    srcEl.className = "card-src";
    srcEl.textContent = sourceLabel(game);

    card.append(img, fallback, sheen, srcEl);

    // Sin arte posible (no-Steam y sin portada custom) → iniciales directas.
    const artKey = game.appId || game.id;
    if (!game.appId && !customCovers.has(artKey)) {
      card.classList.add("no-art");
    }

    card.addEventListener("click", () => {
      const idx = filtered.findIndex((g) => g.id === game.id);
      if (idx !== -1 && idx !== active) setActive(idx);
      else if (game.id === filtered[active]?.id) playActive();
    });

    stage.prepend(card); // las tarjetas van debajo de #active-name y .actions
    cardEls.set(game.id, card);
  }
}

/** Carga el arte de forma perezosa: solo tarjetas cercanas al centro.
    Jerarquía: portada personalizada → CDN Steam → iniciales. */
function lazyLoadArt(game, card, dist) {
  if (dist > 2 || card.dataset.artQueued) return;
  const key = game.appId || game.id; // clave para portadas personalizadas
  const img = card.querySelector("img.art");

  // Jerarquía: custom → auto (storesearch/SGDB) → Steam(appId) → iniciales.
  let src = null;
  let steamAppId = null; // habilita el fallback vertical→horizontal en <img error>
  if (customCovers.has(key)) {
    src = customCovers.get(key);
  } else {
    const a = autoCovers.get(game.id);
    if (a) {
      if (a.kind === "steam") { steamAppId = a.appId; src = artUrl(a.appId, 0); }
      else { src = a.url; }
    } else if (game.appId) {
      steamAppId = game.appId;
      src = artUrl(game.appId, 0);
    }
  }

  // Sin arte aún (no-Steam sin auto-cover resuelto) → iniciales, reintentable.
  if (!src) { card.classList.add("no-art"); return; }

  card.dataset.artQueued = "1";
  card.dataset.steamAppId = steamAppId || "";
  card.classList.remove("no-art");
  img.src = src;
  // Imagen ya cacheada: el evento load puede no re-dispararse → aplicar ya.
  if (img.complete && img.naturalWidth) applyCardPan(game, card);
}

// ---------------------------------------------------------------------------
// Carrusel
// ---------------------------------------------------------------------------

/** Distancia con signo y envoltura (wraparound) entre dos índices. */
function wrappedOffset(idx) {
  const n = filtered.length;
  let off = ((idx - active) % n + n) % n;
  if (off > n / 2) off -= n;
  return off;
}

function layout() {
  const visible = new Set();

  filtered.forEach((game, idx) => {
    visible.add(game.id);
    const card = cardEls.get(game.id);
    if (!card) return;

    const off = wrappedOffset(idx);
    const a = Math.abs(off);
    const sign = Math.sign(off);

    let x = 0, scale = 1, rot = 0, opacity = 1, z = 30;
    if (a === 0) {
      card.classList.add("active");
    } else {
      card.classList.remove("active");
      // Offsets calibrados para la ventana de 620px
      // (ancho útil del stage ≈ 596px, tarjeta activa de 126px).
      if (a === 1) { x = sign * 146; scale = 0.82; rot = -sign * 18; opacity = 0.75; z = 20; }
      else if (a === 2) { x = sign * 240; scale = 0.66; rot = -sign * 26; opacity = 0.38; z = 10; }
      else { x = sign * 305; scale = 0.6; rot = -sign * 30; opacity = 0; z = 1; }
      card.style.pointerEvents = a > 2 ? "none" : "auto";
    }

    card.style.transform = `translateX(${x}px) scale(calc(${scale} * var(--hs, 1))) rotateY(${rot}deg)`;
    card.style.opacity = String(opacity);
    card.style.zIndex = String(z);

    lazyLoadArt(game, card, a);
    // Si el juego tiene paneo guardado, re-aplicarlo: el offset en px depende del
    // tamaño del marco (cambia con el preset de tamaño y al re-escalar la UI).
    if (coverPans.has(game.appId || game.id)) applyCardPan(game, card);
  });

  // Oculta tarjetas fuera del filtro actual.
  for (const [id, card] of cardEls) {
    if (!visible.has(id)) {
      card.classList.remove("active");
      card.style.opacity = "0";
      card.style.pointerEvents = "none";
      card.style.zIndex = "0";
    }
  }

  const current = filtered[active];
  activeName.textContent = current ? current.name : "Sin resultados";
  document.getElementById("btn-play").disabled = !current;
}

function setActive(idx) {
  if (!filtered.length) return;
  active = ((idx % filtered.length) + filtered.length) % filtered.length; // wrap
  layout();
  updateSearchStatus();
}

function step(dir) {
  const now = Date.now();
  if (now < wheelLockUntil) return; // evita saltos en cadena con la rueda
  wheelLockUntil = now + 260;
  setActive(active + dir);
}

// ---------------------------------------------------------------------------
// Buscador
// ---------------------------------------------------------------------------

// Orden fijo de plataformas para el modo "por plataforma": Steam primero,
// luego las conocidas, y Manual / "Otros" al final. El resto entra alfab.
const KNOWN_PLATFORMS = ["Epic", "GOG", "EA", "Ubisoft", "Battle.net", "Rockstar", "Heroic", "Xbox"];

function platformRank(label) {
  if (label === "Steam") return 0;
  const i = KNOWN_PLATFORMS.indexOf(label);
  if (i !== -1) return 1 + i;
  if (label === "Manual") return 98;
  return 99; // "Otros" y fuentes no reconocidas van al final.
}

function byName(a, b) {
  return a.name.localeCompare(b.name, "es", { sensitivity: "base" });
}

// Lista visible: aplica plataformas ocultas + texto del buscador, y ordena
// segun `sortMode` ("name" | "platform").
function computeFiltered() {
  const q = searchInput.value.trim().toLowerCase();
  let list = games.filter((g) => !hiddenPlatforms.has(sourceLabel(g)));
  if (q) list = list.filter((g) => g.name.toLowerCase().includes(q));
  if (sortMode === "name") return [...list].sort(byName);
  return [...list].sort((a, b) => {
    const ra = platformRank(sourceLabel(a));
    const rb = platformRank(sourceLabel(b));
    return ra !== rb ? ra - rb : byName(a, b);
  });
}

function applyFilter() {
  const prevId = filtered[active] ? filtered[active].id : null; // juego centrado antes
  filtered = computeFiltered();
  if (prevId) {
    const idx = filtered.findIndex((g) => g.id === prevId);
    active = idx >= 0 ? idx : Math.min(active, filtered.length - 1);
  } else {
    active = 0;
  }
  if (active < 0 || active >= filtered.length) active = Math.max(0, filtered.length - 1);
  layout();
  updateSearchStatus();
}

// Genera los checkboxes de plataforma segun las presentes en `games` (+conteo).
// Se re-renderiza al abrir el panel y tras cada cambio de checkbox.
function renderPlatformChecks() {
  const host = document.getElementById("fp-platforms");
  if (!host) return;
  const counts = {};
  for (const g of games) {
    const l = sourceLabel(g);
    counts[l] = (counts[l] || 0) + 1;
  }
  const labels = Object.keys(counts).sort((a, b) =>
    platformRank(a) - platformRank(b) || a.localeCompare(b, "es", { sensitivity: "base" })
  );
  host.innerHTML = "";
  for (const l of labels) {
    const row = document.createElement("label");
    row.className = "fp-check";
    const cb = document.createElement("input");
    cb.type = "checkbox";
    cb.checked = !hiddenPlatforms.has(l);
    cb.addEventListener("change", () => {
      if (cb.checked) hiddenPlatforms.delete(l); else hiddenPlatforms.add(l);
      applyFilter();
    });
    const name = document.createElement("span");
    name.textContent = l;
    const cnt = document.createElement("span");
    cnt.className = "fp-count";
    cnt.textContent = String(counts[l]);
    row.append(cb, name, cnt);
    host.appendChild(row);
  }
}

function openFilterPanel() {
  renderPlatformChecks();
  document.querySelectorAll('#filter-panel input[name="sortmode"]').forEach((r) => {
    r.checked = (r.value === sortMode);
  });
  const p = document.getElementById("filter-panel");
  // El boton ya no esta en el borde derecho: alinear el panel debajo del boton.
  const rect = document.getElementById("btn-filter").getBoundingClientRect();
  p.style.right = Math.max(6, window.innerWidth - rect.right) + "px";
  p.hidden = false;
  document.getElementById("btn-filter").classList.add("active");
}

function closeFilterPanel() {
  const p = document.getElementById("filter-panel");
  if (!p || p.hidden) return;
  p.hidden = true;
  document.getElementById("btn-filter").classList.remove("active");
}

function updateSearchStatus() {
  const q = searchInput.value.trim();
  if (!q) { setStatus(baseStatus(), false); return; }
  setStatus(`${filtered.length} de ${games.length} coinciden con “${q}”`, filtered.length > 0);
}

// ---------------------------------------------------------------------------
// Eventos
// ---------------------------------------------------------------------------

function bindEvents() {
  // Rueda del mouse: avanza/retrocede el carrusel.
  // La rueda se binda a `document` (no a `stage`) para que el carrusel navegue
  // incluso con el cursor sobre el boton "Jugar" centrado, que es hijo de #app.
  document.addEventListener("wheel", (e) => {
    // Con un overlay abierto la rueda no debe mover el carrusel.
    if (!document.getElementById("details-overlay").hidden ||
        !document.getElementById("add-overlay").hidden) return;
    e.preventDefault();
    const dir = Math.abs(e.deltaY) > 8 ? Math.sign(e.deltaY) : 0;
    if (dir !== 0) step(dir);
  }, { passive: false });

  // Buscador en vivo.
  searchInput.addEventListener("input", applyFilter);

  // Icono de filtro: abre/cierra el panel de orden + plataformas.
  const btnFilter = document.getElementById("btn-filter");
  btnFilter.addEventListener("click", (e) => {
    e.stopPropagation();
    const p = document.getElementById("filter-panel");
    if (p.hidden) openFilterPanel(); else closeFilterPanel();
  });

  // Click fuera cierra el panel (si no se hizo en el icono o en el panel).
  document.addEventListener("mousedown", (e) => {
    const p = document.getElementById("filter-panel");
    if (!p || p.hidden) return;
    if (p.contains(e.target) || btnFilter.contains(e.target)) return;
    closeFilterPanel();
  });

  // Modo de orden (radios): cambia sortMode y reordena el carrusel.
  document.querySelectorAll('#filter-panel input[name="sortmode"]').forEach((r) => {
    r.addEventListener("change", () => {
      if (!r.checked) return;
      sortMode = r.value;
      applyFilter();
    });
  });

  // Esc cierra el panel de filtro (si esta abierto).
  window.addEventListener("keydown", (e) => {
    if (e.key === "Escape" && !document.getElementById("filter-panel").hidden) closeFilterPanel();
  });

  // Teclado: flechas (estilo consola), Enter para jugar, Esc cierra detalles.
  window.addEventListener("keydown", (e) => {
    if (e.key === "Escape" && !document.getElementById("details-overlay").hidden) {
      hideDetails(); return;
    }
    if (e.key === "ArrowRight") { step(1); e.preventDefault(); }
    else if (e.key === "ArrowLeft") { step(-1); e.preventDefault(); }
    else if (e.key === "Enter" && document.activeElement !== searchInput &&
             !document.getElementById("details-overlay").hidden === false) playActive();
  });

  // Botones de acción.
  document.getElementById("btn-play").addEventListener("click", playActive);
  document.getElementById("btn-details").addEventListener("click", showDetails);
  // Botón “Agregar”: abre el overlay vacío para cargar un juego manual.
  document.getElementById("btn-add")?.addEventListener("click", openAddOverlay);

  // Overlay “Agregar”: cerrar (× o Cancelar) y confirmar.
  document.getElementById("btn-close-add")?.addEventListener("click", closeAddOverlay);
  document.getElementById("add-cancel")?.addEventListener("click", closeAddOverlay);
  document.getElementById("add-confirm")?.addEventListener("click", confirmAddGame);
  // Elegir .exe y portada.
  document.getElementById("add-pick-exe")?.addEventListener("click", pickAddExe);
  document.getElementById("add-change-cover")?.addEventListener("click", () => {
    document.getElementById("add-cover-file")?.click();
  });
  document.getElementById("add-reset-cover")?.addEventListener("click", resetAddCover);
  document.getElementById("add-name")?.addEventListener("input", updateAddConfirm);
  document.getElementById("add-cover-file")?.addEventListener("change", onAddCoverFile);
    // Opciones (binding anterior, retirado). próximamente (por ahora está en el menú de la bandeja).", true));

  // Overlay de detalles: cerrar.
  document.getElementById("btn-close-details").addEventListener("click", hideDetails);

  // Overlay: renombrar.
  document.getElementById("dt-rename-btn").addEventListener("click", saveRename);
  document.getElementById("dt-name").addEventListener("keydown", (e) => {
    if (e.key === "Enter") saveRename();
  });

  // Overlay: argumentos de lanzamiento (solo "Otros"); Enter guarda.
  document.getElementById("dt-args-save").addEventListener("click", saveLaunchArgs);
  document.getElementById("dt-args").addEventListener("keydown", (e) => {
    if (e.key === "Enter") saveLaunchArgs();
  });

  // Overlay: quitar del launcher con confirmación inline.
  document.getElementById("dt-remove-game").addEventListener("click", showRemoveConfirm);
  document.getElementById("dt-remove-no").addEventListener("click", cancelRemoveConfirm);
  document.getElementById("dt-remove-yes").addEventListener("click", confirmRemoveGame);

  // Overlay: abrir carpeta. La caja unificada puede mostrar la ruta completa
  // con el .exe; "Abrir carpeta" debe abrir SOLO el directorio padre.
  document.getElementById("dt-open-folder").addEventListener("click", async () => {
    let folder = (document.getElementById("dt-path").textContent || "").trim();
    if (!folder || folder === "—" || folder === "Cargando…") return;
    if (/\.exe$/i.test(folder)) {
      const i = folder.lastIndexOf("\\");
      folder = i > 0 ? folder.slice(0, i) : folder;
    }
    if (!folder) return;
    try { await invoke("open_install_folder", { path: folder }); }
    catch (err) { setStatus(String(err), true); }
  });

  // Overlay: juegos "Otros" → cambiar la ruta de juego (elegir el .exe).
  document.getElementById("dt-change-path").addEventListener("click", pickAndSetExe);

  // Overlay: cambiar portada personalizada.
  document.getElementById("dt-change-cover")?.addEventListener("click", () => {
    document.getElementById("dt-cover-file")?.click();
  });

  document.getElementById("dt-cover-file")?.addEventListener("change", async (e) => {
    const file = e.target.files?.[0];
    if (!file || !detailsGame) return;
    if (!/image\/(png|jpeg|gif|webp)/.test(file.type)) {
      setStatus("Formato no soportado. Usá PNG, JPG, GIF o WebP.", true);
      e.target.value = "";
      return;
    }
    try {
      const dataUrl = await new Promise((resolve, reject) => {
        const reader = new FileReader();
        reader.onload = () => resolve(reader.result);
        reader.onerror = reject;
        reader.readAsDataURL(file);
      });
      const coverKey = detailsGame.appId || detailsGame.id;
      // Guardar en Rust.
      await invoke("set_custom_cover", { appId: coverKey, dataUrl });
      // Actualizar estado local.
      customCovers.set(coverKey, dataUrl);
      // Portada nueva: el paneo anterior ya no aplica → arranca centrada.
      coverPans.delete(coverKey);
      invoke("clear_cover_pan", { appId: coverKey }).catch(() => {});
      // Refrescar la vista de detalles y la tarjeta activa.
      const img = document.getElementById("dt-art-img");
      img.src = dataUrl;
      img.style.display = "";
      requestAnimationFrame(syncDetailsArt);   // re-centra la portada nueva
      document.getElementById("dt-art-fallback").classList.add("hidden");
      document.getElementById("dt-reset-cover").hidden = false;
      // También actualizar la tarjeta en el carrusel.
      const card = cardEls.get(detailsGame.id);
      if (card) {
        const cardImg = card.querySelector("img.art");
        cardImg.src = dataUrl;
        card.classList.remove("no-art");
        applyCardPan(detailsGame, card); // el paneo viejo ya se borró → centro
      }
      setStatus(`Portada actualizada para “${detailsGame.name}”.`, true);
    } catch (err) {
      setStatus(`Error al guardar portada: ${err}`, true);
    }
    e.target.value = "";
  });

  // Overlay: quitar portada personalizada.
  document.getElementById("dt-reset-cover")?.addEventListener("click", async () => {
    if (!detailsGame) return;
    const coverKey = detailsGame.appId || detailsGame.id;
    try {
      await invoke("remove_custom_cover", { appId: coverKey });
      customCovers.delete(coverKey);
      // Volviendo al arte original → el paneo guardado deja de aplicar.
      coverPans.delete(coverKey);
      invoke("clear_cover_pan", { appId: coverKey }).catch(() => {});
      document.getElementById("dt-reset-cover").hidden = true;
      // Garantía: la opción de volver a elegir portada queda siempre visible.
      document.getElementById("dt-change-cover").hidden = false;

      // Volver al arte por defecto del tipo de juego (Steam/exe/iniciales).
      const fallback = defaultArt(detailsGame);

      const img = document.getElementById("dt-art-img");
      if (fallback) {
        img.style.display = "";
        img.src = fallback;
      } else {
        img.style.display = "none"; // volver a las iniciales
      }
      requestAnimationFrame(syncDetailsArt);   // re-centra si vuelve a haber imagen

      const card = cardEls.get(detailsGame.id);
      if (card) {
        const cardImg = card.querySelector("img.art");
        if (fallback) {
          // Steam: volver al arte del CDN.
          card.dataset.artTried = "";
          card.classList.remove("no-art");
          cardImg.src = fallback;
          applyCardPan(detailsGame, card); // el paneo viejo ya se borró → centro
        } else {
          // No-Steam: volver a las iniciales.
          cardImg.removeAttribute("src");
          card.classList.add("no-art");
        }
      }
      setStatus(`Portada personalizada eliminada.`, true);
    } catch (err) {
      setStatus(`Error al quitar portada: ${err}`, true);
    }
  });

  // La ventana es de tamaño fijo: Tauri mapea el doble clic sobre una
  // zona data-tauri-drag-region a "maximizar" (se vería la skin a
  // pantalla completa). Bloqueamos ese mousedown en fase de captura,
  // antes de que lo vea el listener interno de Tauri.
  document.addEventListener(
    "mousedown",
    (e) => {
      if (e.button === 0 && e.detail === 2) e.stopImmediatePropagation();
    },
    true
  );

}

async function playActive() {
  const game = filtered[active];
  if (!game) return;
  if (!invoke) {
    setStatus("(browser) Lanzamiento solo disponible dentro de Tauri.", true);
    return;
  }
  try {
    await invoke("launch_game", {
      appId: game.appId ?? null,
      name: game.name,
      launchPath: game.launchPath ?? null,
      installDir: game.installDir ?? null,
      args: argsMap[game.id] || null,
    });
    setStatus(`Lanzando “${game.name}”…`, true);
  } catch (err) {
    setStatus(String(err), true);
  }
}

// ---------------------------------------------------------------------------
// Overlay de detalles (dentro del mismo gadget)
// ---------------------------------------------------------------------------

let detailsGame = null; // referencia al juego cuyos datos se muestran

function showDetails() {
  const game = filtered[active];
  if (!game || !invoke) return;
  detailsGame = game;

  // Clave para renombres y portadas: appId (Steam) o id (resto).
  const gameKey = game.appId || game.id;

  // Rellenar título y nombre editable.
  document.getElementById("dt-title").textContent = game.name;
  document.getElementById("dt-name").value = game.name;

  // Resetear estado de los campos.
  document.getElementById("dt-path").textContent = "Cargando…";
  document.getElementById("dt-open-folder").disabled = true;
  document.getElementById("dt-size-field").hidden = true;
  document.getElementById("dt-steam-section").hidden = true;
  document.getElementById("dt-no-data").hidden = true;
  document.getElementById("dt-change-path").hidden = true;

  // Argumentos de lanzamiento: juegos "Otros" y "Manual" (controlan el .exe).
  const argsField = document.getElementById("dt-args-field");
  const canEditArgs = isEditable(game);
  argsField.hidden = !canEditArgs;
  if (canEditArgs) {
    document.getElementById("dt-args").value = argsMap[game.id] || "";
  }

  // Resetear estado de confirmación de "Quitar del launcher".
  document.getElementById("dt-remove-game").hidden = false;
  document.getElementById("dt-remove-confirm").hidden = true;

  // Origen / fuente.
  const srcEl = document.getElementById("dt-source");
  if (srcEl) {
    srcEl.textContent = sourceLabel(game);
    srcEl.hidden = false;
  }

  // Portada.
  const img = document.getElementById("dt-art-img");
  const fb = document.getElementById("dt-art-fallback");
  img.style.display = "";
  fb.classList.remove("hidden");
  document.getElementById("dt-art-initials").textContent = initials(game.name);

  const auto = autoCovers.get(game.id);
  // Steam: prefiere el arte oficial del CDN; la portada SGDB resuelta queda como
  // fallback solo si ese CDN falla. No-Steam usa directo el cover automatico.
  const autoUrl = auto ? (auto.kind === "steam" ? artUrl(auto.appId, 0) : auto.url) : null;
  if (gameKey && customCovers.has(gameKey)) {
    // Portada personalizada: usar directamente.
    img.src = customCovers.get(gameKey);
    img.onload = () => fb.classList.add("hidden");
    img.onerror = () => { img.style.display = "none"; fb.classList.remove("hidden"); };
  } else if (game.appId) {
    // Steam: arte oficial del CDN (vertical, luego horizontal) con fallback SGDB.
    img.src = artUrl(game.appId, 0);
    img.onload = () => fb.classList.add("hidden");
    img.onerror = () => {
      img.src = artUrl(game.appId, 1);
      img.onerror = () => {
        if (autoUrl) {
          img.src = autoUrl; // portada SGDB resuelta
          img.onload = () => fb.classList.add("hidden");
          img.onerror = () => { img.style.display = "none"; fb.classList.remove("hidden"); };
        } else {
          img.style.display = "none";
          fb.classList.remove("hidden");
        }
      };
    };
  } else if (autoUrl) {
    // No-Steam: portada auto (SGDB o Steam resuelto por storesearch).
    img.src = autoUrl;
    img.onload = () => fb.classList.add("hidden");
    img.onerror = () => {
      if (auto && auto.kind === "steam") {
        img.src = artUrl(auto.appId, 1);
        img.onerror = () => { img.style.display = "none"; fb.classList.remove("hidden"); };
      } else {
        img.style.display = "none";
        fb.classList.remove("hidden");
      }
    };
  } else {
    // No-Steam sin portada resuelta: iniciales.
    img.style.display = "none";
    img.removeAttribute("src");
    fb.classList.remove("hidden");
  }

  // Botón "Quitar portada": visible solo si hay una personalizada.
  const resetBtn = document.getElementById("dt-reset-cover");
  if (resetBtn) {
    resetBtn.hidden = !(gameKey && customCovers.has(gameKey));
  }

  // Mostrar overlay.
  document.getElementById("details-overlay").hidden = false;
  // Re-centra la portada ya que el marco tiene tamaño real (box.clientWidth > 0).
  requestAnimationFrame(syncDetailsArt);

  if (game.appId) {
    // Steam: cargar detalles ricos desde Rust.
    invoke("get_game_details", { appId: game.appId })
      .then(fillDetails)
      .catch(() => {
        document.getElementById("dt-path").textContent = "—";
        document.getElementById("dt-no-data").hidden = false;
      });
  } else {
    // No-Steam: caja unificada con la ruta completa (con .exe si existe).
    const pathEl = document.getElementById("dt-path");
    const folderBtn = document.getElementById("dt-open-folder");
    const changeBtn = document.getElementById("dt-change-path");

    let fullPath = game.launchPath || game.installDir || "";
    if (fullPath) {
      pathEl.textContent = fullPath;
      folderBtn.disabled = false;
    } else {
      pathEl.textContent = "—";
    }

    // Botón "Cambiar ruta de juego": solo para juegos "Otros" y "Manual".
    changeBtn.hidden = !isEditable(game);

    // No hay sección Steam ni tamaño en disco para juegos no-Steam.
    document.getElementById("dt-steam-section").hidden = true;
    document.getElementById("dt-size-field").hidden = true;
  }
}

function fillDetails(d) {
  const pathEl = document.getElementById("dt-path");
  const folderBtn = document.getElementById("dt-open-folder");
  const sizeField = document.getElementById("dt-size-field");
  const steamSection = document.getElementById("dt-steam-section");
  const noData = document.getElementById("dt-no-data");

  // Ruta.
  if (d.installPath) {
    pathEl.textContent = d.installPath;
    folderBtn.disabled = false;
  } else {
    pathEl.textContent = "—";
  }

  // Tamaño.
  if (d.diskSizeBytes != null) {
    document.getElementById("dt-size").textContent = formatBytes(d.diskSizeBytes);
    sizeField.hidden = false;
  }

  // Steam section.
  const hasSteam = d.achievementsUnlocked != null || d.hoursPlayed != null || d.lastPlayed != null;
  if (hasSteam) {
    steamSection.hidden = false;
    if (d.achievementsUnlocked != null) {
      document.getElementById("dt-row-ach").hidden = false;
      document.getElementById("dt-ach").textContent = String(d.achievementsUnlocked);
    }
    if (d.hoursPlayed != null) {
      document.getElementById("dt-row-time").hidden = false;
      document.getElementById("dt-time").textContent = formatHours(d.hoursPlayed);
    }
    if (d.lastPlayed != null) {
      document.getElementById("dt-row-last").hidden = false;
      document.getElementById("dt-last").textContent = formatDate(d.lastPlayed);
    }
  }

  // Si no hay NADA, mostrar mensaje.
  if (!d.installPath && d.diskSizeBytes == null && !hasSteam) {
    noData.hidden = false;
  }
}

function hideDetails() {
  document.getElementById("details-overlay").hidden = true;
  detailsGame = null;
}

async function saveRename() {
  if (!detailsGame || !invoke) return;
  const newName = document.getElementById("dt-name").value.trim();
  if (!newName || newName === detailsGame.name) return;
  try {
    // Clave: appId para Steam, id para el resto.
    const key = detailsGame.appId || detailsGame.id;
    await invoke("rename_game", { appId: key, newName });
    // Actualizar nombre en la lista local y en la UI.
    const g = games.find((g) => g.id === detailsGame.id);
    if (g) g.name = newName;
    detailsGame.name = newName;
    document.getElementById("dt-title").textContent = newName;
    // Feedback visual.
    const btn = document.getElementById("dt-rename-btn");
    btn.classList.add("saved");
    setTimeout(() => btn.classList.remove("saved"), 1200);
  } catch (err) {
    setStatus(`Error renombrando: ${err}`, true);
  }
}

// Guarda los argumentos de lanzamiento de un juego "Otros" (persistidos en Rust).
async function saveLaunchArgs() {
  if (!detailsGame || !invoke) return;
  const val = document.getElementById("dt-args").value.trim();
  try {
    await invoke("set_launch_args", { id: detailsGame.id, args: val });
    if (val) argsMap[detailsGame.id] = val; else delete argsMap[detailsGame.id];
    setStatus(`Argumentos guardados para “${detailsGame.name}”.`, true);
    const btn = document.getElementById("dt-args-save");
    btn.classList.add("saved");
    setTimeout(() => btn.classList.remove("saved"), 1200);
  } catch (err) {
    setStatus(`Error al guardar argumentos: ${err}`, true);
  }
}

// Muestra la confirmación inline antes de quitar el juego del launcher.
function showRemoveConfirm() {
  if (!detailsGame) return;
  const text = document.getElementById("dt-remove-confirm-text");
  text.textContent = `¿Seguro que querés quitar “${detailsGame.name}” del launcher? No se desinstala: solo deja de aparecer.`;
  document.getElementById("dt-remove-game").hidden = true;
  document.getElementById("dt-remove-confirm").hidden = false;
}

function cancelRemoveConfirm() {
  document.getElementById("dt-remove-game").hidden = false;
  document.getElementById("dt-remove-confirm").hidden = true;
}

// Confirma la baja: persiste en la lista negra (Rust) y lo saca de la UI ahora.
async function confirmRemoveGame() {
  if (!detailsGame || !invoke) return;
  const id = detailsGame.id;
  const name = detailsGame.name;
  try {
    await invoke("remove_game", { id });
    games = games.filter((g) => g.id !== id);
    filtered = filtered.filter((g) => g.id !== id);
    if (active >= filtered.length) active = Math.max(0, filtered.length - 1);
    hideDetails();
    buildCards();
    layout();
    setStatus(`“${name}” quitado del launcher.`, true);
  } catch (err) {
    setStatus(`Error al quitar el juego: ${err}`, true);
  }
}

// Aplica una nueva ruta de juego a un juego "Otros": persiste el override y
// refleja el cambio en la caja unificada de ruta del overlay.
async function applyExe(exe) {
  if (!detailsGame || !invoke) return;
  exe = (exe || "").trim();
  if (!exe || exe === (detailsGame.launchPath || "")) return;
  try {
    // El override se guarda por game.id (los no-Steam nunca tienen appId).
    await invoke("set_launch_exe", { id: detailsGame.id, exePath: exe });
    const g = games.find((g) => g.id === detailsGame.id);
    if (g) g.launchPath = exe;
    detailsGame.launchPath = exe;
    // Caja unificada: la ruta completa (con el nuevo .exe) queda a la vista.
    document.getElementById("dt-path").textContent = exe;
    document.getElementById("dt-open-folder").disabled = false;
    setStatus(`Ruta actualizada: “${detailsGame.name}”.`, true);
    const btn = document.getElementById("dt-change-path");
    btn.classList.add("saved");
    setTimeout(() => btn.classList.remove("saved"), 1200);
  } catch (err) {
    setStatus(`Error al cambiar la ruta: ${err}`, true);
  }
}

// Abre el selector nativo de Windows y aplica la elección (solo juegos "Otros").
async function pickAndSetExe() {
  if (!detailsGame || !invoke) return;
  try {
    const exe = await invoke("pick_exe"); // ruta, o null si se canceló.
    if (exe) await applyExe(exe);
  } catch (err) {
    setStatus(`No se pudo abrir el selector: ${err}`, true);
  }
}

// Formateo de datos para detalles.
function formatBytes(bytes) {
  const units = ["B", "KB", "MB", "GB"];
  let i = 0, val = bytes;
  while (val >= 1024 && i < units.length - 1) { val /= 1024; i++; }
  return `${val.toFixed(i > 0 ? 1 : 0)} ${units[i]}`;
}

function formatHours(h) {
  if (h < 1) return `${Math.round(h * 60)} min`;
  return `${h.toFixed(1)} h`;
}

function formatDate(epoch) {
  const d = new Date(epoch * 1000);
  return d.toLocaleDateString("es-AR", { year: "numeric", month: "short", day: "numeric" });
}

// ---------------------------------------------------------------------------
// Overlay “Agregar juego” (manual): portada, .exe y nombre visible. Solo se
// persiste en Rust al pulsar “Agregar”.
// ---------------------------------------------------------------------------

function openAddOverlay() {
  if (!invoke) return; // en preview de browser no hay backend.
  // Reset completo del estado transitorio y de los campos.
  addExe = null;
  addCoverDataUrl = null;
  document.getElementById("add-name").value = "";
  document.getElementById("add-path").textContent = "No seleccionado";
  const img = document.getElementById("add-art-img");
  img.style.display = "none";
  img.removeAttribute("src");
  document.getElementById("add-art-fallback").classList.remove("hidden");
  document.getElementById("add-art-initials").textContent = "\uFF0B";
  document.getElementById("add-reset-cover").hidden = true;
  document.getElementById("add-confirm").disabled = true;

  // Muestra el overlay y enfoca el nombre.
  document.getElementById("add-overlay").hidden = false;
  setTimeout(() => document.getElementById("add-name").focus(), 30);
}

function closeAddOverlay() {
  addExe = null;
  addCoverDataUrl = null;
  document.getElementById("add-overlay").hidden = true;
}

// Habilita “Agregar” solo si hay .exe elegido y el nombre no está vacío.
function updateAddConfirm() {
  const name = document.getElementById("add-name").value.trim();
  document.getElementById("add-confirm").disabled = !(name && addExe);
}

// Aplica un .exe al overlay: muestra la ruta y, si el nombre está vacío, lo
// autocompleta con el basename del ejecutable (sin la extensión).
function setAddExe(exe) {
  exe = (exe || "").trim();
  if (!exe) return;
  addExe = exe;
  document.getElementById("add-path").textContent = exe;

  const nameEl = document.getElementById("add-name");
  if (!nameEl.value.trim()) {
    const base = exe.replace(/\\/g, "/").split("/").pop() || "";
    nameEl.value = base.replace(/\.[^./\\]+$/, ""); // quita la última extensión
  }
  updateAddConfirm();
}

// Abre el selector nativo de Windows y aplica la elección al overlay.
async function pickAddExe() {
  if (!invoke) return;
  try {
    const exe = await invoke("pick_exe"); // ruta, o null si se canceló.
    if (exe) setAddExe(exe);
  } catch (err) {
    setStatus(`No se pudo abrir el selector: ${err}`, true);
  }
}

// Carga la portada elegida en memoria (data URL) y la previsualiza.
function onAddCoverFile(e) {
  const file = e.target.files?.[0];
  if (!file) return;
  if (!/image\/(png|jpeg|gif|webp)/.test(file.type)) {
    setStatus("Formato no soportado. Usá PNG, JPG, GIF o WebP.", true);
    e.target.value = "";
    return;
  }
  const reader = new FileReader();
  reader.onload = () => {
    addCoverDataUrl = reader.result;
    const img = document.getElementById("add-art-img");
    img.src = addCoverDataUrl;
    img.style.display = "";
    document.getElementById("add-art-fallback").classList.add("hidden");
    document.getElementById("add-reset-cover").hidden = false;
  };
  reader.onerror = () => setStatus("No se pudo leer la imagen.", true);
  reader.readAsDataURL(file);
  e.target.value = "";
}

// Descarta la portada elegida: vuelve a las iniciales.
function resetAddCover() {
  addCoverDataUrl = null;
  const img = document.getElementById("add-art-img");
  img.style.display = "none";
  img.removeAttribute("src");
  document.getElementById("add-art-fallback").classList.remove("hidden");
  document.getElementById("add-reset-cover").hidden = true;
  // Garantía: la opción de volver a elegir portada queda siempre visible.
  document.getElementById("add-change-cover").hidden = false;
}

// Confirma: persiste el juego (y su portada, si hay) y refresca la lista.
async function confirmAddGame() {
  if (!invoke || !addExe) return;
  const name = document.getElementById("add-name").value.trim();
  if (!name) return; // el botón ya viene deshabilitado; por seguridad.
  const btn = document.getElementById("add-confirm");
  btn.disabled = true;
  setStatus("Guardando…", false);
  try {
    const game = await invoke("add_game", { name, exe: addExe });

    // Portada elegida: se guarda bajo el id determinístico del juego manual.
    if (addCoverDataUrl) {
      try {
        await invoke("set_custom_cover", { appId: game.id, dataUrl: addCoverDataUrl });
        customCovers.set(game.id, addCoverDataUrl);
      } catch (err) {
        console.warn("No se pudo guardar la portada del juego agregado:", err);
      }
    }

    // Re-escaneo completo para integrar el nuevo juego en su posición.
    games = await invoke("list_games");
    const covers = await invoke("get_custom_covers");
    for (const [id, path] of Object.entries(covers)) {
      customCovers.set(id, tauri.core.convertFileSrc(path));
    }
    requestAutoCovers(); // resuelve portadas auto para juegos no-Steam nuevos

    applyFilter();  // filtra la lista nueva con el texto del buscador actual
    buildCards();   // reconstruye las tarjetas
    const idx = filtered.findIndex((g) => g.id === game.id);
    if (idx >= 0) {
      setActive(idx); // centra en el juego agregado
      setStatus(`“${game.name}” agregado al launcher.`, true);
    } else {
      // El escáner ya lo tenía bajo otro id (dedup por .exe o nombre).
      layout();
      setStatus(`“${name}” ya estaba en el launcher.`, true);
    }
    closeAddOverlay();
  } catch (err) {
    setStatus(`Error al agregar: ${err}`, true);
    updateAddConfirm(); // deja el botón en su estado correcto para reintentar
  }
}

// ---------------------------------------------------------------------------
// Paneo de la portada en Detalles: arrastrar la imagen dentro de su marco para
// re-centrarla a voluntad. Se implementa moviendo object-position sobre un
// <img> con object-fit:cover (el navegador recorta y no deja gaps). Solo hay
// área para mover cuando la imagen recorta el marco (ratio distinto al 2:3);
// con ratio idéntico (p. ej. arte Steam 600x900) queda desactivado.
// El offset es efímero: se re-centra al abrir detalles o al cambiar/quitar portada.
// ---------------------------------------------------------------------------
const dtPan = { x: 0, y: 0 };   // desplazamiento de la ventana visible (px), desde el borde izquierdo
const dtCover = { ox: 0, oy: 0 }; // overflow total (img pintada - marco) por eje
let dtPanDrag = null;            // sesión de arrastre activa

function applyDTArtPan() {
  const img = document.getElementById("dt-art-img");
  if (!img) return;
  img.style.objectPosition = `${-dtPan.x}px ${-dtPan.y}px`;
}

// Mide el recorte real y re-centra la portada. Se llama con rAF después de
// mostrar el overlay (así box.clientWidth ya existe) y en cada carga de imagen.
function syncDetailsArt() {
  const img = document.getElementById("dt-art-img");
  const box = img ? img.parentElement : null;
  if (!img || !box) return;
  if (img.style.display === "none") return;      // sin imagen: no aplicar
  const cw = box.clientWidth, ch = box.clientHeight;
  const iw = img.naturalWidth, ih = img.naturalHeight;
  if (!cw || !ch || !iw || !ih) return;          // aún no listo
  const s = Math.max(cw / iw, ch / ih);          // escala de object-fit: cover
  dtCover.ox = iw * s - cw;                       // >= 0
  dtCover.oy = ih * s - ch;
  box.classList.toggle("can-pan", dtCover.ox > 0 || dtCover.oy > 0);
  // Si el usuario panéo esta portada antes, restaura su posición (normalizada);
  // si no, centra. El clamp cubre cambios de tamaño del marco entre sesiones.
  const key = detailsGame ? (detailsGame.appId || detailsGame.id) : null;
  const saved = key ? coverPans.get(key) : undefined;
  if (saved && Number.isFinite(saved.x) && Number.isFinite(saved.y)) {
    dtPan.x = Math.max(0, Math.min(dtCover.ox, saved.x * dtCover.ox));
    dtPan.y = Math.max(0, Math.min(dtCover.oy, saved.y * dtCover.oy));
  } else {
    dtPan.x = dtCover.ox / 2;                     // centrar
    dtPan.y = dtCover.oy / 2;
  }
  applyDTArtPan();
}

function setupDetailsPan() {
  const img = document.getElementById("dt-art-img");
  const box = img ? img.parentElement : null;
  if (!img || !box) return;

  img.addEventListener("load", () => { requestAnimationFrame(syncDetailsArt); });

  img.addEventListener("mousedown", (e) => {
    if (img.style.display === "none") return;     // sin imagen: sin paneo
    if (dtCover.ox <= 0 && dtCover.oy <= 0) return; // ratio idéntico: nada a recortar
    e.preventDefault();                            // evita selección/drag nativo de <img>
    dtPanDrag = { sx: e.clientX, sy: e.clientY, ox: dtPan.x, oy: dtPan.y };
    box.classList.add("panning");
  });

  window.addEventListener("mousemove", (e) => {
    if (!dtPanDrag) return;
    const dx = e.clientX - dtPanDrag.sx;
    const dy = e.clientY - dtPanDrag.sy;
    // El contenido sigue al cursor: arrastrar a la derecha muestra más de la izquierda.
    let px = dtPanDrag.ox - dx;
    let py = dtPanDrag.oy - dy;
    px = Math.max(0, Math.min(dtCover.ox, px));
    py = Math.max(0, Math.min(dtCover.oy, py));
    dtPan.x = px;
    dtPan.y = py;
    applyDTArtPan();
  });

  window.addEventListener("mouseup", () => {
    if (!dtPanDrag) return;
    dtPanDrag = null;
    box.classList.remove("panning");
    persistDetailsPan(); // el offset queda guardado: se restaura al reabrir Detalles
  });
}

// offset guardado (0..1) → object-position en px para un <img> dado.
// La caja de Detalles (.dt-art) y la tarjeta (.card) comparten ratio 0.7,
// así que la MISMA fracción produce el MISMO recorte en ambas: el dato
// persistido una sola vez sirve para los dos marcos. null = sin aplicar.
function cardPanPosition(game, img) {
  const saved = coverPans.get(game.appId || game.id);
  if (!saved || !Number.isFinite(saved.x) || !Number.isFinite(saved.y)) return null;
  const box = img.parentElement;
  const cw = box.clientWidth, ch = box.clientHeight;
  const iw = img.naturalWidth, ih = img.naturalHeight;
  if (!cw || !ch || !iw || !ih) return null; // aún no listo
  const s = Math.max(cw / iw, ch / ih);       // escala de object-fit: cover
  const ox = Math.max(0, iw * s - cw);
  const oy = Math.max(0, ih * s - ch);
  return `-${Math.min(saved.x * ox, ox)}px -${Math.min(saved.y * oy, oy)}px`;
}

// Aplica (o despeja) el paneo guardado en la portada de una tarjeta del carrusel.
function applyCardPan(game, card) {
  const img = card.querySelector("img.art");
  if (!img) return;
  const pos = cardPanPosition(game, img);
  if (pos === null) img.style.removeProperty("object-position"); // sin offset → centro
  else img.style.objectPosition = pos;
}

// Guarda la posición actual del paneo, normalizada (0..1 del overflow total),
// para que sobreviva a cerrar/abrir el overlay y a reinicios de la app.
function persistDetailsPan() {
  if (!invoke || !detailsGame) return;
  const key = detailsGame.appId || detailsGame.id;
  const x = dtCover.ox > 0 ? dtPan.x / dtCover.ox : 0;
  const y = dtCover.oy > 0 ? dtPan.y / dtCover.oy : 0;
  coverPans.set(key, { x, y });
  // La tarjeta del carrusel usa el MISMO offset → actualizarla en vivo al soltar
  // (cerrar Detalles no dispara layout(): sin esto la tarjeta seguiría centrada
  // hasta la próxima navegación por rueda).
  const card = cardEls.get(detailsGame.id);
  if (card) applyCardPan(detailsGame, card);
  invoke("save_cover_pan", { appId: key, x, y })
    .catch((err) => console.warn("No se pudo guardar el paneo:", err));
}

// ---------------------------------------------------------------------------
// Inicio
// ---------------------------------------------------------------------------

async function init() {
  if (!invoke) document.body.classList.add("browser-preview"); // fondo solo en preview de browser
  try {
    games = invoke ? await invoke("list_games") : BROWSER_DEMO;
  } catch (err) {
    console.error("list_games falló:", err);
    games = BROWSER_DEMO;
  }

  // Cargar portadas personalizadas guardadas.
  if (invoke) {
    try {
      const covers = await invoke("get_custom_covers");
      for (const [appId, path] of Object.entries(covers)) {
        // Convertir la ruta absoluta a una URL cargable por el WebView.
        customCovers.set(appId, tauri.core.convertFileSrc(path));
      }
    } catch (err) {
      console.warn("No se pudieron cargar portadas personalizadas:", err);
    }

    // Cargar posiciones de portada guardadas (paneo del overlay Detalles).
    try {
      const pans = await invoke("get_cover_pans");
      for (const [key, p] of Object.entries(pans || {})) coverPans.set(key, p);
    } catch (err) {
      console.warn("No se pudieron cargar posiciones de portada:", err);
    }

    // Cargar argumentos de lanzamiento guardados (para prellenar el overlay).
    try {
      const args = await invoke("get_launch_args");
      for (const [id, val] of Object.entries(args || {})) argsMap[id] = val;
    } catch (err) {
      console.warn("No se pudieron cargar argumentos de lanzamiento:", err);
    }

  }

  // Portadas auto para juegos no-Steam sin portada custom (storesearch/SGDB).
  requestAutoCovers();
  // Si se activa/cambia la key de SGDB desde la bandeja, reintenta lo pendiente.
  if (invoke) tauri.event.listen("cardynox:sgdb-changed", () => requestAutoCovers());

  filtered = computeFiltered();
  buildCards();
  bindEvents();
  setupDetailsPan();
  initGadgetConfig();
  applyUiScale();
  layout();
  setStatus(baseStatus(), false);
}

init();
