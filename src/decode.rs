//! Decodificación de imágenes desde memoria (bytes incrustados en el tablero o mapeados del disco).
//!
//! Igual que en File Browser: JPEG con escalado DCT (1/2–1/8, la imagen pequeña cuesta una fracción
//! de la grande), orientación EXIF, límites de memoria, y los pánicos de un decodificador se atrapan
//! para perder sólo esa imagen.
use std::io::Cursor;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

/// Archivo mapeado en memoria (un tablero `.refs` o un vídeo importado): no ocupa RAM propia.
pub struct MapFile {
    pub map: memmap2::Mmap,
    pub path: std::path::PathBuf,
}

impl MapFile {
    pub fn open(path: &std::path::Path) -> std::io::Result<Arc<MapFile>> {
        let f = std::fs::File::open(path)?;
        // SAFETY: se trata como de sólo lectura; si otro programa lo trunca mientras está abierto el
        // proceso podría recibir SIGBUS, el mismo riesgo que asume cualquier visor con mmap.
        let map = unsafe { memmap2::Mmap::map(&f)? };
        Ok(Arc::new(MapFile { map, path: path.to_path_buf() }))
    }
}

/// Bytes codificados de una imagen, vídeo o documento: propios o una ventana de un archivo mapeado.
#[derive(Clone)]
pub enum Bytes {
    Owned(Arc<[u8]>),
    Mapped(Arc<MapFile>, usize, usize),
}

/// Caché de esta sesión en disco (~/.cache/refs-engine/<pid>): lo importado se copia aquí y se
/// mapea en vez de leerse a la RAM. Copiar (y no mapear el original) evita que el proceso caiga si
/// otro programa trunca el archivo mientras está abierto.
pub fn session_cache() -> std::path::PathBuf {
    let base = std::env::var_os("XDG_CACHE_HOME")
        .map(std::path::PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| std::path::PathBuf::from(h).join(".cache")))
        .unwrap_or_else(std::env::temp_dir);
    base.join("mosaic").join(std::process::id().to_string())
}

/// Borra la caché de esta sesión y la de sesiones anteriores que ya no se están ejecutando.
pub fn cleanup_caches(this_session: bool) {
    let Some(root) = session_cache().parent().map(|p| p.to_path_buf()) else { return };
    let Ok(rd) = std::fs::read_dir(&root) else { return };
    let me = std::process::id().to_string();
    for e in rd.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        let alive = std::path::Path::new("/proc").join(&name).exists();
        if (name == me && this_session) || (name != me && name.parse::<u32>().is_ok() && !alive && cfg!(target_os = "linux")) {
            let _ = std::fs::remove_dir_all(e.path());
        }
    }
}

static SPILL_N: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Copia un archivo a la caché de la sesión y lo mapea.
pub fn spill_file(src: &std::path::Path) -> std::io::Result<Bytes> {
    let dir = session_cache();
    std::fs::create_dir_all(&dir)?;
    let ext = src.extension().and_then(|e| e.to_str()).unwrap_or("bin");
    let p = dir.join(format!("{}.{ext}", SPILL_N.fetch_add(1, Ordering::Relaxed)));
    std::fs::copy(src, &p)?;
    Ok(Bytes::whole(MapFile::open(&p)?))
}

/// Escribe bytes (pegados o descargados) a la caché de la sesión y los mapea; si no se puede, en RAM.
pub fn spill_bytes(b: Vec<u8>, ext: &str) -> Bytes {
    let r = (|| -> std::io::Result<Bytes> {
        let dir = session_cache();
        std::fs::create_dir_all(&dir)?;
        let p = dir.join(format!("{}.{ext}", SPILL_N.fetch_add(1, Ordering::Relaxed)));
        std::fs::write(&p, &b)?;
        Ok(Bytes::whole(MapFile::open(&p)?))
    })();
    r.unwrap_or_else(|_| Bytes::Owned(Arc::from(b)))
}

/// Ajustes del asignador de glibc: con 8 hilos de decodificación, cada uno con su arena, los
/// búferes grandes fragmentan el montón y la memoria no vuelve al sistema.
pub fn tune_allocator() {
    #[cfg(all(target_os = "linux", target_env = "gnu"))]
    // SAFETY: llamadas de configuración de glibc, al arrancar
    unsafe {
        libc::mallopt(libc::M_ARENA_MAX, 2);
        libc::mallopt(libc::M_MMAP_THRESHOLD, 1 << 20);
    }
}

