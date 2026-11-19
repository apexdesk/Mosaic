//! Importación en segundo plano: imágenes, vídeos, documentos, modelos 3D, carpetas (recursivas),
//! URL del navegador y portapapeles. Nada bloquea la ventana: cada elemento llega por un canal en
//! cuanto se sabe su tamaño y aparece con su hueco reservado.
use crate::board::{Kind, Source, content_id};
use crate::decode::{self, Bytes, MapFile};
use crate::i18n::{t, tf};
use crate::media;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc::Sender;

pub struct Imported {
    pub id: u64,
    pub src: Source,
    /// modelo 3D ya cargado (para no leerlo dos veces)
    pub mesh: Option<Arc<crate::mesh3d::Mesh>>,
    /// lote al que pertenece (lo de un mismo soltar se coloca junto)
    pub batch: u64,
}

pub enum Msg {
    Item(Box<Imported>),
    /// un tablero `.refs` soltado o pasado por línea de órdenes
    Board(PathBuf),
    Error(String),
    BatchDone(u64, usize),
}

const IMAGE_EXTS: &[&str] = &["jpg", "jpeg", "jpe", "png", "gif", "webp", "bmp", "tif", "tiff", "ico", "tga"];
/// Un archivo mayor que esto no se incrusta (salvo vídeos, que se mapean).
const MAX_FILE: u64 = 512 << 20;
const MAX_VIDEO: u64 = 8 << 30;
#[cfg(feature = "web")]
const MAX_DOWNLOAD: u64 = 256 << 20;
/// Marco de un modelo 3D en el mundo.
pub const MODEL_FRAME: (u32, u32) = (1000, 800);
/// Tarjeta del mini reproductor de audio en el mundo.
pub const AUDIO_CARD: (u32, u32) = (720, 170);

fn ext_of(p: &Path) -> String {
    p.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase()
}

pub fn is_image_path(p: &Path) -> bool {
    IMAGE_EXTS.contains(&ext_of(p).as_str())
}

pub fn is_board_path(p: &Path) -> bool {
    ext_of(p) == "refs"
}

fn importable(p: &Path) -> bool {
    is_image_path(p) || is_board_path(p) || media::is_video_path(p) || media::is_doc_path(p) || media::is_model_path(p) || crate::audio::is_audio_path(p)
}

pub fn is_url(s: &str) -> bool {
    s.starts_with("http://") || s.starts_with("https://")
}

fn collect(p: &Path, out: &mut Vec<PathBuf>, depth: u32) {
    if p.to_str().is_some_and(is_url) {
        out.push(p.to_path_buf());
    } else if p.is_dir() {
        if depth > 16 {
            return;
        }
        let Ok(rd) = std::fs::read_dir(p) else { return };
        let mut v: Vec<PathBuf> = rd.flatten().map(|e| e.path()).collect();
        v.sort();
        for c in v {
            collect(&c, out, depth + 1);
        }
    } else if importable(p) {
        out.push(p.to_path_buf());
    }
}

/// Imagen a partir de sus bytes.
pub fn image_from_bytes(bytes: Vec<u8>, name: String, batch: u64) -> Result<Imported, String> {
    let (w, h) = decode::probe(&bytes).ok_or_else(|| tf("{}: formato no reconocido", &[&name]))?;
    let id = content_id(&bytes);
    let ext = Path::new(&name).extension().and_then(|e| e.to_str()).unwrap_or("png").to_string();
    Ok(Imported { id, src: Source::new(decode::spill_bytes(bytes, &ext), w, h, name, Kind::Image), mesh: None, batch })
}

/// Imagen de un archivo: copiada a la caché de la sesión y mapeada (no ocupa RAM propia).
fn image_from_file(f: &Path, name: String, batch: u64) -> Result<Imported, String> {
    let bytes = decode::spill_file(f).map_err(|e| e.to_string())?;
    let (w, h) = decode::probe(bytes.as_slice()).ok_or_else(|| tf("{}: formato no reconocido", &[&name]))?;
    let id = content_id(bytes.as_slice());
    Ok(Imported { id, src: Source::new(bytes, w, h, name, Kind::Image), mesh: None, batch })
}

