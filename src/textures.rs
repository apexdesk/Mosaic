//! Motor de texturas con niveles de detalle (LOD).
//!
//! Hereda el diseño del servicio de miniaturas de File Browser y lo lleva a un lienzo con zoom:
//!
//! - **Niveles**: cada imagen se sube a la GPU a 256, 512, 1024… px (lado mayor) hasta su tamaño
//!   real, y sólo se pide el nivel que corresponde a lo que ocupa en pantalla. Un tablero con 500
//!   fotos de 24 MP no carga 48 GB de RGBA: carga lo que se ve, a la resolución a la que se ve.
//! - **Cola «lo visible primero»** que se reemplaza en cada fotograma: al desplazarse o hacer zoom
//!   rápido, el trabajo que ya no se ve se descarta antes de empezar. Primero las miniaturas de lo
//!   visible (aparece algo enseguida), luego los niveles altos, luego lo que está justo fuera.
//! - **Pool de hilos** con pila amplia; JPEG con escalado DCT; pánicos contenidos.
//! - **Mientras llega el nivel pedido se pinta el mejor disponible**: nunca hay parpadeo.
//! - **Mipmaps en la GPU** (glow) y filtrado lineal: suavizado correcto al alejar, gratis.
//! - **Presupuesto de VRAM con LRU**: se desalojan primero los niveles grandes que no se pintaron
//!   en este fotograma; las miniaturas se conservan mientras quepan.
//! - **Subida a la GPU limitada por fotograma**, para que soltar 300 imágenes no congele la ventana.
use crate::decode::{self, Bytes, Decoded, lock};
use std::collections::{HashMap, HashSet};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Condvar, Mutex};

/// Lado mayor del nivel 0 (miniatura).
pub const BASE: u32 = 256;
/// Nivel máximo que se considera (256 << 7 = 32768).
const MAX_LEVEL: u8 = 7;
/// Bytes de RGBA que se suben a la GPU como mucho en un fotograma.
const UPLOAD_PER_FRAME: usize = 96 << 20;

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Key {
    pub src: u64,
    /// página (documentos); 0 en lo demás
    pub page: u32,
    pub level: u8,
    pub gray: bool,
}

/// Qué hay que decodificar.
pub enum Job {
    /// imagen codificada (también el póster de un vídeo)
    Image(Bytes),
    /// página de un documento, dibujada a su tamaño
    Doc(Bytes),
}

pub struct Request {
    pub key: Key,
    pub job: Job,
    /// lado mayor que se decodifica (ver `level_side`)
    pub side: u32,
}

struct Done {
    key: Key,
    img: Decoded,
    /// tamaño de la página en puntos (documentos)
    page_size: Option<(f32, f32)>,
}

/// Página de documento cuyo tamaño se acaba de conocer.
pub struct PageSize {
    pub src: u64,
    pub page: u32,
    pub size: (f32, f32),
}

#[derive(Default)]
struct State {
    /// el último es el más prioritario
    queue: Vec<Request>,
    inflight: HashSet<Key>,
}

struct Shared {
    st: Mutex<State>,
    cv: Condvar,
}

struct Entry {
    tex: egui::TextureHandle,
    bytes: usize,
    last: u64,
}

pub struct Textures {
    shared: Arc<Shared>,
    rx: Receiver<Result<Vec<Done>, Key>>,
    cache: HashMap<Key, Entry>,
    /// decodificadas, esperando turno para subir a la GPU
    pending: Vec<Done>,
    failed: HashSet<Key>,
    frame: u64,
    vram: usize,
    pub budget: usize,
    max_side: u32,
    /// claves pedidas en el último fotograma (lo pendiente que no esté aquí se descarta)
    wanted: HashSet<Key>,
    was_busy: bool,
}

/// Lado mayor (en píxeles) del nivel `level` para una imagen de lado mayor `native`.
pub fn level_side(level: u8, native: u32, max_side: u32) -> u32 {
    (BASE << level).min(native).min(max_side).max(1)
}

/// Último nivel útil: el primero que ya alcanza el tamaño real (o el máximo de la GPU).
pub fn top_level(native: u32, max_side: u32) -> u8 {
    let cap = native.min(max_side);
    let mut l = 0u8;
    while l < MAX_LEVEL && (BASE << l) < cap {
        l += 1;
    }
    l
}

