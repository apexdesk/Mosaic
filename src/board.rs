//! Modelo del tablero y formato de archivo `.refs`.
//!
//! El archivo es autocontenido: guarda los bytes originales de cada imagen, vídeo,
//! documento o modelo 3D, sin recomprimir. Al abrirlo se mapea en memoria y todo se lee
//! directamente del mapa: abrir un tablero de varios GB es instantáneo y sólo se lee lo que se ve.
//! Los textos y los trazos a mano alzada viven en la cabecera.
//!
//! ```text
//! "REFSENG\0" | versión u32 LE | longitud JSON u64 LE | JSON (vista, fuentes, elementos) | bytes
//! ```
use crate::decode::{Bytes, MapFile};
use egui::{Pos2, Vec2, pos2, vec2};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::hash::Hasher;
use std::io::{BufWriter, Write};
use std::path::Path;

const MAGIC: &[u8; 8] = b"REFSENG\0";
const VERSION: u32 = 2;

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    #[default]
    Image,
    Video,
    Doc,
    Model,
    Audio,
}

/// Archivo original (deduplicado por contenido: pegar dos veces lo mismo no lo guarda dos veces).
pub struct Source {
    pub bytes: Bytes,
    /// Tamaño base en el mundo: píxeles (imagen ya orientada, vídeo), puntos × 2 de la primera
    /// página (documento) o un marco fijo (modelo 3D).
    pub w: u32,
    pub h: u32,
    pub name: String,
    pub kind: Kind,
    /// fotograma de portada de un vídeo (PNG)
    pub poster: Option<Bytes>,
    pub pages: u32,
    pub duration: f32,
    /// tamaño (puntos) de las páginas ya dibujadas; se conoce al dibujarlas
    pub page_sizes: HashMap<u32, (f32, f32)>,
    /// forma de onda de un audio (picos 0..255)
    pub peaks: Vec<u8>,
}

impl Source {
    pub fn new(bytes: Bytes, w: u32, h: u32, name: String, kind: Kind) -> Source {
        Source { bytes, w, h, name, kind, poster: None, pages: 0, duration: 0.0, page_sizes: HashMap::new(), peaks: Vec::new() }
    }

    /// Lado mayor que se llega a decodificar (el último nivel de detalle).
    pub fn native(&self) -> u32 {
        match self.kind {
            Kind::Doc => crate::media::DOC_NATIVE,
            Kind::Video => self.poster.as_ref().and_then(|p| crate::decode::probe(p.as_slice())).map(|(w, h)| w.max(h)).unwrap_or(1024),
            _ => self.w.max(self.h),
        }
    }

    pub fn ext(&self) -> String {
        Path::new(&self.name).extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase()
    }
}

/// Texto escrito en el tablero.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Text {
    pub s: String,
    /// tamaño de letra en el mundo (antes de la escala del elemento)
    pub size: f32,
    pub color: [u8; 4],
    #[serde(default)]
    pub bg: Option<[u8; 4]>,
    /// caja medida (mundo, a escala 1); la mide la interfaz al maquetar
    #[serde(default)]
    pub w: f32,
    #[serde(default)]
    pub h: f32,
    // formato (para todo el texto)
    #[serde(default)]
    pub bold: bool,
    #[serde(default)]
    pub italic: bool,
    #[serde(default)]
    pub underline: bool,
    #[serde(default)]
    pub strike: bool,
    /// letra de ancho fijo
    #[serde(default)]
    pub mono: bool,
    /// 0 izquierda, 1 centro, 2 derecha
    #[serde(default)]
    pub align: u8,
}

impl Text {
    pub fn new(s: String, size: f32, color: [u8; 4]) -> Text {
        Text { s, size, color, bg: None, w: size * 0.5, h: size, bold: false, italic: false, underline: false, strike: false, mono: false, align: 0 }
    }
}

/// Marco que agrupa elementos: lo que queda dentro se mueve con él.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct FrameBox {
    /// tamaño en el mundo (a escala 1)
    pub w: f32,
    pub h: f32,
    pub title: String,
    pub color: [u8; 4],
}

