//! Vídeos y documentos en el tablero.
//!
//! - **Vídeo** (como en File Browser): `ffmpeg` da el póster, el tamaño y la duración al importar, y
//!   reproduce con `-re` decodificando a PPM por una tubería que se dibuja como textura; el audio lo
//!   pone `ffplay`. Los vídeos guardados dentro de un `.refs` se leen sin extraerlos, con el
//!   protocolo `subfile` de ffmpeg sobre el propio tablero (y se puede saltar a cualquier punto).
//! - **Documentos** (PDF y oficina) con `apex-view`, el renderizador de File Browser. Son vectoriales:
//!   cada nivel de detalle se dibuja a su tamaño, así que al acercarse el texto sigue nítido. Cada
//!   hilo de decodificación conserva abiertos los últimos documentos que dibujó.
use crate::decode::{Bytes, Decoded, lock};
use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

pub const VIDEO_EXTS: &[&str] = &["mp4", "mkv", "avi", "mov", "webm", "wmv", "flv", "m4v", "mpg", "mpeg", "ts", "3gp", "ogv"];
pub const DOC_EXTS: &[&str] = &["pdf", "docx", "xlsx", "pptx", "odt", "ods", "odp", "rtf", "doc", "xls", "ppt"];
/// Lado mayor al que se dibujan como mucho las páginas (al acercarse mucho).
pub const DOC_NATIVE: u32 = 4096;
/// Puntos de página → unidades del mundo (una página A4 mide ~1190 × 1684).
pub const DOC_SCALE: f32 = 2.0;

fn ext_in(p: &Path, list: &[&str]) -> bool {
    p.extension().and_then(|e| e.to_str()).is_some_and(|e| list.iter().any(|x| x.eq_ignore_ascii_case(e)))
}

pub fn is_video_path(p: &Path) -> bool {
    ext_in(p, VIDEO_EXTS)
}

pub fn is_doc_path(p: &Path) -> bool {
    ext_in(p, DOC_EXTS)
}

fn which(bin: &str) -> bool {
    let exe = if cfg!(windows) { format!("{bin}.exe") } else { bin.to_string() };
    std::env::var_os("PATH").map(|p| std::env::split_paths(&p).any(|d| d.join(&exe).is_file())).unwrap_or(false)
}

pub fn have_ffmpeg() -> bool {
    static V: OnceLock<bool> = OnceLock::new();
    *V.get_or_init(|| which("ffmpeg"))
}

fn have_ffplay() -> bool {
    static V: OnceLock<bool> = OnceLock::new();
    *V.get_or_init(|| which("ffplay"))
}

fn quiet(cmd: &mut Command) -> &mut Command {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000);
    }
    cmd
}

const WHITELIST: &str = "file,pipe,subfile";

/// Entrada para ffmpeg: el archivo, una ventana dentro del tablero (`subfile`) o, si sólo está en
/// memoria, una copia temporal.
pub fn ffmpeg_input(bytes: &Bytes, src: u64) -> Option<String> {
    if let Some((path, off, len)) = bytes.file_window() {
        let whole = std::fs::metadata(path).map(|m| m.len() as usize == len && off == 0).unwrap_or(false);
        let p = path.to_str()?;
        return Some(if whole { p.to_string() } else { format!("subfile,,start,{},end,{},,:{}", off, off + len, p) });
    }
    let dir = std::env::temp_dir().join(format!("mosaic-{}", std::process::id()));
    std::fs::create_dir_all(&dir).ok()?;
    let p = dir.join(format!("{src:016x}.video"));
    if !p.exists() {
        std::fs::write(&p, bytes.as_slice()).ok()?;
    }
    p.to_str().map(|s| s.to_string())
}

// ---------------------------------------------------------------------------------------------
// Vídeo: información al importar
// ---------------------------------------------------------------------------------------------

pub struct VideoInfo {
    pub poster_png: Vec<u8>,
    pub w: u32,
    pub h: u32,
    pub duration: f32,
}