/// Nivel para un tamaño en pantalla (px físicos del lado mayor).
pub fn level_for(screen_px: f32, native: u32, max_side: u32) -> u8 {
    let mut l = 0u8;
    while l < MAX_LEVEL && ((BASE << l) as f32) < screen_px {
        l += 1;
    }
    l.min(top_level(native, max_side))
}

impl Textures {
    pub fn new(ctx: &egui::Context, budget: usize) -> Self {
        decode::install_panic_hook();
        let shared = Arc::new(Shared { st: Mutex::new(State::default()), cv: Condvar::new() });
        let (tx, rx) = channel();
        let n = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4).clamp(2, 8);
        for i in 0..n {
            let (sh, tx, ctx): (Arc<Shared>, Sender<_>, egui::Context) = (shared.clone(), tx.clone(), ctx.clone());
            let _ = std::thread::Builder::new().name(format!("decode-{i}")).stack_size(16 << 20).spawn(move || worker(sh, tx, ctx));
        }
        Textures { shared, rx, cache: HashMap::new(), pending: Vec::new(), failed: HashSet::new(), frame: 0, vram: 0, budget, max_side: 8192, wanted: HashSet::new(), was_busy: false }
    }

    pub fn max_side(&self) -> u32 {
        self.max_side
    }

    pub fn vram(&self) -> usize {
        self.vram
    }

    pub fn busy(&self) -> bool {
        let st = lock(&self.shared.st);
        !st.queue.is_empty() || !st.inflight.is_empty() || !self.pending.is_empty()
    }

    /// Recoge lo decodificado y lo sube a la GPU (con límite por fotograma). Llamar una vez por fotograma.
    pub fn pump(&mut self, ctx: &egui::Context) -> Vec<PageSize> {
        let mut sizes = Vec::new();
        self.frame += 1;
        self.max_side = ctx.input(|i| i.max_texture_side).clamp(2048, 32768) as u32;
        while let Ok(r) = self.rx.try_recv() {
            match r {
                Ok(v) => {
                    if let Some(d) = v.first() {
                        lock(&self.shared.st).inflight.remove(&d.key);
                        if let Some(size) = d.page_size {
                            sizes.push(PageSize { src: d.key.src, page: d.key.page, size });
                        }
                    }
                    self.pending.extend(v);
                }
                Err(k) => {
                    lock(&self.shared.st).inflight.remove(&k);
                    self.failed.insert(k);
                }
            }
        }
        // lo decodificado que ya no se pide (se hizo zoom o se movió la vista) no se sube: memoria
        // que se libera enseguida en vez de esperar turno
        if !self.pending.is_empty() {
            let wanted = &self.wanted;
            self.pending.retain(|d| d.key.level == 0 || wanted.contains(&d.key));
        }
        // las miniaturas primero: son baratas y lo que más se nota
        self.pending.sort_by_key(|d| std::cmp::Reverse(d.key.level));
        let opts = egui::TextureOptions { mipmap_mode: Some(egui::TextureFilter::Linear), ..egui::TextureOptions::LINEAR };
        let mut sent = 0usize;
        while let Some(d) = self.pending.pop() {
            let sz = d.img.rgba.len();
            if sent > 0 && sent + sz > UPLOAD_PER_FRAME {
                self.pending.push(d);
                ctx.request_repaint();
                break;
            }
            sent += sz;
            let img = egui::ColorImage::from_rgba_unmultiplied([d.img.w, d.img.h], &d.img.rgba);
            let tex = ctx.load_texture(format!("{:x}/{}/{}", d.key.src, d.key.page, d.key.level), img, opts);
            // con mipmaps la textura ocupa ~4/3
            let bytes = sz + sz / 3;
            if let Some(old) = self.cache.insert(d.key, Entry { tex, bytes, last: self.frame }) {
                self.vram -= old.bytes;
            }
            self.vram += bytes;
        }
        self.evict();
        // al terminar una ráfaga de decodificación, devolver al sistema lo que quedó libre
        let busy = self.busy();
        if self.was_busy && !busy {
            decode::trim_heap();
        }
        self.was_busy = busy;
        sizes
    }

    fn evict(&mut self) {
        if self.vram <= self.budget {
            return;
        }
        // candidatos: lo que no se ha pintado en este fotograma ni en el anterior; antes los niveles
        // grandes que las miniaturas, y entre iguales el más antiguo
        let f = self.frame;
        let mut v: Vec<(Key, u64, usize)> = self.cache.iter().filter(|(_, e)| e.last + 1 < f).map(|(k, e)| (*k, e.last, e.bytes)).collect();
        v.sort_unstable_by_key(|(k, last, _)| (k.level == 0, *last, std::cmp::Reverse(k.level)));
        for (k, _, b) in v {
            if self.vram <= self.budget * 9 / 10 {
                break;
            }
            self.cache.remove(&k);
            self.vram -= b;
        }
    }

    /// Mejor textura disponible para `want`: el propio nivel, uno menor (el más cercano) o uno mayor.
    pub fn best(&mut self, src: u64, page: u32, gray: bool, want: u8) -> Option<(egui::TextureId, u8)> {
        let f = self.frame;
        let order = (0..=want).rev().chain(want + 1..=MAX_LEVEL);
        for level in order {
            if let Some(e) = self.cache.get_mut(&Key { src, page, level, gray }) {
                e.last = f;
                return Some((e.tex.id(), level));
            }
        }
        None
    }

    pub fn has(&self, k: &Key) -> bool {
        self.cache.contains_key(k)
    }

    pub fn failed(&self, k: &Key) -> bool {
        self.failed.contains(k)
    }

    /// Sustituye la cola de trabajo (ordenada de más a menos prioritario).
    pub fn set_wanted(&mut self, wanted: Vec<Request>) {
        self.wanted = wanted.iter().map(|r| r.key).collect();
        let mut st = lock(&self.shared.st);
        let mut seen = HashSet::new();
        let mut q: Vec<Request> = wanted
            .into_iter()
            .filter(|r| !self.cache.contains_key(&r.key) && !self.failed.contains(&r.key) && !st.inflight.contains(&r.key) && seen.insert(r.key))
            .filter(|r| !self.pending.iter().any(|d| d.key == r.key))
            .collect();
        q.reverse();
        let had = st.queue.len();
        st.queue = q;
        if !st.queue.is_empty() || had > 0 {
            self.shared.cv.notify_all();
        }
    }

    /// Olvida todo (al abrir otro tablero).
    pub fn clear(&mut self) {
        lock(&self.shared.st).queue.clear();
        self.cache.clear();
        self.pending.clear();
        self.failed.clear();
        self.vram = 0;
    }
}