fn doc_from_bytes(bytes: Vec<u8>, name: String, batch: u64) -> Result<Imported, String> {
    let info = media::probe_doc(&bytes)?;
    let id = content_id(&bytes);
    let (w, h) = ((info.w * media::DOC_SCALE).round() as u32, (info.h * media::DOC_SCALE).round() as u32);
    let ext = Path::new(&name).extension().and_then(|e| e.to_str()).unwrap_or("pdf").to_string();
    let mut src = Source::new(decode::spill_bytes(bytes, &ext), w.max(1), h.max(1), name, Kind::Doc);
    src.pages = info.pages;
    src.page_sizes.insert(0, (info.w, info.h));
    Ok(Imported { id, src, mesh: None, batch })
}

fn import_file(f: &Path, batch: u64) -> Result<Imported, String> {
    let name = f.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    let md = std::fs::metadata(f).map_err(|e| e.to_string())?;
    if !md.is_file() {
        return Err(t("no es un archivo normal").into());
    }
    if media::is_video_path(f) {
        if md.len() > MAX_VIDEO {
            return Err(t("vídeo demasiado grande").into());
        }
        // el vídeo no se lee: se mapea (al guardar se copia al tablero sin pasar por la RAM)
        let map = MapFile::open(f).map_err(|e| e.to_string())?;
        let info = media::probe_video(f.to_str().ok_or(t("ruta no válida"))?)?;
        let bytes = Bytes::whole(map);
        let id = content_id(bytes.as_slice());
        let mut src = Source::new(bytes, info.w, info.h, name, Kind::Video);
        src.poster = Some(Bytes::Owned(Arc::from(info.poster_png)));
        src.duration = info.duration;
        return Ok(Imported { id, src, mesh: None, batch });
    }
    if md.len() > MAX_FILE {
        return Err(t("archivo demasiado grande").into());
    }
    if crate::audio::is_audio_path(f) {
        let bytes = decode::spill_file(f).map_err(|e| e.to_string())?;
        let ext = f.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
        let (duration, peaks) = decode::guarded(|| crate::audio::probe(&bytes, &ext, 160)).ok_or(t("el audio no se pudo leer"))??;
        let id = content_id(bytes.as_slice());
        let mut src = Source::new(bytes, AUDIO_CARD.0, AUDIO_CARD.1, name, Kind::Audio);
        src.duration = duration as f32;
        src.peaks = peaks;
        return Ok(Imported { id, src, mesh: None, batch });
    }
    if media::is_model_path(f) {
        let mesh = decode::guarded(|| crate::mesh3d::load(f)).ok_or(t("el modelo no se pudo leer"))??;
        let bytes = decode::spill_file(f).map_err(|e| e.to_string())?;
        let id = content_id(bytes.as_slice());
        let src = Source::new(bytes, MODEL_FRAME.0, MODEL_FRAME.1, name, Kind::Model);
        return Ok(Imported { id, src, mesh: Some(Arc::new(mesh)), batch });
    }
    if media::is_doc_path(f) {
        let bytes = std::fs::read(f).map_err(|e| e.to_string())?;
        return decode::guarded(|| doc_from_bytes(bytes, name, batch)).unwrap_or_else(|| Err(t("el documento no se pudo leer").into()));
    }
    image_from_file(f, name, batch)
}

#[cfg(feature = "web")]
fn download(url: &str) -> Result<Vec<u8>, String> {
    let resp = ureq::get(url).timeout(std::time::Duration::from_secs(30)).call().map_err(|e| e.to_string())?;
    let mut bytes = Vec::new();
    std::io::Read::read_to_end(&mut std::io::Read::take(resp.into_reader(), MAX_DOWNLOAD), &mut bytes).map_err(|e| e.to_string())?;
    Ok(bytes)
}

/// Sin la feature `web` no se descarga nada.
#[cfg(not(feature = "web"))]
fn download(_url: &str) -> Result<Vec<u8>, String> {
    Err(t("la dirección no es una imagen").into())
}

/// Descarga una URL (imagen arrastrada o pegada desde el navegador).
fn import_url(url: &str, batch: u64) -> Result<Imported, String> {
    let bytes = download(url)?;
    let tail = url.split(['?', '#']).next().unwrap_or(url).rsplit('/').next().unwrap_or("");
    let name = if tail.is_empty() { "descargada.png".to_string() } else { tail.to_string() };
    if decode::probe(&bytes).is_some() {
        return image_from_bytes(bytes, name, batch);
    }
    if media::is_doc_path(Path::new(&name)) || bytes.starts_with(b"%PDF") {
        return doc_from_bytes(bytes, name, batch);
    }
    Err(t("la dirección no es una imagen").into())
}