/// Lanza la orden y recoge la salida estándar y la de errores; si se pasa del plazo se mata.
fn run_both(mut c: Command, limit: Duration) -> Option<(Vec<u8>, Vec<u8>, bool)> {
    c.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
    quiet(&mut c);
    let mut child = c.spawn().ok()?;
    let mut so = child.stdout.take()?;
    let mut se = child.stderr.take()?;
    let t_out = std::thread::spawn(move || {
        let mut b = Vec::new();
        let _ = (&mut so).take(256 << 20).read_to_end(&mut b);
        let _ = std::io::copy(&mut so, &mut std::io::sink());
        b
    });
    let t_err = std::thread::spawn(move || {
        let mut b = Vec::new();
        let _ = (&mut se).take(1 << 20).read_to_end(&mut b);
        let _ = std::io::copy(&mut se, &mut std::io::sink());
        b
    });
    let end = Instant::now() + limit;
    let ok = loop {
        match child.try_wait() {
            Ok(Some(st)) => break st.success(),
            Ok(None) if Instant::now() < end => std::thread::sleep(Duration::from_millis(10)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                break false;
            }
        }
    };
    Some((t_out.join().ok()?, t_err.join().ok()?, ok))
}

pub fn parse_duration(s: &str) -> f32 {
    let Some(i) = s.find("Duration: ") else { return 0.0 };
    let t: String = s[i + 10..].chars().take_while(|c| c.is_ascii_digit() || *c == ':' || *c == '.').collect();
    let mut secs = 0.0f64;
    for part in t.split(':') {
        secs = secs * 60.0 + part.parse::<f64>().unwrap_or(0.0);
    }
    secs as f32
}

/// Tamaño del primer flujo de vídeo en la salida de `ffmpeg -i` («…, 1920x1080 [SAR…» o «…, 1920x1080,»).
pub fn parse_video_size(s: &str) -> Option<(u32, u32)> {
    let line = s.lines().find(|l| l.contains("Stream") && l.contains("Video:"))?;
    for tok in line.split([',', ' ', '[']) {
        if let Some((a, b)) = tok.split_once('x')
            && let (Ok(w), Ok(h)) = (a.parse::<u32>(), b.parse::<u32>())
                && w >= 2 && h >= 2 && w <= 65535 && h <= 65535 {
                    return Some((w, h));
                }
    }
    None
}

/// Póster (fotograma en el segundo 1, o el primero), tamaño real y duración.
pub fn probe_video(input: &str) -> Result<VideoInfo, String> {
    if !have_ffmpeg() {
        return Err(t("hace falta ffmpeg para los vídeos").into());
    }
    let scale = "scale='min(2048,iw)':'min(2048,ih)':force_original_aspect_ratio=decrease";
    for seek in ["1", "0"] {
        let mut c = Command::new("ffmpeg");
        c.args(["-hide_banner", "-nostdin", "-ss", seek, "-protocol_whitelist", WHITELIST, "-i", input]);
        c.args(["-frames:v", "1", "-vf", scale, "-f", "image2pipe", "-c:v", "png", "-"]);
        let Some((png, err, _)) = run_both(c, Duration::from_secs(25)) else { continue };
        let err = String::from_utf8_lossy(&err);
        let Some((pw, ph)) = (!png.is_empty()).then(|| crate::decode::probe(&png)).flatten() else { continue };
        let (w, h) = match parse_video_size(&err) {
            // el fotograma ya sale girado según los metadatos: si la orientación no coincide, se cambia
            Some((w, h)) if (w >= h) != (pw >= ph) => (h, w),
            Some(s) => s,
            None => (pw, ph),
        };
        return Ok(VideoInfo { poster_png: png, w, h, duration: parse_duration(&err) });
    }
    Err(t("ffmpeg no pudo leer el vídeo").into())
}

// ---------------------------------------------------------------------------------------------
// Vídeo: reproductor (el de File Browser, en bucle y con silencio)
// ---------------------------------------------------------------------------------------------

#[derive(Default)]
struct Shared {
    epoch: AtomicU64,
    ended: AtomicU64,
    frame: Mutex<Option<egui::ColorImage>>,
}

pub struct Player {
    input: String,
    ctx: egui::Context,
    duration: f64,
    shared: Arc<Shared>,
    video: Option<Child>,
    audio: Option<Child>,
    base: f64,
    started: Option<Instant>,
    pub tex: Option<egui::TextureHandle>,
    pub muted: bool,
    pub error: Option<String>,
}