/// Trazo a mano alzada: puntos relativos al centro, sin escalar.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Ink {
    pub pts: Vec<[f32; 2]>,
    pub width: f32,
    pub color: [u8; 4],
    pub w: f32,
    pub h: f32,
}

impl Ink {
    /// Crea el trazo a partir de puntos del mundo; devuelve también su centro.
    pub fn from_world(pts: &[Pos2], width: f32, color: [u8; 4]) -> (Ink, Pos2) {
        let r = egui::Rect::from_points(pts);
        let c = r.center();
        let pts = pts.iter().map(|p| [p.x - c.x, p.y - c.y]).collect();
        (Ink { pts, width, color, w: r.width(), h: r.height() }, c)
    }
}

/// Un elemento colocado en el tablero.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Item {
    pub id: u64,
    /// fuente (imagen, vídeo, documento o modelo); 0 en textos y trazos
    #[serde(default)]
    pub src: u64,
    /// centro, en coordenadas del mundo
    pub pos: [f32; 2],
    pub scale: f32,
    /// radianes
    #[serde(default)]
    pub rot: f32,
    #[serde(default)]
    pub flip: bool,
    #[serde(default)]
    pub gray: bool,
    /// página mostrada de un documento
    #[serde(default)]
    pub page: u32,
    /// cámara del visor 3D: giro, inclinación, zoom
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cam3d: Option<[f32; 3]>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<Text>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ink: Option<Ink>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub frame: Option<FrameBox>,
    /// elemento al que va unido: al moverse aquel, se mueve éste
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent: Option<u64>,
    /// recorte no destructivo: [x0, y0, x1, y1] en fracciones de la imagen (sin girar ni voltear)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub crop: Option<[f32; 4]>,
    /// visor 3D en proyección ortográfica (por defecto, perspectiva)
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub ortho3d: bool,
}

impl Item {
    pub fn new(id: u64, src: u64, pos: Pos2, scale: f32) -> Item {
        Item { id, src, pos: [pos.x, pos.y], scale, rot: 0.0, flip: false, gray: false, page: 0, cam3d: None, text: None, ink: None, frame: None, parent: None, crop: None, ortho3d: false }
    }
    pub fn center(&self) -> Pos2 {
        pos2(self.pos[0], self.pos[1])
    }
    pub fn set_center(&mut self, p: Pos2) {
        self.pos = [p.x, p.y];
    }
    pub fn is_media(&self) -> bool {
        self.text.is_none() && self.ink.is_none() && self.frame.is_none()
    }
}

#[derive(Serialize, Deserialize, Clone, Copy)]
pub struct View {
    pub cam: [f32; 2],
    pub zoom: f32,
    /// cuadrícula de fondo
    #[serde(default)]
    pub grid: bool,
    /// tema «blueprint» (fondo azul de plano)
    #[serde(default)]
    pub blueprint: bool,
}

#[derive(Serialize, Deserialize)]
struct SrcMeta {
    id: u64,
    off: u64,
    len: u64,
    w: u32,
    h: u32,
    #[serde(default)]
    name: String,
    #[serde(default)]
    kind: Kind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    poster: Option<(u64, u64)>,
    #[serde(default)]
    pages: u32,
    #[serde(default)]
    duration: f32,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    peaks: Vec<u8>,
}

#[derive(Serialize, Deserialize)]
struct Header {
    view: View,
    sources: Vec<SrcMeta>,
    items: Vec<Item>,
    #[serde(default)]
    bg: Option<[u8; 3]>,
}

#[derive(Default)]
pub struct Board {
    pub sources: HashMap<u64, Source>,
    /// orden de pintado: el último queda encima
    pub items: Vec<Item>,
    next_id: u64,
}

/// Identificador por contenido. Los archivos grandes (vídeos) se resumen por muestras: tamaño,
/// principio, final y 64 trozos repartidos, para no leer gigabytes al importar.
/// Recorte de un elemento (todo si no tiene).
pub fn crop_of(it: &Item) -> [f32; 4] {
    if it.is_media() { it.crop.unwrap_or([0.0, 0.0, 1.0, 1.0]) } else { [0.0, 0.0, 1.0, 1.0] }
}

