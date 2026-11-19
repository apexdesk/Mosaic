//! Estado de la aplicación, archivos, importación, disposición y órdenes.
//! La interacción con el ratón y el pintado del lienzo están en `canvas.rs`.
//!
//! Como en File Browser, no hay repintado continuo: la ventana sólo se redibuja con entrada del
//! usuario, cuando llega una textura o un fotograma de vídeo, o durante una animación de cámara.
use crate::board::{Board, FrameBox, Item, Kind, Text, View};
use crate::import::{self, Msg};
use crate::i18n::{t, tf, tn};
use crate::media::{self, Player};
use crate::textures::Textures;
use egui::{Key as K, Modifiers, Pos2, Rect, Vec2, pos2, vec2};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc::{Receiver, Sender, channel};

pub const MIN_ZOOM: f32 = 0.002;
pub const MAX_ZOOM: f32 = 200.0;
const UNDO_CAP: usize = 200;
/// Cámara inicial del visor 3D: giro, inclinación, zoom.
pub const CAM3D: [f32; 3] = [-0.6, 0.5, 1.0];

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Cmd {
    Open,
    Save,
    SaveAs,
    Import,
    Paste,
    Copy,
    SelectAll,
    Escape,
    Delete,
    Duplicate,
    Undo,
    Redo,
    Fit,
    FitAll,
    ActualPixels,
    Arrange,
    Normalize,
    Flip,
    Gray,
    Rotate(i8),
    ResetTransform,
    Front,
    Back,
    OnTop,
    Help,
    Tool(Tool),
    PlayPause,
    Mute,
    Page(i32),
    ExpandPages,
    /// rodear la selección con un marco
    FrameSelection,
    /// alinear la selección: 0 izquierda, 1 centro horizontal, 2 derecha, 3 arriba, 4 centro
    /// vertical, 5 abajo; 6 y 7 reparten el espacio en horizontal y en vertical
    Align(u8),
    /// cuadrícula de fondo
    Grid,
    /// opacidad de la ventana: +1 / −1 (10 %)
    Opacity(i8),
    /// mostrar u ocultar elementos, zoom y VRAM
    ToggleStatus,
    /// emparentar la selección (o romper el emparentado si ya lo está)
    Link,
    Quit,
    /// perspectiva / ortográfica en los modelos 3D seleccionados
    Projection,
    /// entrar en el modo recorte (o salir aceptando)
    Crop,
    /// quitar el recorte
    CropReset,
    /// confirmar (Intro)
    Accept,
    /// tema «blueprint»
    Blueprint,
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Tool {
    Select,
    Text,
    Draw,
    Frame,
}

pub(crate) enum Drag {
    None,
    Pan,
    Move { start: Pos2, orig: Vec<(u64, Pos2)>, moved: bool, toggle_on_click: Option<u64> },
    Scale { pivot: Pos2, corner: Pos2, orig: Vec<(u64, Pos2, f32)> },
    Rotate { center: Pos2, start_angle: f32, orig: f32, id: u64 },
    Marquee { start: Pos2, additive: bool, base: HashSet<u64> },
    /// trazo a mano alzada en curso (puntos del mundo)
    Ink(Vec<Pos2>),
    /// girar un modelo 3D
    Orbit(u64),
    /// arrastrar en la barra de un vídeo
    Seek { id: u64, bar: Rect },
    /// dibujar un marco nuevo (esquina inicial, mundo)
    FrameNew(Pos2),
    /// cambiar el tamaño de un marco sin escalar su contenido
    FrameResize { id: u64, anchor: Pos2 },
    /// mover un borde del recorte (0..7) o la ventana de recorte entera (8)
    Crop { id: u64, handle: u8, start: [f32; 4], from: Vec2 },
}

/// Controles pintados sobre los elementos (se comprueban antes que los propios elementos).
#[derive(Clone, Copy)]
pub(crate) enum Control {
    PlayPause(u64),
    Seek(u64),
    Mute(u64),
    PagePrev(u64),
    PageNext(u64),
    /// cambiar entre perspectiva y ortográfica en un visor 3D
    Projection(u64),
}

pub(crate) enum Dialog {
    Open(PathBuf),
    SaveAs(PathBuf),
    Import(Vec<PathBuf>),
}

/// Colocación de un lote importado (se van poniendo en fila al llegar y se ordenan al terminar).
pub(crate) struct Batch {
    anchor: Pos2,
    x: f32,
    y: f32,
    row_h: f32,
    target_h: f32,
    ids: Vec<u64>,
}

/// Clave del dibujo por software de un modelo: tamaño y cámara.
pub(crate) type SoftKey = (usize, usize, i32, i32, i32);

pub(crate) enum MeshState {
    Loading,
    Ready { mesh: Arc<crate::mesh3d::Mesh>, gpu: crate::gpu3d::Shared, soft: Option<(SoftKey, egui::TextureHandle)> },
    Failed,
}

/// Texto en edición.
pub(crate) struct Editing {
    pub id: u64,
    pub buf: String,
    pub focus: bool,
    /// se edita el título de un marco (no un texto)
    pub title: bool,
    /// punto fijo del texto mientras se escribe (borde izquierdo, centro o borde derecho según la
    /// alineación, arriba), en el mundo: el texto crece desde ahí y no desde su centro
    pub anchor: Option<Pos2>,
}

/// Qué color se está eligiendo en el selector redondo.
#[derive(Clone, Copy, PartialEq, Debug)]
pub(crate) enum ColorTarget {
    Ink,
    TextColor(u64),
    TextBg(u64),
    Frame(u64),
    /// fondo del tablero
    Background,
}

pub struct App {
    pub(crate) board: Board,
    pub(crate) tex: Textures,
    pub(crate) cam: Pos2,
    pub(crate) zoom: f32,
    pub(crate) anim: Option<(Pos2, f32)>,
    pub(crate) sel: HashSet<u64>,
    pub(crate) drag: Drag,
    pub(crate) undo: Vec<Vec<Item>>,
    pub(crate) redo: Vec<Vec<Item>>,
    pub(crate) tx: Sender<Msg>,
    rx: Receiver<Msg>,
    dlg_tx: Sender<Dialog>,
    dlg_rx: Receiver<Dialog>,
    pub(crate) batches: HashMap<u64, Batch>,
    next_batch: u64,
    pub(crate) path: Option<PathBuf>,
    pub(crate) dirty: bool,
    title: String,
    pub(crate) toast: Option<(String, f64)>,
    pub(crate) help: bool,
    pub(crate) on_top: bool,
    pub(crate) bg: [u8; 3],
    pub(crate) confirm_close: bool,
    pub(crate) allow_close: bool,
    pub(crate) canvas: Rect,
    pub(crate) errors: Vec<String>,
    // herramientas
    pub(crate) tool: Tool,
    pub(crate) ink_color: [u8; 4],
    /// grosor del trazo en px de pantalla al dibujar
    pub(crate) ink_width: f32,
    /// tamaño del texto nuevo en px de pantalla
    pub(crate) text_size: f32,
    pub(crate) editing: Option<Editing>,
    // vídeo, 3D y controles
    pub(crate) players: HashMap<u64, Player>,
    pub(crate) meshes: HashMap<u64, MeshState>,
    mesh_tx: Sender<(u64, Result<crate::mesh3d::Mesh, String>)>,
    mesh_rx: Receiver<(u64, Result<crate::mesh3d::Mesh, String>)>,
    pub(crate) active3d: Option<u64>,
    pub(crate) controls: Vec<(Rect, Control)>,
    // arrastre hacia otras aplicaciones (y recepción en Wayland)
    pub(crate) dragout: Option<crate::dragout::DragOut>,
    dragout_tried: bool,
    pub(crate) release_inject: bool,
    pub(crate) last_ptr: Option<Pos2>,
    // ventana sin decoración y cabecera
    pub(crate) color_popup: Option<(ColorTarget, Pos2, u64)>,
    /// mover o redimensionar la ventana en curso: el lienzo no atiende al ratón
    pub(crate) win_op: bool,
    /// títulos de marco en pantalla (para hacer clic en ellos)
    pub(crate) frame_titles: Vec<(Rect, u64)>,
    pub(crate) grid: bool,
    pub(crate) blueprint: bool,
    /// preguntar cómo poner la ventana siempre encima en Wayland
    pub(crate) ontop_prompt: bool,
    pub(crate) cfg: Config,
    /// mini reproductores de los audios
    pub(crate) audio_players: HashMap<u64, crate::audio::AudioPlayer>,
    /// hasta cuándo se ven el menú, el título y los botones de ventana (se ocultan solos)
    pub(crate) chrome_until: f64,
    pub(crate) fade: crate::fade::Fade,
    /// tablero que se abrirá cuando se responda a «¿Guardar los cambios?»
    pub(crate) pending_open: Option<PathBuf>,
    /// orden en que se seleccionaron los elementos (el primero manda al emparentar)
    pub(crate) sel_order: Vec<u64>,
    /// fuente CJK ya cargada (para ver los nombres de los idiomas asiáticos en el menú)
    pub(crate) cjk_loaded: bool,
    /// elemento en modo recorte
    pub(crate) crop_mode: Option<u64>,
    /// enlaces de las páginas de documentos ya consultadas
    pub(crate) doc_links: HashMap<(u64, u32), Vec<apex_view::Link>>,
    links_tx: Sender<((u64, u32), Vec<apex_view::Link>)>,
    links_rx: Receiver<((u64, u32), Vec<apex_view::Link>)>,
}

/// Colores por defecto (también como «Por defecto» en el selector de color).
pub const DEFAULT_INK: [u8; 4] = [255, 90, 70, 255];
pub const DEFAULT_BG: [u8; 3] = [0x1e, 0x1e, 0x21];
pub const DEFAULT_FRAME: [u8; 4] = [255, 255, 255, 18];

/// Fondo «blueprint» al activar la cuadrícula.
/// Azul de plano oscuro y sobrio, de la misma luminosidad que el fondo por defecto.
pub const BLUEPRINT: [u8; 3] = [24, 38, 58];

/// Preferencias que sobreviven entre sesiones (~/.config/refs-engine/config.json).
#[derive(serde::Serialize, serde::Deserialize)]
pub struct Config {
    #[serde(default)]
    pub on_top: bool,
    /// opacidad de la ventana (transparencia)
    #[serde(default = "one")]
    pub opacity: f32,
    /// mostrar el número de elementos, el zoom y la VRAM abajo a la izquierda
    #[serde(default)]
    pub show_status: bool,
    /// idioma: "auto" (el del sistema) o un código (es, en, fr…)
    #[serde(default = "auto")]
    pub lang: String,
    /// tableros abiertos o guardados hace poco (el más reciente primero)
    #[serde(default)]
    pub recent: Vec<PathBuf>,
    /// dibujar las líneas entre elementos emparentados
    #[serde(default)]
    pub show_links: bool,
}

fn one() -> f32 {
    1.0
}

fn auto() -> String {
    "auto".into()
}

impl Default for Config {
    fn default() -> Config {
        Config { on_top: false, opacity: 1.0, show_status: false, lang: auto(), recent: Vec::new(), show_links: false }
    }
}

impl Config {
    fn path() -> Option<PathBuf> {
        let base = std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from).or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
            // Windows: %APPDATA%\mosaic
            .or_else(|| std::env::var_os("APPDATA").map(PathBuf::from))?;
        Some(base.join("mosaic").join("config.json"))
    }
    pub fn load() -> Config {
        // ajustes de cuando se llamaba refs: se copian una vez
        if let Some(p) = Config::path()
            && !p.exists()
            && let Some(old) = p.parent().and_then(|d| d.parent()).map(|b| b.join("refs-engine").join("config.json"))
            && old.exists()
        {
            let _ = std::fs::create_dir_all(p.parent().unwrap_or(&p));
            let _ = std::fs::copy(&old, &p);
        }
        Config::path().and_then(|p| std::fs::read(p).ok()).and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
    }
    pub fn save(&self) {
        if let Some(p) = Config::path() {
            let _ = std::fs::create_dir_all(p.parent().unwrap_or(&p));
            let _ = std::fs::write(p, serde_json::to_vec_pretty(self).unwrap_or_default());
        }
    }
}