impl Player {
    pub fn new(ctx: &egui::Context, input: String, duration: f32) -> Player {
        Player {
            input,
            ctx: ctx.clone(),
            duration: duration as f64,
            shared: Arc::new(Shared::default()),
            video: None,
            audio: None,
            base: 0.0,
            started: None,
            tex: None,
            muted: !have_ffplay(),
            error: None,
        }
    }

    pub fn duration(&self) -> f64 {
        self.duration
    }

    pub fn playing(&self) -> bool {
        self.started.is_some()
    }

    pub fn position(&self) -> f64 {
        let p = self.base + self.started.map(|s| s.elapsed().as_secs_f64()).unwrap_or(0.0);
        if self.duration > 0.0 { p.min(self.duration) } else { p }
    }

    fn stop_procs(&mut self) {
        self.shared.epoch.fetch_add(1, Ordering::SeqCst);
        for mut c in [self.video.take(), self.audio.take()].into_iter().flatten() {
            let _ = c.kill();
            std::thread::spawn(move || {
                let _ = c.wait();
            });
        }
    }

    fn start(&mut self, pos: f64) {
        self.stop_procs();
        let g = self.shared.epoch.load(Ordering::SeqCst);
        let mut v = Command::new("ffmpeg");
        quiet(&mut v)
            .args(["-hide_banner", "-loglevel", "error", "-ss", &format!("{pos:.3}"), "-re", "-protocol_whitelist", WHITELIST, "-i", &self.input])
            .args(["-an", "-sn", "-vf", "scale='min(1920,iw)':-2:flags=fast_bilinear,fps=30", "-f", "image2pipe", "-vcodec", "ppm", "-"])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        let mut child = match v.spawn() {
            Ok(c) => c,
            Err(e) => {
                self.error = Some(e.to_string());
                return;
            }
        };
        let Some(out) = child.stdout.take() else { return };
        self.video = Some(child);
        let (shared, ctx) = (self.shared.clone(), self.ctx.clone());
        std::thread::spawn(move || {
            let mut r = BufReader::with_capacity(1 << 20, out);
            let mut line = String::new();
            loop {
                line.clear();
                if r.read_line(&mut line).unwrap_or(0) == 0 || !line.starts_with("P6") {
                    break;
                }
                line.clear();
                if r.read_line(&mut line).unwrap_or(0) == 0 {
                    break;
                }
                let mut it = line.split_whitespace().map(|x| x.parse::<usize>().unwrap_or(0));
                let (w, h) = (it.next().unwrap_or(0), it.next().unwrap_or(0));
                line.clear();
                let _ = r.read_line(&mut line); // valor máximo
                if w == 0 || h == 0 || w > 8192 || h > 8192 {
                    break;
                }
                let mut buf = vec![0u8; w * h * 3];
                if r.read_exact(&mut buf).is_err() {
                    break;
                }
                if shared.epoch.load(Ordering::SeqCst) != g {
                    return;
                }
                *lock(&shared.frame) = Some(egui::ColorImage::from_rgb([w, h], &buf));
                ctx.request_repaint();
            }
            if shared.epoch.load(Ordering::SeqCst) == g {
                shared.ended.store(g, Ordering::SeqCst);
            }
            ctx.request_repaint();
        });
        if !self.muted {
            self.audio = spawn_audio(&self.input, pos);
        }
        self.base = pos;
        self.started = Some(Instant::now());
    }

    pub fn toggle(&mut self) {
        if self.playing() {
            self.base = self.position();
            self.started = None;
            self.stop_procs();
        } else {
            let at = if self.duration > 0.0 && self.base >= self.duration - 0.2 { 0.0 } else { self.base };
            self.start(at);
        }
    }

    pub fn seek_to(&mut self, secs: f64) {
        let max = if self.duration > 0.5 { self.duration - 0.3 } else { f64::MAX };
        let secs = secs.clamp(0.0, max);
        if self.playing() {
            self.start(secs);
        } else {
            // en pausa: un solo fotograma en esa posición
            self.start(secs);
            self.base = secs;
            self.started = None;
            if let Some(mut a) = self.audio.take() {
                let _ = a.kill();
                std::thread::spawn(move || {
                    let _ = a.wait();
                });
            }
            let g = self.shared.epoch.load(Ordering::SeqCst);
            let (shared, video) = (self.shared.clone(), self.video.take());
            if let Some(mut v) = video {
                std::thread::spawn(move || {
                    let end = Instant::now() + Duration::from_secs(5);
                    while Instant::now() < end && lock(&shared.frame).is_none() && shared.epoch.load(Ordering::SeqCst) == g {
                        std::thread::sleep(Duration::from_millis(10));
                    }
                    let _ = v.kill();
                    let _ = v.wait();
                });
            }
        }
    }