pub fn content_id(b: &[u8]) -> u64 {
    let mut h = std::hash::DefaultHasher::new();
    h.write_usize(b.len());
    const BIG: usize = 64 << 20;
    if b.len() <= BIG {
        h.write(b);
    } else {
        let chunk = 64 << 10;
        h.write(&b[..1 << 20]);
        h.write(&b[b.len() - (1 << 20)..]);
        for k in 1..64 {
            let at = b.len() / 64 * k;
            h.write(&b[at..at + chunk]);
        }
    }
    // 0 queda reservado para «sin fuente»
    h.finish().max(1)
}

impl Board {
    pub fn new_id(&mut self) -> u64 {
        self.next_id += 1;
        self.next_id
    }

    /// Tamaño de un elemento en el mundo (antes de rotar).
    pub fn size(&self, it: &Item) -> Vec2 {
        if let Some(t) = &it.text {
            return vec2(t.w.max(t.size * 0.5), t.h.max(t.size)) * it.scale;
        }
        if let Some(k) = &it.ink {
            return vec2(k.w + k.width, k.h + k.width) * it.scale;
        }
        if let Some(f) = &it.frame {
            return vec2(f.w, f.h) * it.scale;
        }
        let c = crop_of(it);
        self.full_size(it) * vec2(c[2] - c[0], c[3] - c[1])
    }

    /// Tamaño del elemento sin recortar.
    pub fn full_size(&self, it: &Item) -> Vec2 {
        match self.sources.get(&it.src) {
            Some(s) if s.kind == Kind::Doc => match s.page_sizes.get(&it.page) {
                Some((w, h)) => vec2(*w, *h) * crate::media::DOC_SCALE * it.scale,
                None => vec2(s.w as f32, s.h as f32) * it.scale,
            },
            Some(s) => vec2(s.w as f32, s.h as f32) * it.scale,
            None => vec2(100.0, 100.0) * it.scale,
        }
    }

    /// Centro (mundo) de la imagen completa, sin recortar.
    pub fn full_center(&self, it: &Item) -> Pos2 {
        let c = crop_of(it);
        let f = self.full_size(it);
        let mut off = vec2(((c[0] + c[2]) * 0.5 - 0.5) * f.x, ((c[1] + c[3]) * 0.5 - 0.5) * f.y);
        if it.flip {
            off.x = -off.x;
        }
        it.center() - egui::emath::Rot2::from_angle(it.rot) * off
    }

    /// Punto del mundo para una fracción (u, v) de la imagen completa.
    pub fn frac_to_world(&self, it: &Item, u: Vec2) -> Pos2 {
        let f = self.full_size(it);
        let mut l = (u - vec2(0.5, 0.5)) * f;
        if it.flip {
            l.x = -l.x;
        }
        self.full_center(it) + egui::emath::Rot2::from_angle(it.rot) * l
    }

    /// Fracción (u, v) de la imagen completa bajo un punto del mundo.
    pub fn world_to_frac(&self, it: &Item, p: Pos2) -> Vec2 {
        let f = self.full_size(it);
        let mut l = egui::emath::Rot2::from_angle(-it.rot) * (p - self.full_center(it));
        if it.flip {
            l.x = -l.x;
        }
        l / f + vec2(0.5, 0.5)
    }

    /// Esquinas en el mundo: arriba-izq, arriba-der, abajo-der, abajo-izq.
    pub fn corners(&self, it: &Item) -> [Pos2; 4] {
        let half = self.size(it) * 0.5;
        let rot = egui::emath::Rot2::from_angle(it.rot);
        let c = it.center();
        [vec2(-half.x, -half.y), vec2(half.x, -half.y), vec2(half.x, half.y), vec2(-half.x, half.y)].map(|v| c + rot * v)
    }

    pub fn aabb(&self, it: &Item) -> egui::Rect {
        let c = self.corners(it);
        egui::Rect::from_points(&c)
    }

