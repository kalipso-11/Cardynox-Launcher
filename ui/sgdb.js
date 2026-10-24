/* ============================================================
   SteamGridDB — configuración de la API key (ventana desde la bandeja).
   Sin dependencias: HTML + CSS + JS vanilla. Habla con Rust vía
   window.__TAURI__.core.invoke. El tráfico a SGDB lo hace Rust, no el WebView.
   ============================================================ */

"use strict";

const tauri = window.__TAURI__;
const invoke = tauri ? tauri.core.invoke : null;

const keyInput = document.getElementById("key-input");
const statusEl = document.getElementById("status");
const resultEl = document.getElementById("result");

function maskKey(k) {
  if (!k) return "";
  const t = k.trim();
  return t.length <= 4 ? "****" : "***" + t.slice(-4);
}

function updateStatus(key) {
  const on = !!(key && key.trim());
  statusEl.classList.toggle("on", on);
  statusEl.classList.toggle("off", !on);
  statusEl.textContent = on ? "SGDB activo (key " + maskKey(key) + ")" : "SGDB desactivado";
}

function showResult(text, isErr) {
  resultEl.textContent = text || "";
  resultEl.classList.toggle("err", !!isErr);
}

if (invoke) {
  // Prellena el campo con la key actual y pinta el estado.
  invoke("get_sgdb_key")
    .then((k) => {
      if (k) keyInput.value = k;
      updateStatus(k);
    })
    .catch((e) => showResult("No se pudo leer el estado actual: " + e, true));

  document.getElementById("toggle-vis").addEventListener("click", () => {
    keyInput.type = keyInput.type === "password" ? "text" : "password";
  });

  document.getElementById("btn-test").addEventListener("click", async () => {
    showResult("Probando la key…", false);
    try {
      const ok = await invoke("check_sgdb_key", { key: keyInput.value });
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
        keyInput.value.trim()
          ? "Key guardada. Se aplicará a las portadas que aún faltan."
          : "SGDB desactivado.",
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
      showResult("SGDB desactivado.", false);
    } catch (e) {
      showResult(String(e), true);
    }
  });

  // Enlaces externos: se abren en el navegador del sistema (no en la webview).
  document.querySelectorAll("a[data-external]").forEach((link) => {
    link.addEventListener("click", (e) => {
      e.preventDefault();
      invoke("open_external", { url: link.href })
        .catch((err) => showResult("No se pudo abrir el navegador: " + err, true));
    });
  });
} else {
  // Esta ventana solo tiene sentido dentro de Tauri; en un browser no hay IPC.
  showResult("Esta ventana solo funciona dentro del launcher.", true);
}