    pub fn set_muted(&mut self, m: bool) {
        self.muted = m;
        if m {
            if let Some(mut a) = self.audio.take() {
                let _ = a.kill();
                std::thread::spawn(move || {
                    let _ = a.wait();
                });
            }
        } else if self.playing() && have_ffplay() {
            let p = self.position();
            self.start(p);
        }
    }

    /// Sube el último fotograma recibido; al terminar vuelve a empezar (las referencias se ven en bucle).
    pub fn poll(&mut self) {
        let f = lock(&self.shared.frame).take();
        if let Some(img) = f {
            let opts = egui::TextureOptions { mipmap_mode: Some(egui::TextureFilter::Linear), ..egui::TextureOptions::LINEAR };
            match &mut self.tex {
                Some(t) => t.set(img, opts),
                None => self.tex = Some(self.ctx.load_texture("video", img, opts)),
            }
        }
        let g = self.shared.epoch.load(Ordering::SeqCst);
        if self.started.is_some() && self.shared.ended.load(Ordering::SeqCst) == g {
            self.start(0.0);
        }
        if self.started.is_some() {
            // el reloj de la barra avanza aunque no llegue un fotograma nuevo
            self.ctx.request_repaint_after(Duration::from_millis(250));
        }
    }
}

fn spawn_audio(input: &str, pos: f64) -> Option<Child> {
    if !have_ffplay() {
        return None;
    }
    let mut a = Command::new("ffplay");
    quiet(&mut a)
        .args(["-nodisp", "-vn", "-autoexit", "-loglevel", "quiet", "-ss", &format!("{pos:.3}"), "-protocol_whitelist", WHITELIST, "-i", input])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    a.spawn().ok()
}

impl Drop for Player {
    fn drop(&mut self) {
        self.stop_procs();
    }
}