/// Devuelve al sistema la memoria libre del montón (tras una ráfaga de decodificación).
pub fn trim_heap() {
    #[cfg(all(target_os = "linux", target_env = "gnu"))]
    // SAFETY: sin requisitos
    unsafe {
        libc::malloc_trim(0);
    }
}

impl Bytes {
    pub fn as_slice(&self) -> &[u8] {
        match self {
            Bytes::Owned(b) => b,
            Bytes::Mapped(m, off, len) => &m.map[*off..*off + *len],
        }
    }

    pub fn len(&self) -> usize {
        match self {
            Bytes::Owned(b) => b.len(),
            Bytes::Mapped(_, _, len) => *len,
        }
    }

    /// Archivo entero mapeado.
    pub fn whole(m: Arc<MapFile>) -> Bytes {
        let len = m.map.len();
        Bytes::Mapped(m, 0, len)
    }

    /// Dónde puede leerlo un programa externo (ffmpeg): `(ruta, desplazamiento, longitud)` si está
    /// en un archivo; `None` si sólo está en memoria.
    pub fn file_window(&self) -> Option<(&std::path::Path, usize, usize)> {
        match self {
            Bytes::Mapped(m, off, len) => Some((&m.path, *off, *len)),
            Bytes::Owned(_) => None,
        }
    }
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn is_mapped(&self) -> bool {
        matches!(self, Bytes::Mapped(..))
    }
    /// Copia a memoria propia (antes de sobrescribir el archivo mapeado).
    pub fn to_owned_bytes(&self) -> Bytes {
        match self {
            Bytes::Owned(_) => self.clone(),
            Bytes::Mapped(..) => Bytes::Owned(Arc::from(self.as_slice())),
        }
    }
}

pub struct Decoded {
    pub w: usize,
    pub h: usize,
    pub rgba: Vec<u8>,
}

// ---------------------------------------------------------------------------------------------
// Contención de fallos
// ---------------------------------------------------------------------------------------------

thread_local! {
    static GUARDED: std::cell::Cell<u32> = const { std::cell::Cell::new(0) };
}

pub fn guarded<T>(f: impl FnOnce() -> T) -> Option<T> {
    install_panic_hook();
    let _ = GUARDED.try_with(|g| g.set(g.get() + 1));
    let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(f));
    let _ = GUARDED.try_with(|g| g.set(g.get().saturating_sub(1)));
    r.ok()
}

pub fn install_panic_hook() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        let prev = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            if GUARDED.try_with(|g| g.get()).unwrap_or(0) > 0 {
                static SHOWN: AtomicU32 = AtomicU32::new(0);
                if SHOWN.fetch_add(1, Ordering::Relaxed) < 8 {
                    let at = info.location().map(|l| format!("{}:{}", l.file(), l.line())).unwrap_or_default();
                    eprintln!("mosaic: una imagen no se pudo decodificar ({at}); se omite");
                }
                return;
            }
            prev(info);
            #[cfg(not(test))]
            std::process::abort();
        }));
    });
}

pub fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// Las decodificaciones enormes (>48 MB de RGBA) van de una en una: varias a la vez agotarían la RAM.
static HEAVY: Mutex<()> = Mutex::new(());

// ---------------------------------------------------------------------------------------------
// Tamaño sin decodificar
// ---------------------------------------------------------------------------------------------

/// Tamaño (ya orientado según EXIF) leyendo sólo la cabecera.
pub fn probe(b: &[u8]) -> Option<(u32, u32)> {
    let (w, h) = image::ImageReader::new(Cursor::new(b)).with_guessed_format().ok()?.into_dimensions().ok()?;
    if w == 0 || h == 0 {
        return None;
    }
    Some(if exif_orientation_in(b) >= 5 { (h, w) } else { (w, h) })
}

pub fn is_jpeg(b: &[u8]) -> bool {
    b.len() > 3 && b[0] == 0xFF && b[1] == 0xD8 && b[2] == 0xFF
}

// ---------------------------------------------------------------------------------------------
// Decodificación con el lado mayor limitado a `max`
// ---------------------------------------------------------------------------------------------

pub fn decode(b: &[u8], max: u32, gray: bool) -> Option<Decoded> {
    let mut d = if is_jpeg(b) { decode_jpeg(b, max).or_else(|| decode_generic(b, max))? } else { decode_generic(b, max)? };
    if gray {
        to_gray(&mut d.rgba);
    }
    Some(d)
}

