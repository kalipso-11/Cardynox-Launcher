// En release no se crea consola (ventana de terminal negra al arrancar).
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    cardynox_launcher_lib::run();
}