fn worker(sh: Arc<Shared>, tx: Sender<Result<Vec<Done>, Key>>, ctx: egui::Context) {
    loop {
        let req = {
            let mut st = lock(&sh.st);
            loop {
                if let Some(r) = st.queue.pop() {
                    st.inflight.insert(r.key);
                    break r;
                }
                st = sh.cv.wait(st).unwrap_or_else(|e| e.into_inner());
            }
        };
        let k = req.key;
        let side = req.side;
        let out = decode::guarded(|| {
            let (mut img, page_size) = match &req.job {
                Job::Image(b) => (decode::decode(b.as_slice(), side, false)?, None),
                Job::Doc(b) => {
                    let (img, ps) = crate::media::render_page(k.src, b, k.page, side)?;
                    (img, Some(ps))
                }
            };
            if k.gray {
                decode::to_gray(&mut img.rgba);
            }
            let mut v = Vec::with_capacity(2);
            // de paso, la miniatura (si se decodificó algo grande, reducirlo cuesta poco)
            if k.level > 0
                && let Some(t) = decode::downscale(&img, BASE)
            {
                v.push(Done { key: Key { level: 0, ..k }, img: t, page_size });
            }
            v.insert(0, Done { key: k, img, page_size });
            Some(v)
        })
        .flatten();
        if tx.send(out.ok_or(k)).is_err() {
            return;
        }
        ctx.request_repaint();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn levels() {
        assert_eq!(level_for(10.0, 6000, 16384), 0);
        assert_eq!(level_for(300.0, 6000, 16384), 1);
        assert_eq!(level_for(1500.0, 6000, 16384), 3);
        // nunca por encima del tamaño real
        assert_eq!(level_for(99999.0, 6000, 16384), top_level(6000, 16384));
        assert_eq!(level_side(top_level(6000, 16384), 6000, 16384), 6000);
        // ni del máximo de la GPU
        assert_eq!(level_side(top_level(40000, 8192), 40000, 8192), 8192);
        assert_eq!(top_level(100, 8192), 0);
    }
}