pub fn fmt_time(s: f64) -> String {
    let s = s.max(0.0) as u64;
    if s >= 3600 { format!("{}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60) } else { format!("{}:{:02}", s / 60, s % 60) }
}

// ---------------------------------------------------------------------------------------------
// Documentos
// ---------------------------------------------------------------------------------------------

fn fonts() -> apex_view::Fonts {
    static F: OnceLock<apex_view::Fonts> = OnceLock::new();
    F.get_or_init(apex_view::Fonts::system).clone()
}

fn open_doc(b: &[u8]) -> Result<apex_view::Document, String> {
    apex_view::Document::open_with(b.to_vec(), apex_view::Limits::default(), fonts()).map_err(|e| match e {
        apex_view::Error::Encrypted => t("documento protegido con contraseña").to_string(),
        apex_view::Error::TooLarge => t("documento demasiado grande").to_string(),
        _ => t("no se pudo abrir el documento").to_string(),
    })
}

pub struct DocInfo {
    pub pages: u32,
    pub w: f32,
    pub h: f32,
}

pub fn probe_doc(b: &[u8]) -> Result<DocInfo, String> {
    let mut d = open_doc(b)?;
    let pages = d.page_count() as u32;
    if pages == 0 {
        return Err(t("el documento no tiene páginas").into());
    }
    let (w, h) = d.page_size(0).map_err(|_| t("página ilegible").to_string())?;
    if !(w > 0.0 && h > 0.0) {
        return Err(t("página vacía").into());
    }
    Ok(DocInfo { pages, w, h })
}

thread_local! {
    /// documentos abiertos por este hilo (el primero, el último usado)
    static OPEN: std::cell::RefCell<Vec<(u64, apex_view::Document)>> = const { std::cell::RefCell::new(Vec::new()) };
}

/// Dibuja la página con su lado mayor en `side` px. Devuelve también el tamaño de la página (puntos).
pub fn render_page(src: u64, bytes: &Bytes, page: u32, side: u32) -> Option<(Decoded, (f32, f32))> {
    OPEN.with(|open| {
        let mut open = open.borrow_mut();
        let i = match open.iter().position(|(id, _)| *id == src) {
            Some(i) => i,
            None => {
                let d = open_doc(bytes.as_slice()).ok()?;
                open.insert(0, (src, d));
                open.truncate(3);
                0
            }
        };
        let e = open.remove(i);
        open.insert(0, e);
        let doc = &mut open[0].1;
        let (pw, ph) = doc.page_size(page as usize).ok()?;
        if !(pw > 0.0 && ph > 0.0) {
            return None;
        }
        let width = if ph > pw { (side as f32 * pw / ph).round().max(8.0) as u32 } else { side.max(8) };
        let img = doc.render_page(page as usize, width, &|| false).ok()?;
        Some((Decoded { w: img.width as usize, h: img.height as usize, rgba: img.rgba }, (pw, ph)))
    })
}

// ---------------------------------------------------------------------------------------------
// Enlaces web
// ---------------------------------------------------------------------------------------------

/// Tramos (en caracteres) de las direcciones web de un texto: `http://`, `https://` y `www.`.
pub fn url_ranges(s: &str) -> Vec<(usize, usize, String)> {
    let chars: Vec<char> = s.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let starts = |p: &str| chars[i..].iter().take(p.len()).collect::<String>().eq_ignore_ascii_case(p);
        let boundary = i == 0 || !chars[i - 1].is_alphanumeric();
        if boundary && (starts("http://") || starts("https://") || starts("www.")) {
            let mut j = i;
            while j < chars.len() && !chars[j].is_whitespace() && !matches!(chars[j], '"' | '<' | '>' | '«' | '»') {
                j += 1;
            }
            // la puntuación final no forma parte del enlace
            while j > i && matches!(chars[j - 1], '.' | ',' | ';' | ':' | '!' | '?' | ')' | ']' | '\'') {
                j -= 1;
            }
            let url: String = chars[i..j].iter().collect();
            if url.len() > 4 {
                let full = if url.to_ascii_lowercase().starts_with("www.") { format!("https://{url}") } else { url };
                out.push((i, j, full));
            }
            i = j.max(i + 1);
        } else {
            i += 1;
        }
    }
    out
}

/// Abre una dirección con el navegador del sistema. Sólo http(s) y mailto: nada de archivos ni
/// esquemas que ejecuten cosas.
pub fn open_url(url: &str) -> Result<(), String> {
    let l = url.to_ascii_lowercase();
    if !(l.starts_with("http://") || l.starts_with("https://") || l.starts_with("mailto:")) {
        return Err(crate::i18n::tf("Enlace no permitido: {}", &[&url]));
    }
    let mut c = if cfg!(target_os = "macos") {
        Command::new("open")
    } else if cfg!(windows) {
        let mut c = Command::new("rundll32");
        c.arg("url.dll,FileProtocolHandler");
        c
    } else {
        Command::new("xdg-open")
    };
    quiet(&mut c).arg(url).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    let mut child = c.spawn().map_err(|e| e.to_string())?;
    std::thread::spawn(move || {
        let _ = child.wait();
    });
    Ok(())
}

/// Enlaces de una página de documento (fracciones de la página). Abre el documento aparte.
pub fn doc_links(bytes: &Bytes, page: u32) -> Vec<apex_view::Link> {
    open_doc(bytes.as_slice()).ok().and_then(|mut d| d.page_links(page as usize).ok()).unwrap_or_default()
}

/// Textos de los módulos traídos de File Browser: pasan por la traducción como allí.
pub fn t(s: &'static str) -> &'static str {
    crate::i18n::t(s)
}

/// Abre sólo un archivo normal: una tubería con nombre dejaría bloqueado el hilo que lo lee.
pub fn open_regular(path: &Path) -> Option<std::fs::File> {
    if !std::fs::metadata(path).map(|m| m.is_file()).unwrap_or(false) {
        return None;
    }
    #[cfg(unix)]
    let f = {
        use std::os::unix::fs::OpenOptionsExt;
        std::fs::OpenOptions::new().read(true).custom_flags(libc::O_NONBLOCK).open(path).ok()?
    };
    #[cfg(not(unix))]
    let f = std::fs::File::open(path).ok()?;
    f.metadata().ok().filter(|m| m.is_file()).map(|_| f)
}

// ---------------------------------------------------------------------------------------------
// Modelos 3D (visor de File Browser: STL, OBJ, PLY, glTF/GLB, 3MF)
// ---------------------------------------------------------------------------------------------

pub const MODEL_EXTS: &[&str] = &["stl", "obj", "ply", "glb", "gltf", "3mf"];

pub fn is_model_path(p: &Path) -> bool {
    ext_in(p, MODEL_EXTS)
}

/// Carga un modelo incrustado: el cargador lee de un archivo (glTF y 3MF por ruta), así que se
/// usa el original si está mapeado entero, o una copia temporal con la extensión correcta.
pub fn load_model(bytes: &Bytes, src: u64, ext: &str) -> Result<crate::mesh3d::Mesh, String> {
    let ext = ext.to_ascii_lowercase();
    if let Some((path, 0, len)) = bytes.file_window()
        && std::fs::metadata(path).map(|m| m.len() as usize == len).unwrap_or(false) && path.extension().is_some_and(|e| e.eq_ignore_ascii_case(&ext)) {
            return crate::mesh3d::load(path);
        }
    let dir = std::env::temp_dir().join(format!("mosaic-{}", std::process::id()));
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let p = dir.join(format!("{src:016x}.{ext}"));
    if !p.exists() {
        std::fs::write(&p, bytes.as_slice()).map_err(|e| e.to_string())?;
    }
    crate::mesh3d::load(&p)
}

/// Ruta temporal con el nombre original, para arrastrar un elemento a otra aplicación.
pub fn export_temp(name: &str, src: u64, bytes: &Bytes) -> Option<PathBuf> {
    let dir = std::env::temp_dir().join(format!("mosaic-{}", std::process::id())).join(format!("{src:016x}"));
    std::fs::create_dir_all(&dir).ok()?;
    let clean: String = name.chars().map(|c| if c == '/' || c == '\\' || c == '\0' { '_' } else { c }).collect();
    let clean = if clean.trim().is_empty() { "referencia.png".to_string() } else { clean };
    let p = dir.join(clean);
    if !p.exists() {
        std::fs::write(&p, bytes.as_slice()).ok()?;
    }
    Some(p)
}

/// Borra los temporales de esta sesión (al salir).
pub fn cleanup_temp() {
    let _ = std::fs::remove_dir_all(std::env::temp_dir().join(format!("mosaic-{}", std::process::id())));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn urls_in_text() {
        let v = url_ranges("ver https://ejemplo.com/a?b=1, y www.foo.org. Fin");
        assert_eq!(v.iter().map(|u| u.2.as_str()).collect::<Vec<_>>(), ["https://ejemplo.com/a?b=1", "https://www.foo.org"]);
        // tramos en caracteres (no bytes), sin la puntuación final
        let s = "ñandú http://x.es)";
        let u = &url_ranges(s)[0];
        assert_eq!(s.chars().skip(u.0).take(u.1 - u.0).collect::<String>(), "http://x.es");
        assert!(url_ranges("sin enlaces aquí").is_empty());
        assert!(url_ranges("mihttp://no").is_empty());
        // sólo se abren esquemas web
        assert!(open_url("file:///etc/passwd").is_err());
        assert!(open_url("javascript:alert(1)").is_err());
    }

    #[test]
    fn ffmpeg_output_is_parsed() {
        let s = "  Duration: 00:01:02.50, start: 0.0\n  Stream #0:0[0x1](und): Video: h264 (High) (avc1 / 0x31637661), yuv420p(tv, bt709, progressive), 1920x1080 [SAR 1:1 DAR 16:9], 30 fps\n";
        assert!((parse_duration(s) - 62.5).abs() < 1e-3);
        assert_eq!(parse_video_size(s), Some((1920, 1080)));
        assert_eq!(parse_video_size("Stream #0:0: Video: vp9, yuv420p, 640x360, SAR 1:1"), Some((640, 360)));
        assert_eq!(parse_video_size("nada"), None);
        assert_eq!(fmt_time(3723.0), "1:02:03");
        assert_eq!(fmt_time(65.0), "1:05");
    }
}
