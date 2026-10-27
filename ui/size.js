/* ============================================================
   Cardynox Launcher — popup de Tamaño (slider).
   Ventana dinámica creada desde la bandeja (label "size").
   Sin dependencias: HTML + JS vanilla, habla con Rust vía
   window.__TAURI__.core.invoke.

   Mover el slider redimensiona el gadget EN VIVO (0% = Mini,
   100% = mitad de la pantalla) y persiste config.size_pct.
   Cerrar (✕ o Esc) destruye la ventana para que al abrirla
   de nuevo se recree limpia leyendo el valor actual.
   ============================================================ */

"use strict";

const tauri = window.__TAURI__;

(async () => {
  if (!tauri || !tauri.core) return; // fuera de Tauri: no-op (previsualización en browser).
  const invoke = tauri.core.invoke;

  const slider = document.getElementById("slider");
  const label = document.getElementById("pctLabel");
  const closeBtn = document.getElementById("btnClose");
  if (!slider || !label || !closeBtn) return;

  // Posicionar el slider con el porcentaje actual (config.size_pct 0..=1 → 0..100).
  try {
    const cfg = await invoke("get_gadget_config");
    const pct = (cfg && typeof cfg.size_pct === "number") ? cfg.size_pct : 0.5;
    slider.value = String(Math.round(pct * 100));
    label.textContent = slider.value + "%";
  } catch (err) {
    console.warn("No se pudo leer el tamaño actual:", err);
  }

  // Mover el slider → redimensiona el gadget in-place y guarda la config.
  const apply = () => {
    const pct = Number(slider.value) / 100;
    label.textContent = slider.value + "%";
    invoke("set_size_pct", { pct }).catch((err) =>
      console.warn("No se pudo aplicar el tamaño:", err));
  };
  slider.addEventListener("input", apply);

  // Cerrar la popup: destruye la ventana (fallback: ocultar). En ambos casos,
  // al volver a abrirse desde la bandeja se recrea y vuelve a leer la config.
  const close = () => {
    const win = tauri.window && tauri.window.getCurrentWindow();
    if (win) win.destroy().catch(() => win.hide());
  };
  closeBtn.addEventListener("click", close);
  window.addEventListener("keydown", (e) => {
    if (e.key === "Escape") close();
  });
})();