/// ¿Ventana nativa de Wayland? (allí ninguna aplicación puede ponerse «siempre encima» por sí misma)
pub fn native_wayland() -> bool {
    std::env::var_os("WAYLAND_DISPLAY").is_some()
}

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>, args: Vec<PathBuf>) -> Self {
        let ctx = &cc.egui_ctx;
        ctx.set_visuals(egui::Visuals::dark());
        // idioma (el del sistema por defecto) y, si hace falta, la fuente CJK
        let lang = crate::i18n::resolve(&Config::load().lang);
        crate::i18n::set(lang);
        crate::i18n::install_fonts(ctx, lang);
        ctx.options_mut(|o| o.zoom_with_keyboard = false);
        let (tx, rx) = channel();
        let (dlg_tx, dlg_rx) = channel();
        let (mesh_tx, mesh_rx) = channel();
        let (links_tx, links_rx) = channel();
        // presupuesto de VRAM: 1.5 GB por defecto, ajustable con REFS_VRAM_MB
        let budget = std::env::var("REFS_VRAM_MB").ok().and_then(|v| v.parse::<usize>().ok()).unwrap_or(1536) << 20;
        let mut app = App {
            board: Board::default(),
            tex: Textures::new(ctx, budget),
            cam: Pos2::ZERO,
            zoom: 1.0,
            anim: None,
            sel: HashSet::new(),
            drag: Drag::None,
            undo: Vec::new(),
            redo: Vec::new(),
            tx,
            rx,
            dlg_tx,
            dlg_rx,
            batches: HashMap::new(),
            next_batch: 0,
            path: None,
            dirty: false,
            title: String::new(),
            toast: None,
            help: false,
            on_top: false,
            bg: DEFAULT_BG,
            confirm_close: false,
            allow_close: false,
            canvas: Rect::from_min_size(Pos2::ZERO, vec2(1280.0, 800.0)),
            errors: Vec::new(),
            tool: Tool::Select,
            ink_color: DEFAULT_INK,
            ink_width: 4.0,
            text_size: 28.0,
            editing: None,
            players: HashMap::new(),
            meshes: HashMap::new(),
            mesh_tx,
            mesh_rx,
            active3d: None,
            controls: Vec::new(),
            dragout: None,
            dragout_tried: false,
            release_inject: false,
            last_ptr: None,
            color_popup: None,
            win_op: false,
            frame_titles: Vec::new(),
            grid: false,
            blueprint: false,
            ontop_prompt: false,
            cfg: Config::load(),
            audio_players: HashMap::new(),
            chrome_until: 4.0,
            fade: Default::default(),
            pending_open: None,
            sel_order: Vec::new(),
            cjk_loaded: false,
            crop_mode: None,
            doc_links: HashMap::new(),
            links_tx,
            links_rx,
        };
        // reinicio en modo compatible (XWayland) para poder estar siempre encima
        if std::env::var_os("REFS_ON_TOP").is_some() {
            app.on_top = true;
        }
        let (boards, rest): (Vec<PathBuf>, Vec<PathBuf>) = args.into_iter().partition(|p| import::is_board_path(p));
        if let Some(b) = boards.first() {
            app.open_board(b.clone());
        }
        if !rest.is_empty() {
            app.import_paths(ctx, rest, None);
        }
        app
    }

    // -----------------------------------------------------------------------------------------
    // Coordenadas
    // -----------------------------------------------------------------------------------------

    pub(crate) fn to_screen(&self, w: Pos2) -> Pos2 {
        self.canvas.center() + (w - self.cam) * self.zoom
    }

    pub(crate) fn to_world(&self, s: Pos2) -> Pos2 {
        self.cam + (s - self.canvas.center()) / self.zoom
    }

    pub(crate) fn view_world(&self) -> Rect {
        Rect::from_min_max(self.to_world(self.canvas.min), self.to_world(self.canvas.max))
    }

    pub(crate) fn zoom_at(&mut self, screen: Pos2, factor: f32) {
        let before = self.to_world(screen);
        self.zoom = (self.zoom * factor).clamp(MIN_ZOOM, MAX_ZOOM);
        let after = self.to_world(screen);
        self.cam += before - after;
        self.anim = None;
    }

    pub(crate) fn fit_rect(&mut self, r: Rect, animate: bool) {
        if !r.is_finite() || r.width() <= 0.0 || r.height() <= 0.0 {
            return;
        }
        let z = (self.canvas.width() / r.width()).min(self.canvas.height() / r.height()) * 0.92;
        let z = z.clamp(MIN_ZOOM, MAX_ZOOM);
        if animate {
            self.anim = Some((r.center(), z));
        } else {
            self.cam = r.center();
            self.zoom = z;
        }
    }

    pub(crate) fn bounds(&self, only_sel: bool) -> Option<Rect> {
        let mut r: Option<Rect> = None;
        for it in &self.board.items {
            if only_sel && !self.sel.contains(&it.id) {
                continue;
            }
            let b = self.board.aabb(it);
            r = Some(r.map_or(b, |r| r.union(b)));
        }
        r
    }

    pub(crate) fn hit(&self, w: Pos2) -> Option<u64> {
        let tol = 4.0 / self.zoom;
        self.board.items.iter().rev().find(|it| self.board.contains(it, w, tol)).map(|it| it.id)
    }

    pub(crate) fn item(&self, id: u64) -> Option<&Item> {
        self.board.items.iter().find(|i| i.id == id)
    }

    pub(crate) fn item_mut(&mut self, id: u64) -> Option<&mut Item> {
        self.board.items.iter_mut().find(|i| i.id == id)
    }

    pub(crate) fn kind_of(&self, it: &Item) -> Option<Kind> {
        if !it.is_media() {
            return None;
        }
        self.board.sources.get(&it.src).map(|s| s.kind)
    }

    // -----------------------------------------------------------------------------------------
    // Historial
    // -----------------------------------------------------------------------------------------

    pub(crate) fn snapshot(&mut self) {
        self.undo.push(self.board.items.clone());
        if self.undo.len() > UNDO_CAP {
            self.undo.remove(0);
        }
        self.redo.clear();
        self.dirty = true;
    }

    fn undo(&mut self) {
        if let Some(prev) = self.undo.pop() {
            self.redo.push(std::mem::replace(&mut self.board.items, prev));
            self.after_history();
        }
    }

    fn redo(&mut self) {
        if let Some(next) = self.redo.pop() {
            self.undo.push(std::mem::replace(&mut self.board.items, next));
            self.after_history();
        }
    }

    fn after_history(&mut self) {
        let ids: HashSet<u64> = self.board.items.iter().map(|i| i.id).collect();
        self.sel.retain(|id| ids.contains(id));
        self.dirty = true;
    }

    pub(crate) fn toast(&mut self, ctx: &egui::Context, msg: impl Into<String>) {
        let t = ctx.input(|i| i.time) + 2.5;
        self.toast = Some((msg.into(), t));
        ctx.request_repaint_after(std::time::Duration::from_millis(2600));
    }

    // -----------------------------------------------------------------------------------------
    // Archivos
    // -----------------------------------------------------------------------------------------

    pub(crate) fn open_board(&mut self, p: PathBuf) {
        match Board::load(&p) {
            Ok((b, view, bg)) => {
                self.board = b;
                self.tex.clear();
                self.players.clear();
                self.meshes.clear();
                self.cam = pos2(view.cam[0], view.cam[1]);
                self.zoom = if view.zoom.is_finite() && view.zoom > 0.0 { view.zoom.clamp(MIN_ZOOM, MAX_ZOOM) } else { 1.0 };
                if let Some(bg) = bg {
                    self.bg = bg;
                }
                self.grid = view.grid;
                self.blueprint = view.blueprint;
                self.sel.clear();
                self.undo.clear();
                self.redo.clear();
                self.editing = None;
                self.active3d = None;
                self.remember_recent(&p);
                self.path = Some(p);
                self.dirty = false;
                self.anim = None;
            }
            Err(e) => self.errors.push(format!("{}: {e}", p.display())),
        }
    }

    /// Abre un tablero; si el actual tiene cambios, antes pregunta si guardarlos.
    pub(crate) fn request_open(&mut self, p: PathBuf) {
        if self.dirty && !self.board.items.is_empty() {
            self.pending_open = Some(p);
        } else {
            self.open_board(p);
        }
    }

    fn remember_recent(&mut self, p: &std::path::Path) {
        // los tableros de recuperación del modo compatible no cuentan
        if p.starts_with(std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default().join(".local/state/mosaic")) {
            return;
        }
        let p = p.canonicalize().unwrap_or_else(|_| p.to_path_buf());
        self.cfg.recent.retain(|r| *r != p);
        self.cfg.recent.insert(0, p);
        self.cfg.recent.truncate(12);
        self.cfg.save();
    }

    pub(crate) fn save_to(&mut self, ctx: &egui::Context, p: PathBuf) {
        self.commit_text();
        let p = if import::is_board_path(&p) { p } else { p.with_extension("refs") };
        let view = View { cam: [self.cam.x, self.cam.y], zoom: self.zoom, grid: self.grid, blueprint: self.blueprint };
        match self.board.save(&p, view, self.bg) {
            Ok(()) => {
                self.toast(ctx, tf("Guardado: {}", &[&p.file_name().map(|s| s.to_string_lossy()).unwrap_or_default()]));
                self.remember_recent(&p);
                self.path = Some(p);
                self.dirty = false;
            }
            Err(e) => self.errors.push(tf("No se pudo guardar: {}", &[&e])),
        }
    }

    pub(crate) fn dialog(&self, kind: u8) {
        let tx = self.dlg_tx.clone();
        let dir = self.path.as_ref().and_then(|p| p.parent().map(|d| d.to_path_buf()));
        std::thread::spawn(move || {
            let mut d = rfd::FileDialog::new();
            if let Some(dir) = dir {
                d = d.set_directory(dir);
            }
            let mut all: Vec<&str> = vec!["jpg", "jpeg", "png", "gif", "webp", "bmp", "tif", "tiff", "tga", "ico", "refs"];
            all.extend_from_slice(media::VIDEO_EXTS);
            all.extend_from_slice(media::DOC_EXTS);
            all.extend_from_slice(media::MODEL_EXTS);
            all.extend_from_slice(crate::audio::AUDIO_EXTS);
            let r = match kind {
                0 => d.add_filter(t("Tablero de referencias"), &["refs"]).pick_file().map(Dialog::Open),
                1 => d.add_filter(t("Tablero de referencias"), &["refs"]).set_file_name(format!("{}.refs", t("tablero"))).save_file().map(Dialog::SaveAs),
                _ => d.add_filter(t("Referencias"), &all).pick_files().map(Dialog::Import),
            };
            if let Some(r) = r {
                let _ = tx.send(r);
            }
        });
    }

    pub(crate) fn import_paths(&mut self, ctx: &egui::Context, paths: Vec<PathBuf>, at: Option<Pos2>) {
        let b = self.new_batch(at);
        import::spawn_paths(paths, b, self.tx.clone(), ctx.clone());
    }

    fn new_batch(&mut self, at: Option<Pos2>) -> u64 {
        self.next_batch += 1;
        let anchor = at.map(|s| self.to_world(s)).unwrap_or(self.cam);
        let vh = self.view_world().height();
        self.batches.insert(self.next_batch, Batch { anchor, x: anchor.x, y: anchor.y, row_h: 0.0, target_h: vh * 0.3, ids: Vec::new() });
        self.next_batch
    }

    pub(crate) fn receive(&mut self, ctx: &egui::Context) {
        let mut snap = false;
        while let Ok(m) = self.rx.try_recv() {
            match m {
                Msg::Item(im) => {
                    if !snap {
                        self.snapshot();
                        snap = true;
                    }
                    let im = *im;
                    let (w, h) = (im.src.w as f32, im.src.h as f32);
                    let src = self.board.add_source(im.id, im.src);
                    if let Some(mesh) = im.mesh {
                        self.meshes.entry(src).or_insert(MeshState::Ready { mesh, gpu: crate::gpu3d::shared(), soft: None });
                    }
                    let kind = self.board.sources[&src].kind;
                    let id = self.board.new_id();
                    let vw = self.view_world();
                    let zoom = self.zoom;
                    let (pos, scale) = match self.batches.get_mut(&im.batch) {
                        Some(b) => {
                            let scale = (b.target_h / h).min(1.0 / zoom.max(1e-6)).max(1e-4);
                            let (sw, sh) = (w * scale, h * scale);
                            if b.x > b.anchor.x && b.x + sw > b.anchor.x + vw.width() * 0.8 {
                                b.x = b.anchor.x;
                                b.y += b.row_h + 8.0 / zoom;
                                b.row_h = 0.0;
                            }
                            let p = pos2(b.x + sw * 0.5, b.y + sh * 0.5);
                            b.x += sw + 8.0 / zoom;
                            b.row_h = b.row_h.max(sh);
                            b.ids.push(id);
                            (p, scale)
                        }
                        None => (self.cam, 1.0),
                    };
                    let mut it = Item::new(id, src, pos, scale);
                    if kind == Kind::Model {
                        it.cam3d = Some(CAM3D);
                    }
                    self.board.items.push(it);
                }
                Msg::Board(p) => {
                    if self.board.items.is_empty() {
                        self.open_board(p);
                    } else {
                        // tablero soltado sobre otro: se añaden sus elementos
                        match Board::load(&p) {
                            Ok((b, _, _)) => {
                                if !snap {
                                    self.snapshot();
                                    snap = true;
                                }
                                let off = self.cam - b.items.first().map(|i| i.center()).unwrap_or(Pos2::ZERO);
                                for (id, s) in b.sources {
                                    self.board.add_source(id, s);
                                }
                                for mut it in b.items {
                                    it.id = self.board.new_id();
                                    it.set_center(it.center() + off);
                                    self.board.items.push(it);
                                }
                            }
                            Err(e) => self.errors.push(format!("{}: {e}", p.display())),
                        }
                    }
                }
                Msg::Error(e) => self.errors.push(e),
                Msg::BatchDone(b, n) => {
                    if let Some(b) = self.batches.remove(&b) {
                        self.finish_batch(b, n);
                    }
                    if n > 0 {
                        self.toast(ctx, tn("{} elemento añadido|{} elementos añadidos", n, &[]));
                    }
                }
            }
        }
        while let Ok(d) = self.dlg_rx.try_recv() {
            match d {
                Dialog::Open(p) => self.request_open(p),
                Dialog::SaveAs(p) => self.save_to(ctx, p),
                Dialog::Import(v) => self.import_paths(ctx, v, None),
            }
        }
        while let Ok((k, v)) = self.links_rx.try_recv() {
            self.doc_links.insert(k, v);
        }
        while let Ok((src, r)) = self.mesh_rx.try_recv() {
            let st = match r {
                Ok(m) => MeshState::Ready { mesh: Arc::new(m), gpu: crate::gpu3d::shared(), soft: None },
                Err(e) => {
                    self.errors.push(format!("3D: {e}"));
                    MeshState::Failed
                }
            };
            self.meshes.insert(src, st);
        }
    }

    /// Carga en segundo plano el modelo de una fuente (al abrir un tablero o al verse por primera vez).
    pub(crate) fn want_mesh(&mut self, ctx: &egui::Context, src: u64) {
        if self.meshes.contains_key(&src) {
            return;
        }
        let Some(s) = self.board.sources.get(&src) else { return };
        self.meshes.insert(src, MeshState::Loading);
        let (bytes, ext, tx, ctx) = (s.bytes.clone(), s.ext(), self.mesh_tx.clone(), ctx.clone());
        std::thread::spawn(move || {
            let r = crate::decode::guarded(|| media::load_model(&bytes, src, &ext)).unwrap_or_else(|| Err(t("el modelo no se pudo leer").into()));
            let _ = tx.send((src, r));
            ctx.request_repaint();
        });
    }

    /// Al terminar un lote: un elemento se centra a buen tamaño; varios se igualan en alto y se empaquetan.
    fn finish_batch(&mut self, b: Batch, n: usize) {
        if b.ids.is_empty() {
            return;
        }
        let vw = self.view_world();
        let area = vw.width() * vw.height();
        let ids: HashSet<u64> = b.ids.iter().copied().collect();
        if n == 1 && b.ids.len() == 1 {
            let id = b.ids[0];
            let Some(it) = self.item(id) else { return };
            let s = self.board.size(it) / it.scale;
            let scale = (vw.width() * 0.6 / s.x).min(vw.height() * 0.6 / s.y).min(1.0 / self.zoom);
            let anchor = b.anchor;
            if let Some(it) = self.item_mut(id) {
                it.scale = scale.max(1e-4);
                it.set_center(anchor);
            }
        } else {
            let unit: HashMap<u64, Vec2> = self.board.items.iter().filter(|i| ids.contains(&i.id)).map(|i| (i.id, self.board.size(i) / i.scale)).collect();
            let aspects: f32 = unit.values().map(|s| s.x / s.y.max(1e-6)).sum();
            let h = (area * 0.7 / aspects.max(0.01)).sqrt();
            for it in self.board.items.iter_mut().filter(|i| ids.contains(&i.id)) {
                it.scale = (h / unit[&it.id].y.max(1e-6)).max(1e-4);
            }
            self.pack(&ids, Some(b.anchor));
        }
        self.sel = ids;
    }

    // -----------------------------------------------------------------------------------------
    // Disposición
    // -----------------------------------------------------------------------------------------

    /// Empaquetado por estantes (filas), de mayor a menor alto, con un ancho de fila que tiende a
    /// un resultado más o menos cuadrado. Centrado en `center` o en el centro de lo que había.
    pub(crate) fn pack(&mut self, ids: &HashSet<u64>, center: Option<Pos2>) {
        let mut v: Vec<(u64, Rect)> = self.board.items.iter().filter(|i| ids.contains(&i.id)).map(|i| (i.id, self.board.aabb(i))).collect();
        if v.is_empty() {
            return;
        }
        let old = v.iter().fold(v[0].1, |r, (_, b)| r.union(*b));
        let center = center.unwrap_or(old.center());
        let total: f32 = v.iter().map(|(_, r)| r.area()).sum();
        let gap = v.iter().map(|(_, r)| r.height()).fold(f32::MAX, f32::min) * 0.03;
        let row_w = (total.sqrt() * 1.35).max(v.iter().map(|(_, r)| r.width()).fold(0.0, f32::max));
        v.sort_by(|a, b| b.1.height().total_cmp(&a.1.height()));
        let (mut x, mut y, mut rh) = (0.0f32, 0.0f32, 0.0f32);
        let mut placed: Vec<(u64, Pos2)> = Vec::with_capacity(v.len());
        let mut rows: Vec<(usize, f32)> = Vec::new();
        let mut row_start = 0;
        for (k, (id, r)) in v.iter().enumerate() {
            if x > 0.0 && x + r.width() > row_w {
                rows.push((row_start, x - gap));
                row_start = k;
                x = 0.0;
                y += rh + gap;
                rh = 0.0;
            }
            placed.push((*id, pos2(x + r.width() * 0.5, y + r.height() * 0.5)));
            x += r.width() + gap;
            rh = rh.max(r.height());
        }
        rows.push((row_start, x - gap));
        let total_h = y + rh;
        let max_w = rows.iter().map(|r| r.1).fold(0.0, f32::max);
        for (i, (start, w)) in rows.iter().enumerate() {
            let end = rows.get(i + 1).map(|r| r.0).unwrap_or(placed.len());
            let dx = (max_w - w) * 0.5;
            for p in &mut placed[*start..end] {
                p.1.x += dx;
            }
        }
        let off = center - pos2(max_w * 0.5, total_h * 0.5);
        for (id, p) in placed {
            if let Some(it) = self.item_mut(id) {
                it.set_center(p + off);
            }
        }
    }

    fn targets(&self) -> HashSet<u64> {
        if self.sel.is_empty() { self.board.items.iter().map(|i| i.id).collect() } else { self.sel.clone() }
    }

    // -----------------------------------------------------------------------------------------
    // Texto
    // -----------------------------------------------------------------------------------------

    /// Crea un texto vacío en `at` (mundo) y lo abre para escribir.
    pub(crate) fn new_text(&mut self, at: Pos2) {
        self.commit_text();
        self.snapshot();
        let id = self.board.new_id();
        let size = self.text_size / self.zoom;
        let mut it = Item::new(id, 0, at, 1.0);
        it.text = Some(Text::new(String::new(), size, self.ink_color));
        // la esquina superior izquierda queda donde se hizo clic
        it.set_center(at + vec2(size * 0.25, size * 0.5));
        self.board.items.push(it);
        self.sel = HashSet::from([id]);
        self.editing = Some(Editing { id, buf: String::new(), focus: true, title: false, anchor: None });
    }

    pub(crate) fn edit_text(&mut self, id: u64) {
        self.commit_text();
        let Some(s) = self.item(id).and_then(|i| i.text.as_ref()).map(|t| t.s.clone()) else { return };
        self.snapshot();
        self.sel = HashSet::from([id]);
        self.editing = Some(Editing { id, buf: s, focus: true, title: false, anchor: None });
    }

    pub(crate) fn edit_frame_title(&mut self, id: u64) {
        self.commit_text();
        let Some(t) = self.item(id).and_then(|i| i.frame.as_ref()).map(|f| f.title.clone()) else { return };
        self.snapshot();
        self.sel = HashSet::from([id]);
        self.editing = Some(Editing { id, buf: t, focus: true, title: true, anchor: None });
    }

    /// Crea un marco que ocupa `r` (mundo), debajo de todo.
    pub(crate) fn new_frame(&mut self, r: Rect) -> u64 {
        let id = self.board.new_id();
        let n = self.board.items.iter().filter(|i| i.frame.is_some()).count() + 1;
        let mut it = Item::new(id, 0, r.center(), 1.0);
        it.frame = Some(FrameBox { w: r.width().max(1.0), h: r.height().max(1.0), title: tf("Marco {}", &[&n]), color: DEFAULT_FRAME });
        self.board.items.insert(0, it);
        id
    }

    /// Los marcos siempre debajo del resto (conservando su orden y el de los demás).
    pub(crate) fn frames_to_back(&mut self) {
        self.board.items.sort_by_key(|i| i.frame.is_none());
    }

    /// Elementos que quedan dentro de un marco (su caja entera dentro del marco).
    pub(crate) fn frame_contents(&self, id: u64) -> Vec<u64> {
        let Some(f) = self.item(id) else { return Vec::new() };
        let r = self.board.aabb(f).expand(1e-3);
        self.board.items.iter().filter(|i| i.id != id && r.contains_rect(self.board.aabb(i))).map(|i| i.id).collect()
    }

    /// Termina la edición: guarda el texto (o borra el elemento si quedó vacío).
    pub(crate) fn commit_text(&mut self) {
        let Some(e) = self.editing.take() else { return };
        if e.title {
            if let Some(f) = self.item_mut(e.id).and_then(|i| i.frame.as_mut()) {
                f.title = e.buf;
            }
            self.dirty = true;
            return;
        }
        if e.buf.trim().is_empty() {
            self.board.items.retain(|i| i.id != e.id);
            self.sel.remove(&e.id);
            return;
        }
        if let Some(t) = self.item_mut(e.id).and_then(|i| i.text.as_mut()) {
            t.s = e.buf;
        }
        self.dirty = true;
    }

    // -----------------------------------------------------------------------------------------
    // Órdenes
    // -----------------------------------------------------------------------------------------

    fn selected_of(&self, kind: Kind) -> Vec<u64> {
        self.board.items.iter().filter(|i| self.sel.contains(&i.id) && self.kind_of(i) == Some(kind)).map(|i| i.id).collect()
    }

    pub(crate) fn toggle_play(&mut self, ctx: &egui::Context, id: u64) {
        // audio: el mini reproductor nativo
        if self.item(id).is_some_and(|i| self.kind_of(i) == Some(Kind::Audio)) {
            self.ensure_audio(ctx, id);
            if let Some(p) = self.audio_players.get_mut(&id) {
                p.toggle();
            }
            return;
        }
        if !self.players.contains_key(&id) {
            let Some(it) = self.item(id) else { return };
            let Some(s) = self.board.sources.get(&it.src) else { return };
            if !media::have_ffmpeg() {
                self.errors.push(t("Para reproducir vídeos hace falta ffmpeg").into());
                return;
            }
            let Some(input) = media::ffmpeg_input(&s.bytes, it.src) else {
                self.errors.push(t("No se pudo preparar el vídeo").into());
                return;
            };
            let p = Player::new(ctx, input, s.duration);
            self.players.insert(id, p);
        }
        if let Some(p) = self.players.get_mut(&id) {
            p.toggle();
        }
    }

    pub(crate) fn change_page(&mut self, id: u64, d: i32) {
        let Some(it) = self.item(id) else { return };
        let pages = self.board.sources.get(&it.src).map(|s| s.pages).unwrap_or(1).max(1);
        let np = (it.page as i32 + d).clamp(0, pages as i32 - 1) as u32;
        if np != it.page {
            if let Some(it) = self.item_mut(id) {
                it.page = np;
            }
            self.dirty = true;
        }
    }

    pub fn cmd(&mut self, ctx: &egui::Context, c: Cmd) {
        match c {
            Cmd::Open => self.dialog(0),
            Cmd::Save => match self.path.clone() {
                Some(p) => self.save_to(ctx, p),
                None => self.dialog(1),
            },
            Cmd::SaveAs => self.dialog(1),
            Cmd::Import => self.dialog(2),
            Cmd::Paste => self.paste(ctx, String::new()),
            Cmd::Copy => {
                let first = self.board.items.iter().rev().find(|i| self.sel.contains(&i.id)).cloned();
                match first {
                    Some(it) if it.text.is_some() => {
                        ctx.copy_text(it.text.map(|t| t.s).unwrap_or_default());
                        self.toast(ctx, t("Texto copiado"));
                    }
                    Some(it) => {
                        if let Some(s) = self.board.sources.get(&it.src) {
                            let bytes = match s.kind {
                                Kind::Image => Some(s.bytes.clone()),
                                Kind::Video => s.poster.clone(),
                                _ => None,
                            };
                            if let Some(b) = bytes {
                                import::spawn_copy_image(b, self.tx.clone(), ctx.clone());
                                self.toast(ctx, t("Imagen copiada"));
                            }
                        }
                    }
                    None => {}
                }
            }
            Cmd::SelectAll => self.sel = self.board.items.iter().map(|i| i.id).collect(),
            Cmd::Escape => {
                if self.crop_mode.take().is_some() {
                    // cancelar: vuelve a como estaba al entrar
                    if let Some(prev) = self.undo.pop() {
                        self.board.items = prev;
                    }
                } else if self.editing.is_some() {
                    self.commit_text();
                } else if self.help {
                    self.help = false;
                } else if self.active3d.is_some() {
                    self.active3d = None;
                } else if self.tool != Tool::Select {
                    self.tool = Tool::Select;
                } else {
                    self.sel.clear();
                }
            }
            Cmd::Delete => {
                if !self.sel.is_empty() {
                    self.snapshot();
                    let sel = std::mem::take(&mut self.sel);
                    self.board.items.retain(|i| !sel.contains(&i.id));
                    self.players.retain(|id, _| !sel.contains(id));
                    self.audio_players.retain(|id, _| !sel.contains(id));
                }
            }
            Cmd::Duplicate => {
                if !self.sel.is_empty() {
                    self.snapshot();
                    let d = 24.0 / self.zoom;
                    let copies: Vec<Item> = self.board.items.iter().filter(|i| self.sel.contains(&i.id)).cloned().collect();
                    self.sel.clear();
                    // las copias conservan el emparentado entre ellas; con el original no
                    let remap: HashMap<u64, u64> = copies.iter().map(|c| (c.id, self.board.new_id())).collect();
                    for mut it in copies {
                        it.id = remap[&it.id];
                        it.parent = it.parent.and_then(|p| remap.get(&p).copied());
                        it.pos = [it.pos[0] + d, it.pos[1] + d];
                        self.sel.insert(it.id);
                        self.board.items.push(it);
                    }
                    self.frames_to_back();
                }
            }
            Cmd::Undo => {
                self.editing = None;
                self.undo()
            }
            Cmd::Redo => self.redo(),
            Cmd::Fit => {
                if let Some(r) = self.bounds(!self.sel.is_empty()) {
                    self.fit_rect(r, true);
                }
            }
            Cmd::FitAll => {
                if let Some(r) = self.bounds(false) {
                    self.fit_rect(r, true);
                }
            }
            Cmd::ActualPixels => {
                let ppp = ctx.pixels_per_point();
                let it = self.board.items.iter().rev().find(|i| self.sel.contains(&i.id)).cloned();
                match it {
                    Some(it) => self.anim = Some((it.center(), (1.0 / (it.scale * ppp)).clamp(MIN_ZOOM, MAX_ZOOM))),
                    None => self.anim = Some((self.cam, 1.0 / ppp)),
                }
            }
            Cmd::Arrange => {
                // un marco seleccionado: ordenar su contenido dentro y ajustar el marco
                let frame = (self.sel.len() == 1).then(|| *self.sel.iter().next().unwrap()).filter(|id| self.item(*id).is_some_and(|i| i.frame.is_some()));
                if let Some(fid) = frame {
                    let inside: HashSet<u64> = self.frame_contents(fid).into_iter().filter(|id| self.item(*id).is_some_and(|i| i.frame.is_none())).collect();
                    if !inside.is_empty() {
                        self.snapshot();
                        let c = self.item(fid).map(|i| i.center()).unwrap_or(self.cam);
                        self.pack(&inside, Some(c));
                        self.fit_frame(fid, &inside);
                    }
                    return;
                }
                let t: HashSet<u64> = self.targets().into_iter().filter(|id| self.item(*id).is_some_and(|i| i.frame.is_none())).collect();
                if t.len() > 1 {
                    self.snapshot();
                    self.pack(&t, None);
                }
            }
            Cmd::FrameSelection => {
                let sel: HashSet<u64> = self.sel.iter().copied().filter(|id| self.item(*id).is_some_and(|i| i.frame.is_none())).collect();
                if let Some(r) = sel.iter().filter_map(|id| self.item(*id)).map(|i| self.board.aabb(i)).reduce(|a, b| a.union(b)) {
                    self.snapshot();
                    let pad = r.size().max_elem() * 0.05;
                    let id = self.new_frame(r.expand(pad));
                    self.sel = HashSet::from([id]);
                }
            }
            Cmd::Normalize => {
                let t: HashSet<u64> = self.targets().into_iter().filter(|id| self.item(*id).is_some_and(|i| i.frame.is_none())).collect();
                if t.len() > 1 {
                    self.snapshot();
                    let hs: HashMap<u64, f32> = self.board.items.iter().filter(|i| t.contains(&i.id)).map(|i| (i.id, self.board.size(i).y)).collect();
                    let avg = hs.values().sum::<f32>() / hs.len() as f32;
                    for it in self.board.items.iter_mut().filter(|i| t.contains(&i.id)) {
                        it.scale *= avg / hs[&it.id].max(1e-6);
                    }
                    self.pack(&t, None);
                }
            }
            Cmd::Flip | Cmd::Gray | Cmd::Rotate(_) | Cmd::ResetTransform => {
                if self.sel.is_empty() {
                    return;
                }
                self.snapshot();
                let sel = self.sel.clone();
                // el marco del visor 3D y la tarjeta de audio no giran ni se voltean
                let mut models: HashSet<u64> = self.selected_of(Kind::Model).into_iter().collect();
                models.extend(self.selected_of(Kind::Audio));
                // la rotación de un grupo gira también las posiciones alrededor de su centro
                let center = self.bounds(true).map(|r| r.center()).unwrap_or(self.cam);
                for it in self.board.items.iter_mut().filter(|i| sel.contains(&i.id) && i.frame.is_none()) {
                    let model = models.contains(&it.id);
                    match c {
                        Cmd::Flip => {
                            if !model {
                                it.flip = !it.flip;
                                it.rot = -it.rot;
                            }
                            if sel.len() > 1 {
                                it.pos[0] = 2.0 * center.x - it.pos[0];
                            }
                        }
                        Cmd::Gray => it.gray = !it.gray,
                        Cmd::Rotate(d) => {
                            let a = d as f32 * std::f32::consts::FRAC_PI_2;
                            // el marco del visor 3D no gira (se gira el modelo dentro)
                            if !model {
                                it.rot = snap_angle(it.rot + a);
                            }
                            if sel.len() > 1 {
                                let p = center + egui::emath::Rot2::from_angle(a) * (it.center() - center);
                                it.set_center(p);
                            }
                        }
                        _ => {
                            it.rot = 0.0;
                            it.flip = false;
                            it.gray = false;
                            if model {
                                it.cam3d = Some(CAM3D);
                            }
                        }
                    }
                }
            }
            Cmd::Front | Cmd::Back => {
                if self.sel.is_empty() {
                    return;
                }
                self.snapshot();
                let (mut s, mut o): (Vec<Item>, Vec<Item>) = std::mem::take(&mut self.board.items).into_iter().partition(|i| self.sel.contains(&i.id));
                self.board.items = if c == Cmd::Front {
                    o.append(&mut s);
                    o
                } else {
                    s.append(&mut o);
                    s
                };
                self.frames_to_back();
            }
            Cmd::OnTop => {
                // en Wayland la petición se ignora: se ofrece el modo compatible
                if !self.on_top && native_wayland() {
                    self.ontop_prompt = true;
                    return;
                }
                self.on_top = !self.on_top;
                self.cfg.on_top = self.on_top;
                self.cfg.save();
                let lvl = if self.on_top { egui::WindowLevel::AlwaysOnTop } else { egui::WindowLevel::Normal };
                ctx.send_viewport_cmd(egui::ViewportCommand::WindowLevel(lvl));
                self.toast(ctx, if self.on_top { t("Siempre encima: sí") } else { t("Siempre encima: no") });
            }
            Cmd::Help => self.help = !self.help,
            Cmd::Opacity(d) => {
                self.cfg.opacity = (self.cfg.opacity + d as f32 * 0.1).clamp(0.1, 1.0);
                self.cfg.save();
                self.toast(ctx, tf("Opacidad de la ventana: {} %", &[&format!("{:.0}", self.cfg.opacity * 100.0)]));
            }
            Cmd::Link => self.toggle_link(),
            Cmd::Crop => {
                if self.crop_mode.take().is_some() {
                    return;
                }
                if let Some(id) = self.croppable_selected() {
                    self.commit_text();
                    self.snapshot();
                    self.crop_mode = Some(id);
                    self.toast(ctx, t("Recorte: arrastra los bordes · Intro acepta · Esc cancela"));
                }
            }
            Cmd::CropReset => {
                let ids: Vec<u64> = self.board.items.iter().filter(|i| self.sel.contains(&i.id) && i.crop.is_some()).map(|i| i.id).collect();
                if !ids.is_empty() {
                    self.snapshot();
                    self.crop_mode = None;
                    for id in ids {
                        // la imagen completa queda donde estaba
                        let Some(it) = self.item(id) else { continue };
                        let c = self.board.full_center(it);
                        if let Some(it) = self.item_mut(id) {
                            it.crop = None;
                            it.set_center(c);
                        }
                    }
                }
            }
            Cmd::Accept => {
                if self.crop_mode.take().is_some() {
                    self.dirty = true;
                }
            }
            Cmd::Projection => {
                let ids = self.selected_of(Kind::Model).into_iter().chain(self.active3d).collect::<HashSet<u64>>();
                if !ids.is_empty() {
                    self.snapshot();
                    for it in self.board.items.iter_mut().filter(|i| ids.contains(&i.id)) {
                        it.ortho3d = !it.ortho3d;
                    }
                }
            }
            Cmd::Quit => ctx.send_viewport_cmd(egui::ViewportCommand::Close),
            Cmd::ToggleStatus => {
                self.cfg.show_status = !self.cfg.show_status;
                self.cfg.save();
            }
            Cmd::Grid => {
                self.grid = !self.grid;
                self.dirty = true;
            }
            Cmd::Blueprint => {
                self.blueprint = !self.blueprint;
                // el tema de plano se ve con su cuadrícula
                if self.blueprint {
                    self.grid = true;
                }
                self.dirty = true;
            }
            Cmd::Tool(t) => {
                self.commit_text();
                self.tool = t;
                self.active3d = None;
            }
            Cmd::PlayPause => {
                let mut ids = self.selected_of(Kind::Video);
                ids.extend(self.selected_of(Kind::Audio));
                for id in ids {
                    self.toggle_play(ctx, id);
                }
            }
            Cmd::Mute => {
                for id in self.selected_of(Kind::Video) {
                    if let Some(p) = self.players.get_mut(&id) {
                        let m = !p.muted;
                        p.set_muted(m);
                    }
                }
            }
            Cmd::Page(d) => {
                for id in self.selected_of(Kind::Doc) {
                    self.change_page(id, d);
                }
            }
            Cmd::ExpandPages => self.expand_pages(),
            Cmd::Align(a) => self.align(a),
        }
    }

    /// Cada página de los documentos seleccionados como un elemento propio, en rejilla.
    fn expand_pages(&mut self) {
        let docs = self.selected_of(Kind::Doc);
        if docs.is_empty() {
            return;
        }
        self.snapshot();
        let mut all = HashSet::new();
        for id in docs {
            let Some(it) = self.item(id).cloned() else { continue };
            let pages = self.board.sources.get(&it.src).map(|s| s.pages).unwrap_or(1).min(300);
            let mut ids = HashSet::from([id]);
            for p in 0..pages {
                if p == it.page {
                    continue;
                }
                let nid = self.board.new_id();
                let mut c = it.clone();
                c.id = nid;
                c.page = p;
                self.board.items.push(c);
                ids.insert(nid);
            }
            self.pack(&ids, Some(it.center()));
            all.extend(ids);
        }
        self.sel = all;
    }

    /// Alinea o reparte la selección según las cajas de los elementos (los marcos llevan su contenido).
    fn align(&mut self, a: u8) {
        let ids: Vec<u64> = self.board.items.iter().filter(|i| self.sel.contains(&i.id)).map(|i| i.id).collect();
        if ids.len() < 2 || (a >= 6 && ids.len() < 3) {
            return;
        }
        let boxes: Vec<(u64, Rect)> = ids.iter().filter_map(|id| self.item(*id).map(|i| (*id, self.board.aabb(i)))).collect();
        let all = boxes.iter().map(|b| b.1).reduce(|x, y| x.union(y)).unwrap();
        let mut moves: Vec<(u64, Vec2)> = Vec::new();
        match a {
            0..=5 => {
                for (id, r) in &boxes {
                    let d = match a {
                        0 => vec2(all.left() - r.left(), 0.0),
                        1 => vec2(all.center().x - r.center().x, 0.0),
                        2 => vec2(all.right() - r.right(), 0.0),
                        3 => vec2(0.0, all.top() - r.top()),
                        4 => vec2(0.0, all.center().y - r.center().y),
                        _ => vec2(0.0, all.bottom() - r.bottom()),
                    };
                    moves.push((*id, d));
                }
            }
            _ => {
                // repartir: mismos huecos entre cajas, de la primera a la última
                let horiz = a == 6;
                let mut v = boxes.clone();
                v.sort_by(|x, y| if horiz { x.1.left().total_cmp(&y.1.left()) } else { x.1.top().total_cmp(&y.1.top()) });
                let span = if horiz { all.width() } else { all.height() };
                let used: f32 = v.iter().map(|b| if horiz { b.1.width() } else { b.1.height() }).sum();
                let gap = (span - used) / (v.len() - 1) as f32;
                let mut at = if horiz { all.left() } else { all.top() };
                for (id, r) in &v {
                    let d = if horiz { vec2(at - r.left(), 0.0) } else { vec2(0.0, at - r.top()) };
                    moves.push((*id, d));
                    at += (if horiz { r.width() } else { r.height() }) + gap;
                }
            }
        }
        self.snapshot();
        // un marco se lleva lo que tiene dentro (si eso no está también seleccionado)
        let mut extra: Vec<(u64, Vec2)> = Vec::new();
        for (id, d) in &moves {
            if self.item(*id).is_some_and(|i| i.frame.is_some()) {
                for c in self.frame_contents(*id) {
                    if !self.sel.contains(&c) {
                        extra.push((c, *d));
                    }
                }
            }
        }
        // y los hijos emparentados siguen a su padre
        let moved: HashSet<u64> = moves.iter().map(|m| m.0).collect();
        for (id, d) in moves.clone() {
            for c in self.with_descendants(&HashSet::from([id])) {
                if !moved.contains(&c) {
                    extra.push((c, d));
                }
            }
        }
        for (id, d) in moves.into_iter().chain(extra) {
            if let Some(it) = self.item_mut(id) {
                it.set_center(it.center() + d);
            }
        }
    }

    /// Pide (en segundo plano, una sola vez) los enlaces de una página de documento.
    pub(crate) fn want_doc_links(&mut self, ctx: &egui::Context, src: u64, page: u32) {
        if self.doc_links.contains_key(&(src, page)) {
            return;
        }
        let Some(s) = self.board.sources.get(&src) else { return };
        self.doc_links.insert((src, page), Vec::new());
        let (bytes, tx, ctx) = (s.bytes.clone(), self.links_tx.clone(), ctx.clone());
        std::thread::spawn(move || {
            let v = crate::decode::guarded(|| media::doc_links(&bytes, page)).unwrap_or_default();
            let _ = tx.send(((src, page), v));
            ctx.request_repaint();
        });
    }

    /// Enlace bajo un punto del mundo: dirección web de un texto o zona de enlace de un documento.
    pub(crate) fn link_at(&self, ctx: &egui::Context, w: Pos2) -> Option<String> {
        let id = self.hit(w)?;
        let it = self.item(id)?;
        // posición local del elemento, sin giro ni espejo
        let mut l = egui::emath::Rot2::from_angle(-it.rot) * (w - it.center());
        if it.flip {
            l.x = -l.x;
        }
        if let Some(t) = &it.text {
            let urls = media::url_ranges(&t.s);
            if urls.is_empty() {
                return None;
            }
            let fs = 64.0;
            let galley = ctx.fonts_mut(|f| f.layout_job(crate::canvas::text_job(t, &t.s, fs)));
            let gr = galley.rect;
            let g = l / (t.size * it.scale) * fs + gr.center().to_vec2();
            if !gr.expand(2.0).contains(g.to_pos2()) {
                return None;
            }
            let ci = galley.cursor_from_pos(g).index.0;
            return urls.into_iter().find(|(a, b, _)| ci >= *a && ci < *b).map(|u| u.2);
        }
        if self.kind_of(it) == Some(Kind::Doc) {
            let size = self.board.size(it);
            let f = (l + size * 0.5) / size;
            let links = self.doc_links.get(&(it.src, it.page))?;
            return links.iter().find(|k| f.x >= k.x && f.x <= k.x + k.w && f.y >= k.y && f.y <= k.y + k.h).map(|k| k.uri.clone());
        }
        None
    }

    pub(crate) fn set_lang(&mut self, ctx: &egui::Context, code: &str) {
        self.cfg.lang = code.to_string();
        self.cfg.save();
        let i = crate::i18n::resolve(code);
        crate::i18n::set(i);
        if !crate::i18n::install_fonts(ctx, i) {
            self.errors.push(t("No se encontró una fuente para este idioma (instala Noto Sans CJK)").into());
        }
    }

    // -----------------------------------------------------------------------------------------
    // Emparentar: un elemento va unido a otro (p. ej. un texto junto a su imagen)
    // -----------------------------------------------------------------------------------------

    /// Todos los que cuelgan de `ids` (hijos, nietos…), sin repetir y sin ciclos.
    pub(crate) fn with_descendants(&self, ids: &HashSet<u64>) -> HashSet<u64> {
        let mut out = ids.clone();
        let mut frontier: Vec<u64> = ids.iter().copied().collect();
        while let Some(p) = frontier.pop() {
            for it in &self.board.items {
                if it.parent == Some(p) && out.insert(it.id) {
                    frontier.push(it.id);
                }
            }
        }
        out
    }

    /// ¿Hay algo emparentado en la selección (como padre o como hijo)?
    pub(crate) fn selection_linked(&self) -> bool {
        self.board.items.iter().any(|i| (self.sel.contains(&i.id) && i.parent.is_some_and(|p| self.item(p).is_some())) || i.parent.is_some_and(|p| self.sel.contains(&p)))
    }

    /// El único elemento seleccionado, si se puede recortar (imagen, vídeo o página).
    pub(crate) fn croppable_selected(&self) -> Option<u64> {
        if self.sel.len() != 1 {
            return None;
        }
        let id = *self.sel.iter().next()?;
        let it = self.item(id)?;
        matches!(self.kind_of(it), Some(Kind::Image) | Some(Kind::Video) | Some(Kind::Doc)).then_some(id)
    }

    /// Mantiene el orden de selección al día: quita lo deseleccionado y añade al final lo nuevo
    /// (seleccionado por otras vías: todo, duplicar, importar…).
    pub(crate) fn sync_sel_order(&mut self) {
        self.sel_order.retain(|id| self.sel.contains(id));
        if self.sel_order.len() != self.sel.len() {
            let mut fresh: Vec<u64> = self.sel.iter().filter(|id| !self.sel_order.contains(id)).copied().collect();
            fresh.sort_unstable();
            self.sel_order.extend(fresh);
        }
    }

    /// Un solo gesto: si la selección ya está emparentada, se rompe; si no, todo se une al primer
    /// elemento seleccionado, que pasa a arrastrar a los demás.
    pub(crate) fn toggle_link(&mut self) {
        if self.selection_linked() {
            self.snapshot();
            let sel = self.sel.clone();
            for it in self.board.items.iter_mut() {
                if sel.contains(&it.id) || it.parent.is_some_and(|p| sel.contains(&p)) {
                    it.parent = None;
                }
            }
            return;
        }
        if self.sel.len() < 2 {
            return;
        }
        // el padre es el primer elemento seleccionado
        self.sync_sel_order();
        let Some(pid) = self.sel_order.first().copied() else { return };
        self.snapshot();
        let sel = self.sel.clone();
        for it in self.board.items.iter_mut() {
            if it.id == pid {
                // el padre deja de colgar de alguien de la selección (evita ciclos)
                if it.parent.is_some_and(|p| sel.contains(&p)) {
                    it.parent = None;
                }
            } else if sel.contains(&it.id) {
                it.parent = Some(pid);
            }
        }
    }

    /// Crea el mini reproductor de un audio (lee sus etiquetas y carátula en segundo plano).
    pub(crate) fn ensure_audio(&mut self, ctx: &egui::Context, id: u64) {
        if self.audio_players.contains_key(&id) {
            return;
        }
        let Some(s) = self.item(id).and_then(|i| self.board.sources.get(&i.src)) else { return };
        let mut p = crate::audio::AudioPlayer::new(ctx, s.bytes.clone(), &s.ext());
        p.duration = s.duration as f64;
        self.audio_players.insert(id, p);
    }

    /// Ajusta el marco a la caja de `ids`, con margen.
    pub(crate) fn fit_frame(&mut self, fid: u64, ids: &HashSet<u64>) {
        let Some(r) = ids.iter().filter_map(|id| self.item(*id)).map(|i| self.board.aabb(i)).reduce(|a, b| a.union(b)) else { return };
        let r = r.expand(r.size().max_elem() * 0.05);
        if let Some(it) = self.item_mut(fid) {
            let s = it.scale;
            if let Some(f) = it.frame.as_mut() {
                f.w = r.width() / s;
                f.h = r.height() / s;
            }
            it.set_center(r.center());
        }
    }

    /// Pegar: rutas o URL del texto si las hay; si no, la imagen del portapapeles.
    fn paste(&mut self, ctx: &egui::Context, text: String) {
        let at = ctx.input(|i| i.pointer.hover_pos()).filter(|p| self.canvas.contains(*p));
        let paths = import::paths_in_text(&text);
        if !paths.is_empty() {
            self.import_paths(ctx, paths, at);
        } else {
            let b = self.new_batch(at);
            import::spawn_clipboard_image(b, self.tx.clone(), ctx.clone());
        }
    }

    pub(crate) fn keys(&mut self, ctx: &egui::Context) {
        // con un texto en edición el teclado es suyo; Esc lo cierra
        if self.editing.is_some() {
            if ctx.input_mut(|i| i.consume_key(Modifiers::NONE, K::Escape)) {
                self.cmd(ctx, Cmd::Escape);
            }
            return;
        }
        if ctx.egui_wants_keyboard_input() {
            return;
        }
        let cmd = Modifiers::COMMAND;
        let cs = Modifiers::COMMAND | Modifiers::SHIFT;
        let n = Modifiers::NONE;
        let s = Modifiers::SHIFT;
        let table: &[(Modifiers, K, Cmd)] = &[
            (cs, K::S, Cmd::SaveAs),
            (cs, K::Z, Cmd::Redo),
            (cmd, K::S, Cmd::Save),
            (cmd, K::O, Cmd::Open),
            (cmd, K::I, Cmd::Import),
            (cmd, K::C, Cmd::Copy),
            (cmd, K::A, Cmd::SelectAll),
            (cmd, K::D, Cmd::Duplicate),
            (cmd, K::Z, Cmd::Undo),
            (cmd, K::Y, Cmd::Redo),
            (cmd, K::R, Cmd::ResetTransform),
            (cmd, K::T, Cmd::OnTop),
            (cs, K::ArrowUp, Cmd::Opacity(1)),
            (cs, K::ArrowDown, Cmd::Opacity(-1)),
            (cmd, K::G, Cmd::FrameSelection),
            (cmd, K::L, Cmd::Link),
            (Modifiers::ALT, K::C, Cmd::CropReset),
            (n, K::C, Cmd::Crop),
            (n, K::Enter, Cmd::Accept),
            (Modifiers::ALT, K::ArrowLeft, Cmd::Align(0)),
            (Modifiers::ALT, K::ArrowRight, Cmd::Align(2)),
            (Modifiers::ALT, K::ArrowUp, Cmd::Align(3)),
            (Modifiers::ALT, K::ArrowDown, Cmd::Align(5)),
            (Modifiers::ALT, K::H, Cmd::Align(1)),
            (Modifiers::ALT, K::V, Cmd::Align(4)),
            (cmd, K::Num0, Cmd::FitAll),
            (s, K::R, Cmd::Rotate(-1)),
            (cs, K::G, Cmd::Blueprint),
            (s, K::G, Cmd::Grid),
            (s, K::M, Cmd::Mute),
            (n, K::R, Cmd::Rotate(1)),
            (n, K::Delete, Cmd::Delete),
            (n, K::Backspace, Cmd::Delete),
            (n, K::Escape, Cmd::Escape),
            (n, K::F, Cmd::Fit),
            (n, K::Home, Cmd::FitAll),
            (n, K::Num1, Cmd::ActualPixels),
            (n, K::A, Cmd::Arrange),
            (n, K::N, Cmd::Normalize),
            (n, K::H, Cmd::Flip),
            (n, K::G, Cmd::Gray),
            (n, K::V, Cmd::Tool(Tool::Select)),
            (n, K::T, Cmd::Tool(Tool::Text)),
            (n, K::D, Cmd::Tool(Tool::Draw)),
            (n, K::B, Cmd::Tool(Tool::Draw)),
            (n, K::K, Cmd::Tool(Tool::Frame)),
            (n, K::P, Cmd::PlayPause),
            (n, K::M, Cmd::Flip),
            (n, K::Comma, Cmd::Page(-1)),
            (n, K::Period, Cmd::Page(1)),
            (n, K::E, Cmd::ExpandPages),
            (n, K::O, Cmd::Projection),
            (n, K::PageUp, Cmd::Front),
            (n, K::CloseBracket, Cmd::Front),
            (n, K::PageDown, Cmd::Back),
            (n, K::OpenBracket, Cmd::Back),
            (n, K::F1, Cmd::Help),
            (s, K::Questionmark, Cmd::Help),
            (n, K::Questionmark, Cmd::Help),
        ];
        let mut fired = Vec::new();
        ctx.input_mut(|i| {
            for (m, k, c) in table {
                if i.consume_key(*m, *k) {
                    fired.push(*c);
                }
            }
        });
        for c in fired {
            self.cmd(ctx, c);
        }
        // pegar (egui-winit parcheado: llega aunque el portapapeles no tenga texto)
        let pastes: Vec<String> = ctx.input(|i| i.events.iter().filter_map(|e| if let egui::Event::Paste(t) = e { Some(t.clone()) } else { None }).collect());
        for t in pastes {
            self.paste(ctx, t);
        }
        // zoom con teclado y empujar la selección con las flechas
        let (zin, zout, nudge) = ctx.input_mut(|i| {
            let zin = i.consume_key(cmd, K::Plus) || i.consume_key(cmd, K::Equals) || i.consume_key(n, K::Plus);
            let zout = i.consume_key(cmd, K::Minus) || i.consume_key(n, K::Minus);
            let step = if i.modifiers.shift { 10.0 } else { 1.0 };
            let mut d = Vec2::ZERO;
            for (k, v) in [(K::ArrowLeft, vec2(-1.0, 0.0)), (K::ArrowRight, vec2(1.0, 0.0)), (K::ArrowUp, vec2(0.0, -1.0)), (K::ArrowDown, vec2(0.0, 1.0))] {
                // (con Alt las flechas alinean)
                if i.key_pressed(k) && !i.modifiers.alt {
                    d += v * step;
                }
            }
            (zin, zout, d)
        });
        if zin {
            self.zoom_at(self.canvas.center(), 1.25);
        }
        if zout {
            self.zoom_at(self.canvas.center(), 0.8);
        }
        if nudge != Vec2::ZERO && !self.sel.is_empty() {
            self.snapshot();
            let d = nudge / self.zoom;
            let sel = self.with_descendants(&self.sel.clone());
            for it in self.board.items.iter_mut().filter(|i| sel.contains(&i.id)) {
                it.set_center(it.center() + d);
            }
        }
    }

    // -----------------------------------------------------------------------------------------
    // Arrastre hacia otras aplicaciones
    // -----------------------------------------------------------------------------------------

    /// Archivos (con su nombre original) de lo seleccionado, para soltarlos en otro programa.
    fn selected_files(&self) -> Vec<PathBuf> {
        let mut out = Vec::new();
        let mut seen = HashSet::new();
        for it in self.board.items.iter().filter(|i| self.sel.contains(&i.id) && i.is_media()) {
            if !seen.insert(it.src) {
                continue;
            }
            let Some(s) = self.board.sources.get(&it.src) else { continue };
            // lo que ya es un archivo entero en el disco (un vídeo importado) se entrega tal cual
            if let Some((p, 0, len)) = s.bytes.file_window()
                && !import::is_board_path(p)
                && std::fs::metadata(p).map(|m| m.len() as usize == len).unwrap_or(false)
            {
                out.push(p.to_path_buf());
                continue;
            }
            if let Some(p) = media::export_temp(&s.name, it.src, &s.bytes) {
                out.push(p);
            }
        }
        out
    }

    /// Al sacar de la ventana lo que se está moviendo, pasa a ser un arrastre del sistema (soltar en
    /// Krita, Photoshop, Blender, el gestor de archivos…). También recibe lo que se suelta desde
    /// otras aplicaciones en Wayland, donde winit no lo entrega.
    pub(crate) fn drag_out_tick(&mut self, ctx: &egui::Context, frame: &eframe::Frame) {
        if self.dragout.is_none() && !self.dragout_tried {
            self.dragout_tried = true;
            self.dragout = crate::dragout::DragOut::new(frame, ctx);
            if std::env::var_os("REFS_TRACE").is_some() || cfg!(feature = "selftest") && std::env::var_os("REFS_SHOT").is_some() {
                eprintln!("[trace] arrastrar y soltar del sistema: {}", if self.dragout.is_some() { "activo" } else { "no disponible" });
            }
        }
        let (pos, down) = ctx.input(|i| (i.pointer.latest_pos(), i.pointer.primary_down()));
        if pos.is_some() {
            self.last_ptr = pos;
        }
        let outside = pos.map(|p| !ctx.content_rect().shrink(2.0).contains(p)).unwrap_or(false);
        let moving = matches!(self.drag, Drag::Move { moved: true, .. });
        let want = moving && down && outside && !self.sel.is_empty();
        let paths = if want && self.dragout.is_some() { self.selected_files() } else { Vec::new() };
        let Some(d) = self.dragout.as_mut() else { return };
        d.pump();
        let dropped = d.take_dropped();
        let done = d.take_done();
        let started = !paths.is_empty() && d.start(&paths);
        if d.active() {
            ctx.request_repaint_after(std::time::Duration::from_millis(10));
        }
        if started {
            // lo movido vuelve a su sitio: se ha llevado a otra aplicación, no movido
            if let Some(prev) = self.undo.pop() {
                self.board.items = prev;
            }
            self.drag = Drag::None;
            self.release_inject = true;
            ctx.request_repaint();
        }
        if done {
            self.release_inject = true;
            ctx.request_repaint();
        }
        if !dropped.is_empty() {
            let at = self.last_ptr.filter(|p| self.canvas.contains(*p));
            self.import_paths(ctx, dropped, at);
        }
    }

    pub(crate) fn animate(&mut self, ctx: &egui::Context) {
        let Some((c, z)) = self.anim else { return };
        let dt = ctx.input(|i| i.stable_dt).min(0.05);
        let t = 1.0 - (-dt * 14.0).exp();
        // zoom en escala logarítmica: la animación se siente uniforme
        let lz = self.zoom.ln() + (z.ln() - self.zoom.ln()) * t;
        self.zoom = lz.exp();
        self.cam += (c - self.cam) * t;
        if (self.zoom / z - 1.0).abs() < 0.002 && (c - self.cam).length() * self.zoom < 0.5 {
            self.zoom = z;
            self.cam = c;
            self.anim = None;
        } else {
            ctx.request_repaint();
        }
    }

    pub(crate) fn update_title(&mut self, ctx: &egui::Context) {
        let name = self.path.as_ref().and_then(|p| p.file_name()).map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| t("Sin título").into());
        let t = format!("{}{} — Mosaic", if self.dirty { "● " } else { "" }, name);
        if t != self.title {
            ctx.send_viewport_cmd(egui::ViewportCommand::Title(t.clone()));
            self.title = t;
        }
    }
}