/// Importa rutas (archivos, carpetas o URL) en un hilo aparte.
pub fn spawn_paths(paths: Vec<PathBuf>, batch: u64, tx: Sender<Msg>, ctx: egui::Context) {
    std::thread::spawn(move || {
        let mut files = Vec::new();
        for p in &paths {
            collect(p, &mut files, 0);
        }
        let mut n = 0;
        for f in files {
            if is_board_path(&f) {
                let _ = tx.send(Msg::Board(f));
                continue;
            }
            let shown = f.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| f.display().to_string());
            let r = match f.to_str().filter(|s| is_url(s)) {
                Some(u) => import_url(u, batch),
                None => import_file(&f, batch),
            };
            let _ = tx.send(match r {
                Ok(i) => {
                    n += 1;
                    Msg::Item(Box::new(i))
                }
                Err(e) => Msg::Error(format!("{shown}: {e}")),
            });
            ctx.request_repaint();
        }
        let _ = tx.send(Msg::BatchDone(batch, n));
        ctx.request_repaint();
    });
}

/// Rutas o URL en un texto pegado (una por línea, admite `file://`).
pub fn paths_in_text(t: &str) -> Vec<PathBuf> {
    t.lines()
        .map(|l| l.trim())
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .filter_map(|l| {
            if is_url(l) && !l.contains(char::is_whitespace) {
                return Some(PathBuf::from(l));
            }
            let s = l.strip_prefix("file://").map(percent_decode).unwrap_or_else(|| l.to_string());
            let p = PathBuf::from(s);
            (p.is_absolute() && p.exists()).then_some(p)
        })
        .collect()
}

fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%'
            && i + 2 < b.len()
            && let Some(v) = std::str::from_utf8(&b[i + 1..i + 3]).ok().and_then(|h| u8::from_str_radix(h, 16).ok())
        {
            out.push(v);
            i += 3;
            continue;
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Imagen del portapapeles (en un hilo: en Wayland/X11 puede tardar si la ofrece otro programa).
pub fn spawn_clipboard_image(batch: u64, tx: Sender<Msg>, ctx: egui::Context) {
    std::thread::spawn(move || {
        let r = (|| -> Result<Imported, String> {
            let mut cb = arboard::Clipboard::new().map_err(|e| e.to_string())?;
            let img = cb.get_image().map_err(|_| t("el portapapeles no contiene una imagen").to_string())?;
            let png = decode::encode_png(img.width as u32, img.height as u32, &img.bytes).ok_or(t("no se pudo codificar la imagen pegada"))?;
            image_from_bytes(png, "pegada.png".into(), batch)
        })();
        let n = r.is_ok() as usize;
        let _ = tx.send(match r {
            Ok(i) => Msg::Item(Box::new(i)),
            Err(e) => Msg::Error(e),
        });
        let _ = tx.send(Msg::BatchDone(batch, n));
        ctx.request_repaint();
    });
}

/// Copia una imagen (a su resolución completa) al portapapeles.
pub fn spawn_copy_image(bytes: Bytes, tx: Sender<Msg>, ctx: egui::Context) {
    std::thread::spawn(move || {
        let r = decode::guarded(|| decode::decode(bytes.as_slice(), u32::MAX, false)).flatten();
        let res = match r {
            Some(d) => arboard::Clipboard::new()
                .and_then(|mut cb| cb.set_image(arboard::ImageData { width: d.w, height: d.h, bytes: d.rgba.into() }))
                .map_err(|e| e.to_string()),
            None => Err(t("no se pudo decodificar").into()),
        };
        if let Err(e) = res {
            let _ = tx.send(Msg::Error(tf("Copiar: {}", &[&e])));
            ctx.request_repaint();
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uri_lists() {
        assert_eq!(percent_decode("/a%20b/c%C3%B1.png"), "/a b/cñ.png");
        assert_eq!(percent_decode("/x%2"), "/x%2");
        let tmp = std::env::temp_dir();
        let t = format!("# comentario\nfile://{}\nno/absoluta\nhttps://example.com/a.png\n", tmp.display());
        assert_eq!(paths_in_text(&t), vec![tmp, PathBuf::from("https://example.com/a.png")]);
    }

    #[test]
    fn kinds_by_extension() {
        assert!(importable(Path::new("/a/b.MP4")));
        assert!(importable(Path::new("/a/b.pdf")));
        assert!(importable(Path::new("/a/b.glb")));
        assert!(!importable(Path::new("/a/b.txt")));
    }
}
