/* ============================================================
   SteamGridDB — configuración de la API key (ventana desde la bandeja).
   Sin dependencias: HTML + CSS + JS vanilla. Habla con Rust vía
   window.__TAURI__.core.invoke. El tráfico a SGDB lo hace Rust, no el WebView.
   Soporta i18n (es/en) desde la misma config.lang del gadget.
   ============================================================ */

"use strict";

const tauri = window.__TAURI__;
const invoke = tauri ? tauri.core.invoke : null;

// ---------------------------------------------------------------------------
// i18n (ventana SGDB) — mismo esquema que el gadget: data-i18n* + t()/tf().
// Los mensajes de check_sgdb_key ya vienen traducidos desde Rust (le pasamos
// el idioma activo en cada invocación).
// ---------------------------------------------------------------------------

const I18N = {
  es: {
    sub: "Configuración de carátulas automáticas",
    whyH: "¿Para qué sirve?",
    whyA: "SteamGridDB es una base de datos comunitaria de carátulas y arte de juegos. En Cardynox funciona como fuente ",
    whyB: "secundaria",
    whyC: ": cuando un juego ",
    whyD: "no",
    whyE: "está en tu biblioteca de Steam y el buscador del store (que no requiere key) tampoco lo encuentra, SGDB busca su portada vertical para mostrarla en la tarjeta.",
    whyMuted: "Es opcional. Los juegos de Steam ya traen su portada; SGDB solo completa las que les faltan a los demás.",
    howH: "Cómo crear tu key",
    step1a: "Creá una cuenta gratuita en",
    step1b: ".",
    step2a: "Después de crear la cuenta, entrá a",
    step2b: "para generar y ver tu API key.",
    step3: "Copiala y pegala abajo.",
    limitsA: "Límite del plan gratuito: ~500 peticiones por hora. El launcher hace muy pocas consultas y guarda caché local, así que sobra holgura. La key se guarda",
    onlyWord: "solo",
    limitsB: "en tu equipo (carpeta de configuración) y nunca se distribuye.",
    statusOff: "SGDB desactivado",
    statusOn: "SGDB activo (key {key})",
    keyPh: "Pegá tu API key de SteamGridDB",
    toggleTitle: "Mostrar / ocultar",
    btnSave: "Guardar y activar",
    btnTest: "Probar",
    btnDisable: "Desactivar",
    readErr: "No se pudo leer el estado actual: {err}",
    testing: "Probando la key…",
    savedMsg: "Key guardada. Se aplicará a las portadas que aún faltan.",
    disabledMsg: "SGDB desactivado.",
    browserErr: "No se pudo abrir el navegador: {err}",
    browserOnly: "Esta ventana solo funciona dentro del launcher.",
  },
  en: {
    sub: "Automatic cover settings",
    whyH: "What is it for?",
    whyA: "SteamGridDB is a community database of game covers and art. In Cardynox it works as a ",
    whyB: "secondary",
    whyC: " source: when a game ",
    whyD: "is not",
    whyE: " in your Steam library and the store search (which needs no key) can't find it either, SGDB looks for its vertical cover to show it on the card.",
    whyMuted: "It is optional. Steam games already have their cover; SGDB only fills in the ones the others are missing.",
    howH: "How to create your key",
    step1a: "Create a free account on",
    step1b: ".",
    step2a: "After creating your account, go to",
    step2b: "to generate and view your API key.",
    step3: "Copy it and paste it below.",
    limitsA: "Free plan limit: ~500 requests per hour. The launcher makes very few queries and keeps a local cache, so there's plenty of headroom. The key is stored",
    onlyWord: "only",
    limitsB: "on your machine (config folder) and never distributed.",
    statusOff: "SGDB disabled",
    statusOn: "SGDB enabled (key {key})",
    keyPh: "Paste your SteamGridDB API key",
    toggleTitle: "Show / hide",
    btnSave: "Save and enable",
    btnTest: "Test",
    btnDisable: "Disable",
    readErr: "Could not read the current state: {err}",
    testing: "Testing the key…",
    savedMsg: "Key saved. It will apply to the covers still missing.",
    disabledMsg: "SGDB disabled.",
    browserErr: "Could not open the browser: {err}",
    browserOnly: "This window only works inside the launcher.",
  },
};