impl App {
    pub(crate) fn effective_bg(&self) -> [u8; 3] {
        if self.blueprint { BLUEPRINT } else { self.bg }
    }

    /// Reinicia en modo compatible (XWayland), donde GNOME/KDE sí respetan «siempre encima».
    pub(crate) fn restart_compat_on_top(&mut self, ctx: &egui::Context) {
        // el tablero pasa al proceso nuevo por archivo
        let path = match self.path.clone() {
            Some(p) => {
                if self.dirty {
                    self.save_to(ctx, p.clone());
                }
                Some(p)
            }
            None if !self.board.items.is_empty() => {
                let dir = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(std::env::temp_dir).join(".local/state/mosaic");
                let _ = std::fs::create_dir_all(&dir);
                let p = dir.join("sin-titulo.refs");
                self.save_to(ctx, p.clone());
                Some(p)
            }
            None => None,
        };
        if self.dirty {
            return; // no se pudo guardar: el aviso ya está en pantalla
        }
        self.cfg.on_top = true;
        self.cfg.save();
        let mut c = std::process::Command::new(std::env::current_exe().unwrap_or_else(|_| PathBuf::from("mosaic")));
        c.env("REFS_X11", "1").env("REFS_ON_TOP", "1");
        if let Some(p) = path {
            c.arg(p);
        }
        match c.spawn() {
            Ok(_) => {
                self.allow_close = true;
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
            Err(e) => self.errors.push(tf("No se pudo reiniciar: {}", &[&e])),
        }
    }
}

pub(crate) fn snap_angle(a: f32) -> f32 {
    let q = std::f32::consts::FRAC_PI_2;
    let r = (a / q).round() * q;
    let a = if (a - r).abs() < 1e-3 { r } else { a };
    a.rem_euclid(std::f32::consts::TAU)
}

impl eframe::App for App {
    fn raw_input_hook(&mut self, _ctx: &egui::Context, raw: &mut egui::RawInput) {
        // tras empezar un arrastre del sistema el compositor se queda el puntero: egui no recibe la
        // suelta del botón, así que se simula (igual que en File Browser)
        if std::mem::take(&mut self.release_inject) {
            let pos = self.last_ptr.unwrap_or(Pos2::ZERO);
            raw.events.push(egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed: false, modifiers: Modifiers::NONE });
            raw.events.push(egui::Event::PointerGone);
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        self.frame_ui(ui, frame);
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        self.players.clear();
        media::cleanup_temp();
        crate::decode::cleanup_caches(true);
    }

    fn clear_color(&self, _v: &egui::Visuals) -> [f32; 4] {
        // opaco: la transparencia de toda la ventana la aplica `fade` al final del fotograma
        let b = self.effective_bg();
        egui::Color32::from_rgb(b[0], b[1], b[2]).to_normalized_gamma_f32()
    }
}