    /// ¿Cae `p` sobre el elemento? En los trazos cuenta la distancia a la línea (`tol`: margen en
    /// unidades del mundo), no su caja.
    pub fn contains(&self, it: &Item, p: Pos2, tol: f32) -> bool {
        let half = self.size(it) * 0.5;
        let mut local = egui::emath::Rot2::from_angle(-it.rot) * (p - it.center());
        if local.x.abs() > half.x + tol || local.y.abs() > half.y + tol {
            return false;
        }
        let Some(k) = &it.ink else { return true };
        if it.flip {
            local.x = -local.x;
        }
        let q = local / it.scale;
        let r = k.width * 0.5 + tol / it.scale;
        let d2 = |a: [f32; 2], b: [f32; 2]| -> f32 {
            let (a, b) = (vec2(a[0], a[1]), vec2(b[0], b[1]));
            let ab = b - a;
            let t = ((q - a).dot(ab) / ab.length_sq().max(1e-9)).clamp(0.0, 1.0);
            (q - (a + ab * t)).length_sq()
        };
        if k.pts.len() == 1 {
            return d2(k.pts[0], k.pts[0]) <= r * r;
        }
        k.pts.windows(2).any(|w| d2(w[0], w[1]) <= r * r)
    }

    /// Añade una fuente (si ya existe ese contenido se reutiliza). `id` lo calcula quien importa.
    pub fn add_source(&mut self, id: u64, s: Source) -> u64 {
        self.sources.entry(id).or_insert(s);
        id
    }

    // -----------------------------------------------------------------------------------------
    // Archivo
    // -----------------------------------------------------------------------------------------

    pub fn load(path: &Path) -> Result<(Board, View, Option<[u8; 3]>), String> {
        let map = MapFile::open(path).map_err(|e| e.to_string())?;
        let m = &map.map;
        if m.len() < 20 || &m[..8] != MAGIC {
            return Err(crate::i18n::t("no es un tablero .refs").into());
        }
        let ver = u32::from_le_bytes(m[8..12].try_into().unwrap());
        if ver > VERSION {
            return Err(crate::i18n::tf("versión {} no soportada (actualiza la aplicación)", &[&ver]));
        }
        let jlen = u64::from_le_bytes(m[12..20].try_into().unwrap()) as usize;
        let jend = 20usize.checked_add(jlen).filter(|&e| e <= m.len()).ok_or(crate::i18n::t("archivo truncado"))?;
        let hdr: Header = serde_json::from_slice(&m[20..jend]).map_err(|e| crate::i18n::tf("cabecera dañada: {}", &[&e]))?;
        let total = m.len() as u64;
        let window = |off: u64, len: u64| -> Option<Bytes> {
            let off = (jend as u64).checked_add(off)?;
            (off.checked_add(len)? <= total).then(|| Bytes::Mapped(map.clone(), off as usize, len as usize))
        };
        let mut b = Board::default();
        for s in hdr.sources {
            let Some(bytes) = window(s.off, s.len) else {
                eprintln!("mosaic: fuente {} fuera del archivo; se omite", s.id);
                continue;
            };
            let mut src = Source::new(bytes, s.w, s.h, s.name, s.kind);
            src.poster = s.poster.and_then(|(o, l)| window(o, l));
            src.pages = s.pages;
            src.duration = s.duration;
            src.peaks = s.peaks;
            b.sources.insert(s.id, src);
        }
        b.items = hdr
            .items
            .into_iter()
            .filter(|i| (!i.is_media() || b.sources.contains_key(&i.src)) && i.scale.is_finite() && i.scale > 0.0)
            .collect();
        b.next_id = b.items.iter().map(|i| i.id).max().unwrap_or(0);
        Ok((b, hdr.view, hdr.bg))
    }