let lang = "es"; // idioma actual (config.lang, compartido con el gadget)

function t(key) {
  const d = I18N[lang] || I18N.es;
  return key in d ? d[key] : key;
}

function tf(template, vars) {
  let s = t(template);
  for (const [k, v] of Object.entries(vars)) s = s.split("{" + k + "}").join(String(v));
  return s;
}

/** Aplica el idioma a esta ventana (estático + estado dinámico). */
function applyLang(l) {
  lang = I18N[l] ? l : "es";
  document.documentElement.lang = lang;
  for (const el of document.querySelectorAll("[data-i18n]")) el.textContent = t(el.dataset.i18n);
  for (const el of document.querySelectorAll("[data-i18n-ph]")) el.placeholder = t(el.dataset.i18nPh);
  for (const el of document.querySelectorAll("[data-i18n-title]")) el.title = t(el.dataset.i18nTitle);
  updateStatus(currentKey); // re-dibuja "SGDB activo/desactivado" en el idioma nuevo
}

// ---------------------------------------------------------------------------
// Estado y UI
// ---------------------------------------------------------------------------

const keyInput = document.getElementById("key-input");
const statusEl = document.getElementById("status");
const resultEl = document.getElementById("result");

let currentKey = null; // key actualmente guardada (para re-dibujar el estado)

function maskKey(k) {
  if (!k) return "";
  const t = k.trim();
  return t.length <= 4 ? "****" : "***" + t.slice(-4);
}

function updateStatus(key) {
  currentKey = key;
  const on = !!(key && key.trim());
  statusEl.classList.toggle("on", on);
  statusEl.classList.toggle("off", !on);
  statusEl.textContent = on ? tf("statusOn", { key: maskKey(key) }) : t("statusOff");
}

function showResult(text, isErr) {
  resultEl.textContent = text || "";
  resultEl.classList.toggle("err", !!isErr);
}

if (invoke) {
  // Idioma: el mismo que el gadget (config.lang). Si cambia desde la bandeja
  // mientras esta ventana está abierta, se actualiza en vivo.
  invoke("get_gadget_config")
    .then((cfg) => applyLang(cfg.lang || "es"))
    .catch(() => {});
  tauri.event.listen("cardynox:set-lang", (e) => applyLang(e.payload));

  // Prellena el campo con la key actual y pinta el estado.
  invoke("get_sgdb_key")
    .then((k) => {
      if (k) keyInput.value = k;
      updateStatus(k);
    })
    .catch((e) => showResult(tf("readErr", { err: e }), true));

  document.getElementById("toggle-vis").addEventListener("click", () => {
    keyInput.type = keyInput.type === "password" ? "text" : "password";
  });

  document.getElementById("btn-test").addEventListener("click", async () => {
    showResult(t("testing"), false);
    try {
      // Rust traduce el resultado según el idioma que mandamos.
      const ok = await invoke("check_sgdb_key", { key: keyInput.value, lang });
      showResult(ok, false);
    } catch (e) {
      showResult(String(e), true);
    }
  });

  document.getElementById("btn-save").addEventListener("click", async () => {
    try {
      await invoke("set_sgdb_key", { key: keyInput.value });
      updateStatus(keyInput.value);
      showResult(
        keyInput.value.trim() ? t("savedMsg") : t("disabledMsg"),
        false
      );
    } catch (e) {
      showResult(String(e), true);
    }
  });

  document.getElementById("btn-disable").addEventListener("click", async () => {
    keyInput.value = "";
    try {
      await invoke("set_sgdb_key", { key: "" });
      updateStatus("");
      showResult(t("disabledMsg"), false);
    } catch (e) {
      showResult(String(e), true);
    }
  });

  // Enlaces externos: se abren en el navegador del sistema (no en la webview).
  document.querySelectorAll("a[data-external]").forEach((link) => {
    link.addEventListener("click", (e) => {
      e.preventDefault();
      invoke("open_external", { url: link.href })
        .catch((err) => showResult(tf("browserErr", { err }), true));
    });
  });
} else {
  // Esta ventana solo tiene sentido dentro de Tauri; en un browser no hay IPC.
  showResult(t("browserOnly"), true);
}