fn decode_generic(b: &[u8], max: u32) -> Option<Decoded> {
    let r = image::ImageReader::new(Cursor::new(b)).with_guessed_format().ok()?;
    let (w, h) = r.into_dimensions().ok()?;
    let mut lim = image::Limits::default();
    lim.max_image_width = Some(65535);
    lim.max_image_height = Some(65535);
    lim.max_alloc = Some(2 << 30);
    let _heavy = (w as u64 * h as u64 * 4 > 48 << 20).then(|| lock(&HEAVY));
    let mut r = image::ImageReader::new(Cursor::new(b)).with_guessed_format().ok()?;
    r.limits(lim);
    let img = r.decode().ok()?;
    let img = if img.width() > max || img.height() > max { img.thumbnail(max, max) } else { img };
    let mut rgba = img.into_rgba8();
    let o = exif_orientation_in(b);
    if o > 1 {
        rgba = orient(rgba, o);
    }
    Some(Decoded { w: rgba.width() as usize, h: rgba.height() as usize, rgba: rgba.into_raw() })
}

fn decode_jpeg(b: &[u8], max: u32) -> Option<Decoded> {
    let orientation = exif_orientation_in(b);
    let mut dec = jpeg_decoder::Decoder::new(Cursor::new(b));
    dec.read_info().ok()?;
    let info = dec.info()?;
    let (w0, h0) = (info.width as u32, info.height as u32);
    if w0 as u64 * h0 as u64 > 400_000_000 {
        return None;
    }
    let _heavy = (w0 as u64 * h0 as u64 * 4 > 48 << 20 && w0.max(h0) <= max * 2).then(|| lock(&HEAVY));
    if w0 > max || h0 > max {
        // escalado en el dominio DCT: decodifica directamente a 1/2, 1/4 o 1/8
        let s = (max as f32 / w0.max(h0) as f32).min(1.0);
        let _ = dec.scale((w0 as f32 * s).ceil().max(1.0) as u16, (h0 as f32 * s).ceil().max(1.0) as u16);
    }
    let px = dec.decode().ok()?;
    let info = dec.info()?;
    let (w, h) = (info.width as u32, info.height as u32);
    let rgb: Vec<u8> = match info.pixel_format {
        jpeg_decoder::PixelFormat::RGB24 => px,
        jpeg_decoder::PixelFormat::L8 => px.iter().flat_map(|&v| [v, v, v]).collect(),
        jpeg_decoder::PixelFormat::CMYK32 => px
            .as_chunks::<4>().0.iter()
            .flat_map(|c| {
                let k = c[3] as u32;
                [(c[0] as u32 * k / 255) as u8, (c[1] as u32 * k / 255) as u8, (c[2] as u32 * k / 255) as u8]
            })
            .collect(),
        _ => return None,
    };
    let img = image::RgbImage::from_raw(w, h, rgb)?;
    let img = if w > max || h > max {
        let s = max as f32 / w.max(h) as f32;
        let (nw, nh) = (((w as f32 * s).round() as u32).max(1), ((h as f32 * s).round() as u32).max(1));
        image::imageops::thumbnail(&img, nw, nh)
    } else {
        img
    };
    let rgba = image::DynamicImage::ImageRgb8(img).into_rgba8();
    let rgba = orient(rgba, orientation);
    Some(Decoded { w: rgba.width() as usize, h: rgba.height() as usize, rgba: rgba.into_raw() })
}

/// Reduce una imagen ya decodificada (para generar la miniatura a partir de una decodificación grande).
pub fn downscale(d: &Decoded, max: u32) -> Option<Decoded> {
    if d.w as u32 <= max && d.h as u32 <= max {
        return None;
    }
    // prestado, sin copiar el nivel grande (hasta 64 MB en un 4096²)
    let img = image::ImageBuffer::<image::Rgba<u8>, &[u8]>::from_raw(d.w as u32, d.h as u32, d.rgba.as_slice())?;
    let s = max as f32 / d.w.max(d.h) as f32;
    let (nw, nh) = (((d.w as f32 * s).round() as u32).max(1), ((d.h as f32 * s).round() as u32).max(1));
    let small = image::imageops::thumbnail(&img, nw, nh);
    Some(Decoded { w: small.width() as usize, h: small.height() as usize, rgba: small.into_raw() })
}