    /// Guarda de forma atómica (archivo temporal + renombrar) y vuelve a mapear el resultado, de modo
    /// que lo pegado o importado deja de ocupar RAM y se lee del tablero.
    pub fn save(&mut self, path: &Path, view: View, bg: [u8; 3]) -> Result<(), String> {
        let mut ids: Vec<u64> = Vec::new();
        let mut seen = std::collections::HashSet::new();
        for it in &self.items {
            if it.is_media() && self.sources.contains_key(&it.src) && seen.insert(it.src) {
                ids.push(it.src);
            }
        }
        // orden de escritura: (fuente, ¿póster?)
        let mut blobs: Vec<(u64, bool)> = Vec::new();
        let mut metas = Vec::with_capacity(ids.len());
        let mut off = 0u64;
        for id in &ids {
            let s = &self.sources[id];
            let len = s.bytes.len() as u64;
            let mut meta = SrcMeta { id: *id, off, len, w: s.w, h: s.h, name: s.name.clone(), kind: s.kind, poster: None, pages: s.pages, duration: s.duration, peaks: s.peaks.clone() };
            blobs.push((*id, false));
            off += len;
            if let Some(p) = &s.poster {
                meta.poster = Some((off, p.len() as u64));
                blobs.push((*id, true));
                off += p.len() as u64;
            }
            metas.push(meta);
        }
        let hdr = Header { view, sources: metas, items: self.items.clone(), bg: Some(bg) };
        let json = serde_json::to_vec(&hdr).map_err(|e| e.to_string())?;

        // en Windows no se puede reemplazar un archivo mapeado: lo que venga de él se copia antes
        if cfg!(windows) {
            for s in self.sources.values_mut() {
                if s.bytes.file_window().is_some_and(|(p, _, _)| p == path) {
                    s.bytes = s.bytes.to_owned_bytes();
                }
                if let Some(p) = &mut s.poster {
                    *p = p.to_owned_bytes();
                }
            }
        }
        let tmp = path.with_extension("refs.tmp");
        let write = || -> std::io::Result<()> {
            let f = std::fs::File::create(&tmp)?;
            let mut w = BufWriter::with_capacity(1 << 20, f);
            w.write_all(MAGIC)?;
            w.write_all(&VERSION.to_le_bytes())?;
            w.write_all(&(json.len() as u64).to_le_bytes())?;
            w.write_all(&json)?;
            for (id, poster) in &blobs {
                let s = &self.sources[id];
                let b = if *poster { s.poster.as_ref().unwrap() } else { &s.bytes };
                w.write_all(b.as_slice())?;
            }
            let f = w.into_inner().map_err(|e| e.into_error())?;
            f.sync_all()?;
            Ok(())
        };
        if let Err(e) = write() {
            let _ = std::fs::remove_file(&tmp);
            return Err(e.to_string());
        }
        std::fs::rename(&tmp, path).map_err(|e| e.to_string())?;

        // volver a mapear: los bytes pasan a leerse del archivo recién escrito
        if let Ok(map) = MapFile::open(path) {
            let base = 20 + json.len();
            for m in &hdr.sources {
                if let Some(s) = self.sources.get_mut(&m.id) {
                    s.bytes = Bytes::Mapped(map.clone(), base + m.off as usize, m.len as usize);
                    if let (Some((o, l)), Some(p)) = (m.poster, s.poster.as_mut()) {
                        *p = Bytes::Mapped(map.clone(), base + o as usize, l as usize);
                    }
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    fn owned(b: &[u8]) -> Bytes {
        Bytes::Owned(Arc::from(b))
    }

    #[test]
    fn save_and_load_roundtrip() {
        let dir = std::env::temp_dir().join(format!("refs-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("t.refs");
        let png = crate::decode::encode_png(4, 2, &[255u8; 32]).unwrap();
        let mut b = Board::default();
        let id = content_id(&png);
        let src = b.add_source(id, Source::new(owned(&png), 4, 2, "a.png".into(), Kind::Image));
        // la misma imagen otra vez no duplica la fuente
        assert_eq!(b.add_source(content_id(&png), Source::new(owned(&png), 4, 2, "b.png".into(), Kind::Image)), src);
        // un «vídeo» con póster
        let mut v = Source::new(owned(b"no es un video"), 640, 360, "v.mp4".into(), Kind::Video);
        v.poster = Some(owned(&png));
        v.duration = 3.5;
        let vid = b.add_source(77, v);
        for k in 0..3 {
            let id = b.new_id();
            let mut it = Item::new(id, src, pos2(k as f32 * 10.0, 0.0), 2.0);
            it.rot = 0.5;
            it.flip = k == 1;
            b.items.push(it);
        }
        let id = b.new_id();
        b.items.push(Item::new(id, vid, Pos2::ZERO, 1.0));
        let id = b.new_id();
        let mut t = Item::new(id, 0, Pos2::ZERO, 1.0);
        let mut tx = Text::new("hola".into(), 40.0, [255; 4]);
        tx.bold = true;
        tx.align = 1;
        t.text = Some(tx);
        b.items.push(t);
        let id = b.new_id();
        let mut k = Item::new(id, 0, Pos2::ZERO, 1.0);
        let (ink, c) = Ink::from_world(&[pos2(0.0, 0.0), pos2(10.0, 10.0)], 3.0, [0, 0, 0, 255]);
        k.ink = Some(ink);
        k.set_center(c);
        b.items.push(k);
        let id = b.new_id();
        let mut f = Item::new(id, 0, Pos2::ZERO, 1.0);
        f.frame = Some(FrameBox { w: 500.0, h: 300.0, title: "Marco".into(), color: [40, 40, 40, 255] });
        b.items.insert(0, f);

        let view = View { cam: [1.0, 2.0], zoom: 3.0, grid: true, blueprint: false };
        b.save(&p, view, [1, 2, 3]).unwrap();
        assert!(b.sources[&src].bytes.file_window().is_some());
        let (l, v, bg) = Board::load(&p).unwrap();
        assert_eq!(l.items, b.items);
        assert_eq!(l.sources[&src].bytes.as_slice(), png.as_slice());
        assert_eq!(l.sources[&vid].bytes.as_slice(), b"no es un video");
        assert_eq!(l.sources[&vid].poster.as_ref().unwrap().as_slice(), png.as_slice());
        assert_eq!((l.sources[&vid].kind, l.sources[&vid].duration), (Kind::Video, 3.5));
        assert_eq!((v.zoom, bg), (3.0, Some([1, 2, 3])));
        // guardar encima del archivo mapeado
        let mut l = l;
        l.save(&p, v, [0, 0, 0]).unwrap();
        assert_eq!(Board::load(&p).unwrap().0.items.len(), 7);
        assert!(Board::load(&dir.join("no-existe.refs")).is_err());
        std::fs::write(dir.join("x.refs"), b"REFSENG\0\x01\0\0\0\xff\xff\xff\xff\0\0\0\0").unwrap();
        assert!(Board::load(&dir.join("x.refs")).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn hit_test_rotated_and_ink() {
        let mut b = Board::default();
        let src = b.add_source(5, Source::new(owned(b"x"), 200, 20, String::new(), Kind::Image));
        let mut it = Item::new(1, src, Pos2::ZERO, 1.0);
        it.rot = std::f32::consts::FRAC_PI_2;
        assert!(b.contains(&it, pos2(0.0, 90.0), 0.0));
        assert!(!b.contains(&it, pos2(90.0, 0.0), 0.0));
        // un trazo diagonal: el centro de la caja está sobre la línea, una esquina no
        let (ink, c) = Ink::from_world(&[pos2(0.0, 0.0), pos2(100.0, 100.0)], 4.0, [0; 4]);
        let mut k = Item::new(2, 0, c, 1.0);
        k.ink = Some(ink);
        assert!(b.contains(&k, pos2(50.0, 50.0), 0.0));
        assert!(!b.contains(&k, pos2(95.0, 5.0), 1.0));
    }

    #[test]
    fn sampled_id_for_big_files() {
        let mut v = vec![0u8; (64 << 20) + 10];
        let a = content_id(&v);
        v[3] = 1;
        assert_ne!(a, content_id(&v));
    }
}
