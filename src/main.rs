//! Mosaic — tablero de referencias para artistas: imágenes, vídeos, documentos, modelos 3D, textos y
//! dibujo a mano alzada en un lienzo infinito acelerado por GPU.
// en Windows, sin ventana de consola detrás de la aplicación
#![cfg_attr(windows, windows_subsystem = "windows")]
mod app;
mod audio;
mod board;
mod canvas;
mod chrome;
mod decode;
mod dragout;
mod fade;
#[cfg(all(unix, not(any(target_os = "macos", target_os = "android"))))]
mod dragout_linux;
#[cfg(target_os = "macos")]
mod dragout_macos;
#[cfg(windows)]
mod dragout_windows;
mod gpu3d;
mod i18n;
mod i18n_table;
mod import;
mod markup;
mod media;
mod mesh3d;
mod pbr;
mod textures;

use std::path::PathBuf;

fn main() -> eframe::Result {
    decode::install_panic_hook();
    decode::tune_allocator();
    decode::cleanup_caches(false);
    let args: Vec<PathBuf> = std::env::args_os().skip(1).map(PathBuf::from).collect();
    // «siempre encima» en Wayland: modo compatible (XWayland), recordado entre sesiones
    let cfg = app::Config::load();
    let compat = std::env::var_os("REFS_X11").is_some() || (cfg.on_top && app::native_wayland());
    if compat && std::env::var_os("DISPLAY").is_some() {
        // SAFETY: al arrancar, antes de crear ningún hilo
        unsafe {
            std::env::remove_var("WAYLAND_DISPLAY");
            std::env::set_var("REFS_ON_TOP", "1");
        }
    }
    let on_top = std::env::var_os("REFS_ON_TOP").is_some();
    let icon = image::load_from_memory(include_bytes!("../assets/icon-256.png")).ok().map(|i| {
        let i = i.into_rgba8();
        egui::IconData { width: i.width(), height: i.height(), rgba: i.into_raw() }
    });
    let vb = egui::ViewportBuilder::default()
        .with_title("Mosaic")
        .with_app_id("mosaic")
        .with_inner_size([1280.0, 800.0])
        .with_min_inner_size([320.0, 240.0])
        .with_drag_and_drop(true)
        // sin decoración del sistema: la cabecera la dibuja la aplicación (como File Browser)
        .with_decorations(false)
        // ventana transparente: la opacidad se elige desde la aplicación
        .with_transparent(true)
        .with_window_level(if on_top { egui::WindowLevel::AlwaysOnTop } else { egui::WindowLevel::Normal });
    let vb = match icon {
        Some(i) => vb.with_icon(std::sync::Arc::new(i)),
        None => vb,
    };
    // sin MSAA; búfer de profundidad para el visor 3D (como en File Browser)
    let mut o = eframe::NativeOptions { viewport: vb, multisampling: 0, depth_buffer: 24, ..Default::default() };
    // igual que File Browser: en Wayland, el vsync de EGL bloquea el bucle si la ventana está tapada
    if std::env::var_os("WAYLAND_DISPLAY").is_some() {
        o.glow_options.vsync = false;
    }
    eframe::run_native("Mosaic", o, Box::new(move |cc| Ok(Box::new(app::App::new(cc, args)))))
}
