/* ============================================================
   Ventana "Juegos bloqueados" — lista los juegos sacados del launcher y
   permite re-habilitarlos. Sin dependencias: HTML + CSS + JS vanilla. Habla
   con Rust vía window.__TAURI__.core.invoke. Soporta i18n (es/en) desde la
   misma config.lang del gadget.
   ============================================================ */

"use strict";

const tauri = window.__TAURI__;
const invoke = tauri ? tauri.core.invoke : null;

// ---------------------------------------------------------------------------
// i18n — mismo esquema que el gadget y SGDB: data-i18n* + t()/tf().
// ---------------------------------------------------------------------------
const I18N = {
  es: {
    title: "Juegos bloqueados",
    sub: "Juegos que sacaste del launcher. Podés re-habilitarlos desde acá.",
    empty: "No hay juegos bloqueados.",
    enable: "Habilitar",
    enabling: "…",
    statusOk: "“{name}” re-habilitado.",
    statusDiscarded: "“{name}” eliminado de la lista.",
    readErr: "No se pudo leer la lista: {err}",
    errGeneric: "Ocurrió un error: {err}",
    dlgTitle: "El juego ya no está instalado",
    dlgBody: "El juego ya no está instalado en la ruta original. Se eliminará de esta lista; instalalo de nuevo y volvelo a agregar desde el botón “Agregar”.",
    cancel: "Cancelar",
    accept: "Aceptar",
    srcOther: "Otro",
    plat_steam: "Steam",
    plat_epic: "Epic",
    plat_gog: "GOG",
    plat_ea: "EA",
    plat_ubisoft: "Ubisoft",
    plat_battle: "Battle.net",
    plat_rockstar: "Rockstar",
    plat_amazon: "Amazon",
    plat_itch: "itch.io",
    plat_heroic: "Heroic",
    plat_xbox: "Xbox",
    plat_other: "Otros",
  },
  en: {
    title: "Blocked games",
    sub: "Games you removed from the launcher. You can re-enable them here.",
    empty: "No blocked games.",
    enable: "Enable",
    enabling: "…",
    statusOk: "“{name}” re-enabled.",
    statusDiscarded: "“{name}” removed from the list.",
    readErr: "Could not read the list: {err}",
    errGeneric: "An error occurred: {err}",
    dlgTitle: "The game is no longer installed",
    dlgBody: "The game is no longer installed at its original path. It will be removed from this list; install it again and add it back from the “Add” button.",
    cancel: "Cancel",
    accept: "Accept",
    srcOther: "Other",
    plat_steam: "Steam",
    plat_epic: "Epic",
    plat_gog: "GOG",
    plat_ea: "EA",
    plat_ubisoft: "Ubisoft",
    plat_battle: "Battle.net",
    plat_rockstar: "Rockstar",
    plat_amazon: "Amazon",
    plat_itch: "itch.io",
    plat_heroic: "Heroic",
    plat_xbox: "Xbox",
    plat_other: "Other",
  },
};

let lang = "es";

function t(key) {
  const d = I18N[lang] || I18N.es;
  return key in d ? d[key] : key;
}
function tf(template, vars) {
  let s = t(template);
  for (const [k, v] of Object.entries(vars)) s = s.split("{" + k + "}").join(String(v));
  return s;
}

function applyLang(l) {
  lang = I18N[l] ? l : "es";
  document.documentElement.lang = lang;
  for (const el of document.querySelectorAll("[data-i18n]")) el.textContent = t(el.dataset.i18n);
  render(); // re-dibuja con los labels nuevos (plataformas, botones)
}

// ---------------------------------------------------------------------------
// Estado
// ---------------------------------------------------------------------------
let blocked = [];        // juegos bloqueados actuales (BloquedGame[])
let pendingId = null;    // id del juego que espera el cartel "no instalado"

const listEl = document.getElementById("list");
const emptyEl = document.getElementById("empty");
const statusEl = document.getElementById("status");
const overlayEl = document.getElementById("confirm");

function initials(name) {
  const parts = (name || "").trim().split(/\s+/).filter(Boolean);
  if (parts.length === 0) return "?";
  if (parts.length === 1) return parts[0].slice(0, 2).toUpperCase();
  return (parts[0][0] + parts[1][0]).toUpperCase();
}