pub fn to_gray(rgba: &mut [u8]) {
    for p in rgba.as_chunks_mut::<4>().0 {
        // luminancia Rec.709 en enteros
        let y = ((p[0] as u32 * 54 + p[1] as u32 * 183 + p[2] as u32 * 19) >> 8) as u8;
        p[0] = y;
        p[1] = y;
        p[2] = y;
    }
}

// ---------------------------------------------------------------------------------------------
// EXIF
// ---------------------------------------------------------------------------------------------

/// Orientación EXIF (1..8) a partir del principio de un JPEG; 1 si no hay.
pub fn exif_orientation_in(b: &[u8]) -> u8 {
    let n = b.len().min(256 * 1024);
    let b = &b[..n];
    if n < 12 || b[0] != 0xFF || b[1] != 0xD8 {
        return 1;
    }
    let mut i = 2;
    while i + 4 < n {
        if b[i] != 0xFF {
            return 1;
        }
        let marker = b[i + 1];
        let len = u16::from_be_bytes([b[i + 2], b[i + 3]]) as usize;
        if marker == 0xE1 && i + 10 < n && &b[i + 4..i + 10] == b"Exif\0\0" {
            let t = i + 10;
            if t + 8 > n {
                return 1;
            }
            let le = &b[t..t + 2] == b"II";
            let u16_at = |o: usize| -> Option<u16> {
                let s = b.get(o..o.checked_add(2)?)?;
                Some(if le { u16::from_le_bytes([s[0], s[1]]) } else { u16::from_be_bytes([s[0], s[1]]) })
            };
            let u32_at = |o: usize| -> Option<u32> {
                let s = b.get(o..o.checked_add(4)?)?;
                Some(if le { u32::from_le_bytes([s[0], s[1], s[2], s[3]]) } else { u32::from_be_bytes([s[0], s[1], s[2], s[3]]) })
            };
            let Some(ifd) = u32_at(t + 4) else { return 1 };
            let Some(base) = usize::try_from(ifd).ok().and_then(|v| t.checked_add(v)) else { return 1 };
            let Some(cnt) = u16_at(base) else { return 1 };
            for k in 0..cnt as usize {
                let Some(e) = base.checked_add(2 + k * 12) else { return 1 };
                if u16_at(e) == Some(0x0112) {
                    return e.checked_add(8).and_then(u16_at).filter(|v| (1..=8).contains(v)).map(|v| v as u8).unwrap_or(1);
                }
            }
            return 1;
        }
        if marker == 0xDA {
            return 1;
        }
        i += 2 + len;
    }
    1
}

fn orient(img: image::RgbaImage, o: u8) -> image::RgbaImage {
    use image::imageops::{flip_horizontal, flip_vertical, rotate180, rotate270, rotate90};
    match o {
        2 => flip_horizontal(&img),
        3 => rotate180(&img),
        4 => flip_vertical(&img),
        5 => flip_horizontal(&rotate90(&img)),
        6 => rotate90(&img),
        7 => flip_horizontal(&rotate270(&img)),
        8 => rotate270(&img),
        _ => img,
    }
}

/// Codifica RGBA como PNG (imágenes pegadas desde el portapapeles).
pub fn encode_png(w: u32, h: u32, rgba: &[u8]) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    let enc = image::codecs::png::PngEncoder::new_with_quality(&mut out, image::codecs::png::CompressionType::Fast, image::codecs::png::FilterType::Adaptive);
    image::ImageEncoder::write_image(enc, rgba, w, h, image::ExtendedColorType::Rgba8).ok()?;
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn png(w: u32, h: u32) -> Vec<u8> {
        let rgba: Vec<u8> = (0..w * h).flat_map(|i| [(i % 251) as u8, 100, 200, 255]).collect();
        encode_png(w, h, &rgba).unwrap()
    }

    #[test]
    fn probe_and_decode_with_limit() {
        let b = png(300, 120);
        assert_eq!(probe(&b), Some((300, 120)));
        let d = decode(&b, 100, false).unwrap();
        assert_eq!((d.w, d.h), (100, 40));
        let d = decode(&b, 1000, true).unwrap();
        assert_eq!((d.w, d.h), (300, 120));
        assert!(d.rgba.chunks_exact(4).all(|p| p[0] == p[1] && p[1] == p[2]));
    }

    #[test]
    fn garbage_is_rejected_without_panic() {
        assert!(probe(b"no soy una imagen").is_none());
        assert!(guarded(|| decode(&[0xFF, 0xD8, 0xFF, 0, 1, 2, 3], 64, false)).flatten().is_none());
    }
}