function platformLabel(source) {
  const s = (source || "").toLowerCase();
  let key = null;
  if (s === "steam" || s === "demo") key = "steam";
  else if (s === "epic") key = "epic";
  else if (s === "gog") key = "gog";
  else if (s === "ea" || s === "origin") key = "ea";
  else if (s === "ubisoft" || s === "uplay") key = "ubisoft";
  else if (s === "battle" || s === "blizzard") key = "battle";
  else if (s === "rockstar") key = "rockstar";
  else if (s === "amazon" || s === "prime") key = "amazon";
  else if (s === "itch") key = "itch";
  else if (s === "heroic") key = "heroic";
  else if (s === "xbox") key = "xbox";
  else if (s === "manual" || s === "folder" || s === "registry" || s === "dir" || s === "unknown" || s === "") key = "other";
  if (key && t("plat_" + key)) return t("plat_" + key);
  return source || t("srcOther");
}

function showStatus(text, kind) {
  statusEl.textContent = text || "";
  statusEl.classList.toggle("ok", kind === "ok");
  statusEl.classList.toggle("err", kind === "err");
}

/** Dibuja la lista (idempotente: se re-llama al cambiar de idioma). */
function render() {
  listEl.innerHTML = "";
  for (const g of blocked) {
    const row = document.createElement("div");
    row.className = "row";
    row.dataset.id = g.id;

    const cover = document.createElement("div");
    cover.className = "cover init";
    cover.textContent = initials(g.name || g.id);

    const info = document.createElement("div");
    info.className = "info";
    const name = document.createElement("div");
    name.className = "name";
    name.textContent = g.name || g.id;
    name.title = g.name || g.id;
    const src = document.createElement("div");
    src.className = "src";
    src.textContent = platformLabel(g.source);
    info.append(name, src);

    const btn = document.createElement("button");
    btn.className = "enable";
    btn.textContent = t("enable");
    btn.addEventListener("click", () => enableGame(g, row, btn));

    row.append(cover, info, btn);
    listEl.appendChild(row);
  }
  emptyEl.hidden = blocked.length > 0;
}

async function load() {
  if (!invoke) return;
  try {
    blocked = await invoke("get_blocked_games");
    render();
  } catch (e) {
    showStatus(tf("readErr", { err: e }), "err");
  }
}

function removeRow(id, statusText, kind) {
  blocked = blocked.filter((g) => g.id !== id);
  const el = listEl.querySelector(`[data-id="${CSS.escape(id)}"]`);
  if (el) el.remove();
  emptyEl.hidden = blocked.length > 0;
  if (statusText) showStatus(statusText, kind);
}

// ---------------------------------------------------------------------------
// Re-habilitar / descartar
// ---------------------------------------------------------------------------
async function enableGame(g, row, btn) {
  if (!invoke) return;
  btn.disabled = true;
  btn.textContent = t("enabling");
  try {
    const res = await invoke("unblock_game", { id: g.id });
    if (res && res.reenabled) {
      removeRow(g.id, tf("statusOk", { name: g.name || g.id }), "ok");
      return;
    }
    // No está instalado → no lo tocó Rust; pedirle confirmación al usuario.
    pendingId = g.id;
    showConfirm();
  } catch (e) {
    showStatus(tf("errGeneric", { err: e }), "err");
    btn.disabled = false;
    btn.textContent = t("enable");
  }
}

function showConfirm() {
  overlayEl.hidden = false;
}
function hideConfirm() {
  overlayEl.hidden = true;
  pendingId = null;
}

async function acceptDiscard() {
  if (!invoke || !pendingId) return hideConfirm();
  const id = pendingId;
  const name = (blocked.find((g) => g.id === id) || {}).name || id;
  try {
    await invoke("discard_blocked", { id });
    removeRow(id, tf("statusDiscarded", { name }), "ok");
  } catch (e) {
    showStatus(tf("errGeneric", { err: e }), "err");
  }
  hideConfirm();
}

// ---------------------------------------------------------------------------
// Arranque: idioma + datos + eventos
// ---------------------------------------------------------------------------
if (invoke) {
  invoke("get_gadget_config")
    .then((cfg) => applyLang(cfg.lang || "es"))
    .catch(() => {});
  tauri.event.listen("cardynox:set-lang", (e) => applyLang(e.payload));
  load();
} else {
  // Preview en browser: sin datos (se ve la estructura + estado vacío).
  emptyEl.hidden = false;
}

document.getElementById("dlg-cancel").addEventListener("click", hideConfirm);
document.getElementById("dlg-accept").addEventListener("click", acceptDiscard);
overlayEl.addEventListener("click", (e) => { if (e.target === overlayEl) hideConfirm(); });
document.addEventListener("keydown", (e) => {
  if (e.key === "Escape" && !overlayEl.hidden) hideConfirm();
});
