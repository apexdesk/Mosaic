//! Lienzo: ratón, herramientas, pintado de cada tipo de elemento, controles sobre los elementos,
//! barra de herramientas, menú y avisos.
use crate::app::{App, CAM3D, Cmd, Control, Drag, MeshState, Tool};
use crate::board::{Ink, Item, Kind, Text};
use crate::i18n::{t, tf, tn};
use crate::media;
use crate::textures::{self, Job, Key, Request};
use egui::{Color32, CornerRadius, FontId, Key as K, Pos2, Rect, Sense, Stroke, StrokeKind, Vec2, pos2, vec2};
use std::collections::HashSet;

const ACCENT: Color32 = Color32::from_rgb(0x4d, 0x9c, 0xff);
const ACTIVE3D: Color32 = Color32::from_rgb(0xff, 0x9f, 0x1c);
const HANDLE: f32 = 9.0;
/// Radio de las esquinas de la ventana cuando no está maximizada.
const WINDOW_RADIUS: f32 = 12.0;
/// Alto de la franja superior que mueve la ventana (donde flotan las pastillas de la cabecera).
const TOP_STRIP: f32 = 52.0;
const ROT_HANDLE_DIST: f32 = 26.0;

/// Transformación mundo → pantalla (copiable, para pintar sin tomar prestada la aplicación).
#[derive(Clone, Copy)]
struct Xf {
    center: Pos2,
    cam: Pos2,
    zoom: f32,
}

impl Xf {
    fn s(&self, w: Pos2) -> Pos2 {
        self.center + (w - self.cam) * self.zoom
    }
}

fn c32(c: [u8; 4]) -> Color32 {
    Color32::from_rgba_unmultiplied(c[0], c[1], c[2], c[3])
}

/// Superposiciones que se pintan después de todos los elementos (quedan encima).
enum Over {
    Video { id: u64, bb: Rect },
    Doc { id: u64, bb: Rect, page: u32, pages: u32 },
    Model { id: u64, bb: Rect, active: bool, ortho: bool },
}

impl App {
    pub(crate) fn frame_ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        for ps in self.tex.pump(&ctx) {
            if let Some(s) = self.board.sources.get_mut(&ps.src) {
                s.page_sizes.insert(ps.page, ps.size);
            }
        }
        self.receive(&ctx);
        self.drag_out_tick(&ctx, frame);

        // archivos o carpetas soltados (X11, Windows, macOS; en Wayland llegan por `dragout`)
        let (dropped, hover) = ctx.input(|i| (i.raw.dropped_files.clone(), i.pointer.hover_pos()));
        if !dropped.is_empty() {
            let paths: Vec<std::path::PathBuf> = dropped.iter().map(|f| f.path().to_path_buf()).collect();
            self.import_paths(&ctx, paths, hover);
        }

        self.keys(&ctx);
        self.animate(&ctx);

        // reproductores: los de elementos borrados se paran; los demás suben su último fotograma
        let ids: HashSet<u64> = self.board.items.iter().map(|i| i.id).collect();
        self.sync_sel_order();
        self.players.retain(|id, _| ids.contains(id));
        for p in self.players.values_mut() {
            p.poll();
        }
        self.audio_players.retain(|id, _| ids.contains(id));
        let mut audio_playing = false;
        for p in self.audio_players.values_mut() {
            p.poll();
            audio_playing |= p.playing();
        }
        if audio_playing {
            // la barra y la onda avanzan mientras suena
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
        }

        // ventana sin decoración: bordes para redimensionar y cabecera propia
        self.window_edges(&ctx);

        let eb = self.effective_bg();
        let bg = Color32::from_rgb(eb[0], eb[1], eb[2]);
        egui::CentralPanel::default().frame(egui::Frame::new().fill(bg)).show(ui, |ui| {
            let rect = ui.max_rect();
            self.canvas = rect;
            let resp = ui.interact(rect, ui.id().with("canvas"), Sense::click_and_drag());
            self.pointer(&ctx, &resp);
            let painter = ui.painter_at(rect);
            self.paint(&ctx, &painter);
            if ctx.input(|i| !i.raw.hovered_files.is_empty()) {
                painter.rect(rect.shrink(6.0), CornerRadius::same(8), ACCENT.gamma_multiply(0.08), Stroke::new(2.0, ACCENT), StrokeKind::Inside);
            }
            self.text_editor(&ctx);
            self.overlays(&ctx, &painter);
            self.selected_text_format_bar(&ctx);
            self.context_menu(&ctx, &resp);
        });

        // la cabecera flota sobre el lienzo (después de él: queda encima)
        self.header(&ctx);
        self.color_popup_ui(&ctx);
        self.ontop_dialog(&ctx);
        self.close_guard(&ctx);
        self.update_title(&ctx);
        // transparencia de toda la ventana y esquinas redondeadas (si no está maximizada): lo último
        let square = ctx.input(|i| i.viewport().maximized.unwrap_or(false) || i.viewport().fullscreen.unwrap_or(false));
        let radius = if square { 0.0 } else { WINDOW_RADIUS };
        if radius > 0.0 {
            // borde fino para que la ventana se distinga sobre fondos oscuros
            let p = ctx.layer_painter(egui::LayerId::new(egui::Order::Foreground, egui::Id::new("window-border")));
            p.rect_stroke(ctx.content_rect(), CornerRadius::same(radius as u8), Stroke::new(1.0, Color32::from_white_alpha(28)), StrokeKind::Inside);
        }
        self.fade.paint(&ctx, self.cfg.opacity, radius);
        #[cfg(feature = "selftest")]
        self.selftest(&ctx);
    }

    fn xf(&self) -> Xf {
        Xf { center: self.canvas.center(), cam: self.cam, zoom: self.zoom }
    }

    // -----------------------------------------------------------------------------------------
    // Ratón
    // -----------------------------------------------------------------------------------------

    /// Esquinas (mundo) del marco de transformación: el elemento girado si hay uno solo, o la caja
    /// del grupo.
    fn frame_corners(&self) -> Option<([Pos2; 4], Option<u64>)> {
        if self.sel.len() == 1 {
            let id = *self.sel.iter().next()?;
            let it = self.item(id)?;
            return Some((self.board.corners(it), Some(id)));
        }
        let r = self.bounds(true)?;
        Some(([r.left_top(), r.right_top(), r.right_bottom(), r.left_bottom()], None))
    }

    fn rot_handle(&self, corners: &[Pos2; 4]) -> Pos2 {
        let (a, b) = (self.to_screen(corners[0]), self.to_screen(corners[1]));
        let (d, c) = (self.to_screen(corners[3]), self.to_screen(corners[2]));
        let top = a.lerp(b, 0.5);
        let bottom = d.lerp(c, 0.5);
        let up = (top - bottom).normalized();
        let up = if up.is_finite() { up } else { vec2(0.0, -1.0) };
        top + up * ROT_HANDLE_DIST
    }

    /// ¿El marco seleccionado admite girar? (el del visor 3D no)
    fn can_rotate(&self, id: u64) -> bool {
        self.item(id).is_some_and(|it| it.frame.is_none() && !matches!(self.kind_of(it), Some(Kind::Model) | Some(Kind::Audio)))
    }

    fn handle_drag(&self, pos: Pos2) -> Option<Drag> {
        let (c, single) = self.frame_corners()?;
        // un marco cambia de tamaño sin escalar lo que contiene
        if let Some(id) = single.filter(|id| self.item(*id).is_some_and(|i| i.frame.is_some())) {
            let k = c.iter().position(|p| self.to_screen(*p).distance(pos) <= HANDLE)?;
            return Some(Drag::FrameResize { id, anchor: c[(k + 2) % 4] });
        }
        if let Some(id) = single
            && self.can_rotate(id)
            && self.rot_handle(&c).distance(pos) <= HANDLE
        {
            let it = self.item(id)?;
            let v = self.to_world(pos) - it.center();
            return Some(Drag::Rotate { center: it.center(), start_angle: v.y.atan2(v.x), orig: it.rot, id });
        }
        let k = c.iter().position(|p| self.to_screen(*p).distance(pos) <= HANDLE)?;
        let orig = self.board.items.iter().filter(|i| self.sel.contains(&i.id)).map(|i| (i.id, i.center(), i.scale)).collect();
        Some(Drag::Scale { pivot: c[(k + 2) % 4], corner: c[k], orig })
    }

    fn pointer(&mut self, ctx: &egui::Context, resp: &egui::Response) {
        let (pos, pressed, down, released, mid_down, sec_pressed, mods, scroll, pinch, delta) = ctx.input(|i| {
            (
                i.pointer.latest_pos(),
                i.pointer.primary_pressed(),
                i.pointer.primary_down(),
                i.pointer.primary_released(),
                i.pointer.middle_down(),
                i.pointer.secondary_pressed(),
                i.modifiers,
                i.smooth_scroll_delta,
                i.zoom_delta(),
                i.pointer.delta(),
            )
        });
        let space = self.editing.is_none() && !ctx.egui_wants_keyboard_input() && ctx.input(|i| i.key_down(K::Space));
        let Some(pos) = pos else { return };
        if self.win_op {
            if !down {
                self.win_op = false;
            }
            return;
        }
        let hovered = resp.hovered();
        let w = self.to_world(pos);

        // rueda: zoom alrededor del cursor; sobre un visor 3D activo, zoom del modelo
        if hovered {
            let active = self.active3d.filter(|id| self.hit(w) == Some(*id));
            if let Some(id) = active {
                let f = pinch * (scroll.y * 0.002).exp();
                if (f - 1.0).abs() > 1e-4
                    && let Some(it) = self.item_mut(id)
                {
                    let c = it.cam3d.get_or_insert(CAM3D);
                    c[2] = (c[2] * f).clamp(0.05, 50.0);
                    self.dirty = true;
                }
            } else {
                let mut f = pinch;
                if scroll.y != 0.0 {
                    f *= (scroll.y * 0.0025).exp();
                }
                if scroll.x != 0.0 && mods.shift {
                    self.cam.x -= scroll.x / self.zoom;
                }
                if (f - 1.0).abs() > 1e-4 {
                    self.zoom_at(pos, f);
                }
            }
        }

        // botón central: desplazar
        if mid_down && hovered && matches!(self.drag, Drag::None | Drag::Pan) {
            self.cam -= delta / self.zoom;
            self.anim = None;
            ctx.set_cursor_icon(egui::CursorIcon::Grabbing);
            return;
        }

        // clic derecho sobre un elemento sin seleccionar: seleccionarlo para el menú
        if sec_pressed
            && hovered
            && let Some(id) = self.hit(w)
            && !self.sel.contains(&id)
        {
            self.sel = HashSet::from([id]);
        }

        // la franja superior (donde iba la barra de título) mueve la ventana si se pulsa en vacío
        let in_strip = pos.y < self.canvas.top() + TOP_STRIP && self.hit(w).is_none() && !self.controls.iter().any(|(r, _)| r.contains(pos));
        let strip_id = egui::Id::new("strip-press");
        if pressed && hovered && in_strip && self.tool == Tool::Select && !space && matches!(self.drag, Drag::None) {
            // ni selección ni rectángulo: se espera a ver si es un arrastre (mover) o un doble clic
            ctx.data_mut(|d| d.insert_temp(strip_id, true));
            if self.editing.is_some() {
                self.commit_text();
            }
            return;
        }
        if ctx.data(|d| d.get_temp::<bool>(strip_id)).unwrap_or(false) {
            if down && ctx.input(|i| i.pointer.is_decidedly_dragging()) {
                ctx.send_viewport_cmd(egui::ViewportCommand::StartDrag);
                ctx.data_mut(|d| d.remove::<bool>(strip_id));
                self.win_op = true;
            } else if !down {
                ctx.data_mut(|d| d.remove::<bool>(strip_id));
                if resp.double_clicked() {
                    let max = ctx.input(|i| i.viewport().maximized.unwrap_or(false));
                    ctx.send_viewport_cmd(egui::ViewportCommand::Maximized(!max));
                }
            }
            return;
        }
        // Ctrl+clic sobre un enlace (texto o documento): se abre en el navegador
        if mods.command && hovered && self.tool == Tool::Select && self.editing.is_none()
            && let Some(url) = self.link_at(ctx, w) {
                ctx.set_cursor_icon(egui::CursorIcon::PointingHand);
                egui::Tooltip::always_open(ctx.clone(), egui::LayerId::new(egui::Order::Tooltip, egui::Id::new("link-tip")), egui::Id::new("link-tip"), egui::PopupAnchor::Pointer).show(|ui| {
                    ui.label(&url);
                });
                if pressed {
                    if let Err(e) = media::open_url(&url) {
                        self.errors.push(e);
                    }
                    return;
                }
            }
        if pressed && hovered {
            if self.editing.is_some() {
                self.commit_text();
            }
            let control = self.controls.iter().rev().find(|(r, _)| r.contains(pos)).map(|(r, c)| (*r, *c));
            self.drag = if space || mods.alt {
                Drag::Pan
            } else if let Some((r, c)) = control {
                match c {
                    Control::PlayPause(id) => {
                        self.toggle_play(ctx, id);
                        Drag::None
                    }
                    Control::Seek(id) => Drag::Seek { id, bar: r },
                    Control::Mute(id) => {
                        if let Some(p) = self.players.get_mut(&id) {
                            let m = !p.muted;
                            p.set_muted(m);
                        }
                        Drag::None
                    }
                    Control::PagePrev(id) => {
                        self.change_page(id, -1);
                        Drag::None
                    }
                    Control::PageNext(id) => {
                        self.change_page(id, 1);
                        Drag::None
                    }
                    Control::Projection(id) => {
                        self.snapshot();
                        if let Some(it) = self.item_mut(id) {
                            it.ortho3d = !it.ortho3d;
                        }
                        Drag::None
                    }
                }
            } else {
                match self.tool {
                    Tool::Draw => Drag::Ink(vec![w]),
                    Tool::Frame => Drag::FrameNew(w),
                    Tool::Text => {
                        match self.hit(w).filter(|id| self.item(*id).is_some_and(|i| i.text.is_some())) {
                            Some(id) => self.edit_text(id),
                            None => self.new_text(w),
                        }
                        Drag::None
                    }
                    Tool::Select => self.press_select(w, pos, mods),
                }
            };
        }

        if down {
            let zoom = self.zoom;
            match &mut self.drag {
                Drag::None | Drag::Seek { .. } | Drag::FrameNew(_) => {}
                Drag::Crop { id, handle, start, from } => {
                    let (id, handle, start, from) = (*id, *handle, *start, *from);
                    self.drag_crop(id, handle, start, from, w);
                }
                Drag::FrameResize { id, anchor } => {
                    let (id, r) = (*id, Rect::from_two_pos(*anchor, w));
                    if let Some(it) = self.item_mut(id) {
                        let s = it.scale;
                        if let Some(f) = it.frame.as_mut() {
                            f.w = (r.width() / s).max(10.0);
                            f.h = (r.height() / s).max(10.0);
                        }
                        it.set_center(r.center());
                    }
                }
                Drag::Pan => {
                    self.cam -= delta / zoom;
                    self.anim = None;
                    ctx.set_cursor_icon(egui::CursorIcon::Grabbing);
                }
                Drag::Ink(pts) => {
                    if pts.last().is_none_or(|l| (w - *l).length() * zoom > 1.5) {
                        pts.push(w);
                    }
                }
                Drag::Orbit(id) => {
                    let id = *id;
                    if let Some(it) = self.item_mut(id) {
                        let c = it.cam3d.get_or_insert(CAM3D);
                        c[0] += delta.x * 0.01;
                        c[1] = (c[1] + delta.y * 0.01).clamp(-1.55, 1.55);
                    }
                    ctx.set_cursor_icon(egui::CursorIcon::AllScroll);
                }
                Drag::Move { start, orig, moved, .. } => {
                    let d = w - *start;
                    if !*moved && d.length() * zoom > 2.0 {
                        *moved = true;
                        self.undo.push(self.board.items.clone());
                        self.redo.clear();
                        self.dirty = true;
                    }
                    if *moved {
                        let map: std::collections::HashMap<u64, Pos2> = orig.iter().copied().collect();
                        for it in self.board.items.iter_mut() {
                            if let Some(p) = map.get(&it.id) {
                                it.set_center(*p + d);
                            }
                        }
                        ctx.set_cursor_icon(egui::CursorIcon::Grabbing);
                    }
                }
                Drag::Scale { pivot, corner, orig } => {
                    let a = *corner - *pivot;
                    let f = ((w - *pivot).dot(a) / a.length_sq().max(1e-9)).max(0.01);
                    let map: std::collections::HashMap<u64, (Pos2, f32)> = orig.iter().map(|(id, p, s)| (*id, (*p, *s))).collect();
                    let pivot = *pivot;
                    for it in self.board.items.iter_mut() {
                        if let Some((p, s)) = map.get(&it.id) {
                            it.set_center(pivot + (*p - pivot) * f);
                            it.scale = (s * f).max(1e-5);
                        }
                    }
                }
                Drag::Rotate { center, start_angle, orig, id } => {
                    let v = w - *center;
                    let mut a = *orig + (v.y.atan2(v.x) - *start_angle);
                    if mods.shift {
                        let step = 15f32.to_radians();
                        a = (a / step).round() * step;
                    }
                    let id = *id;
                    if let Some(it) = self.item_mut(id) {
                        it.rot = a;
                    }
                }
                Drag::Marquee { start, additive, base } => {
                    let r = Rect::from_two_pos(*start, w);
                    let mut sel = if *additive { base.clone() } else { HashSet::new() };
                    for it in &self.board.items {
                        if r.intersects(self.board.aabb(it)) {
                            sel.insert(it.id);
                        }
                    }
                    // orden de selección con el rectángulo: del más cercano al punto de partida
                    let start = *start;
                    let mut fresh: Vec<(u64, f32)> = sel.iter().filter(|id| !self.sel_order.contains(id)).filter_map(|id| self.item(*id).map(|i| (*id, self.board.aabb(i).center().distance(start)))).collect();
                    fresh.sort_by(|a, b| a.1.total_cmp(&b.1));
                    self.sel = sel;
                    self.sel_order.retain(|id| self.sel.contains(id));
                    self.sel_order.extend(fresh.into_iter().map(|f| f.0));
                }
            }
        }

        if released {
            match std::mem::replace(&mut self.drag, Drag::None) {
                Drag::Move { moved: false, toggle_on_click: Some(id), .. } => {
                    self.sel.remove(&id);
                }
                Drag::Ink(pts) if !pts.is_empty() => {
                    self.snapshot();
                    let (ink, c) = Ink::from_world(&pts, self.ink_width / self.zoom, self.ink_color);
                    let id = self.board.new_id();
                    let mut it = Item::new(id, 0, c, 1.0);
                    it.ink = Some(ink);
                    self.board.items.push(it);
                }
                Drag::FrameNew(start) => {
                    self.snapshot();
                    let r = Rect::from_two_pos(start, w);
                    // un clic sin arrastrar: marco de tamaño razonable en ese punto
                    let r = if r.width() * self.zoom < 8.0 || r.height() * self.zoom < 8.0 { Rect::from_min_size(start, vec2(640.0, 420.0) / self.zoom) } else { r };
                    let id = self.new_frame(r);
                    self.sel = HashSet::from([id]);
                    self.tool = Tool::Select;
                    self.edit_frame_title(id);
                }
                Drag::Seek { id, bar } if self.audio_players.contains_key(&id) => {
                    let frac = ((pos.x - bar.left()) / bar.width().max(1.0)).clamp(0.0, 1.0) as f64;
                    if let Some(p) = self.audio_players.get_mut(&id) {
                        let d = p.duration;
                        if p.playing() {
                            p.seek_to(frac * d);
                        } else {
                            // en pausa: sólo mueve el cabezal; al reproducir sigue desde ahí
                            p.seek_to(frac * d);
                            p.toggle();
                        }
                    }
                }
                Drag::Seek { id, bar } => {
                    let frac = ((pos.x - bar.left()) / bar.width().max(1.0)).clamp(0.0, 1.0) as f64;
                    if !self.players.contains_key(&id) {
                        // crear el reproductor en pausa
                        self.toggle_play(ctx, id);
                        if let Some(p) = self.players.get_mut(&id) {
                            p.toggle();
                        }
                    }
                    if let Some(p) = self.players.get_mut(&id) {
                        let d = p.duration();
                        p.seek_to(frac * d);
                    }
                }
                _ => {}
            }
        }

        // doble clic: 3D → girarlo; vídeo → reproducir; texto → editar; resto → encuadrar
        if resp.double_clicked() && self.tool == Tool::Select && !self.controls.iter().any(|(r, _)| r.contains(pos)) {
            match self.hit(w) {
                Some(id) => {
                    let it = self.item(id).cloned();
                    match it.as_ref().and_then(|i| if i.text.is_some() { None } else { self.kind_of(i) }) {
                        _ if it.as_ref().is_some_and(|i| i.frame.is_some()) => self.edit_frame_title(id),
                        Some(Kind::Model) => {
                            self.active3d = Some(id);
                            self.toast(ctx, t("Visor 3D: arrastra para girar, rueda para acercar, Esc para salir"));
                        }
                        Some(Kind::Video) | Some(Kind::Audio) => self.toggle_play(ctx, id),
                        _ if it.as_ref().is_some_and(|i| i.text.is_some()) => self.edit_text(id),
                        _ => {
                            if let Some(r) = it.map(|i| self.board.aabb(&i)) {
                                self.fit_rect(r, true);
                            }
                        }
                    }
                }
                None => self.cmd(ctx, Cmd::FitAll),
            }
        }

        // cursores
        if matches!(self.drag, Drag::None) && hovered {
            match self.tool {
                Tool::Draw => ctx.set_cursor_icon(egui::CursorIcon::Crosshair),
                Tool::Text => ctx.set_cursor_icon(egui::CursorIcon::Text),
                Tool::Frame => ctx.set_cursor_icon(egui::CursorIcon::Crosshair),
                Tool::Select => {
                    if self.controls.iter().any(|(r, _)| r.contains(pos)) {
                        ctx.set_cursor_icon(egui::CursorIcon::PointingHand);
                    } else if self.active3d.is_some_and(|id| self.hit(w) == Some(id)) {
                        ctx.set_cursor_icon(egui::CursorIcon::AllScroll);
                    } else if let Some((c, single)) = self.frame_corners() {
                        if single.is_some_and(|id| self.can_rotate(id)) && self.rot_handle(&c).distance(pos) <= HANDLE {
                            ctx.set_cursor_icon(egui::CursorIcon::Alias);
                        } else if let Some(k) = c.iter().position(|p| self.to_screen(*p).distance(pos) <= HANDLE) {
                            ctx.set_cursor_icon(if k % 2 == 0 { egui::CursorIcon::ResizeNwSe } else { egui::CursorIcon::ResizeNeSw });
                        }
                    }
                }
            }
            if space {
                ctx.set_cursor_icon(egui::CursorIcon::Grab);
            }
        }
    }

    fn press_select(&mut self, w: Pos2, pos: Pos2, mods: egui::Modifiers) -> Drag {
        if let Some(id) = self.crop_mode {
            if let Some(it) = self.item(id) {
                let c = crate::board::crop_of(it);
                let from = self.board.world_to_frac(it, w);
                for (k, h) in crop_handles(c).iter().enumerate() {
                    if self.to_screen(self.board.frac_to_world(it, *h)).distance(pos) <= HANDLE + 2.0 {
                        return Drag::Crop { id, handle: k as u8, start: c, from };
                    }
                }
                if from.x >= c[0] && from.x <= c[2] && from.y >= c[1] && from.y <= c[3] {
                    return Drag::Crop { id, handle: 8, start: c, from };
                }
            }
            // clic fuera: se acepta el recorte
            self.crop_mode = None;
            self.dirty = true;
            return Drag::None;
        }
        if let Some(id) = self.active3d.filter(|id| self.hit(w) == Some(*id)) {
            self.snapshot();
            return Drag::Orbit(id);
        }
        if let Some(d) = self.handle_drag(pos) {
            self.snapshot();
            return d;
        }
        // el título de un marco también lo agarra
        let title_hit = self.frame_titles.iter().find(|(r, _)| r.contains(pos)).map(|(_, id)| *id);
        if let Some(id) = title_hit.or_else(|| self.hit(w)) {
            let mut toggle = None;
            if mods.shift || mods.command {
                if self.sel.contains(&id) {
                    toggle = Some(id);
                } else {
                    self.sel.insert(id);
                    self.sel_order.push(id);
                }
            } else if !self.sel.contains(&id) {
                self.sel = HashSet::from([id]);
                self.sel_order = vec![id];
            }
            // un marco arrastra lo que tiene dentro
            let mut moving: HashSet<u64> = self.sel.clone();
            for fid in self.sel.iter().filter(|id| self.item(**id).is_some_and(|i| i.frame.is_some())) {
                moving.extend(self.frame_contents(*fid));
            }
            // lo emparentado va con su padre
            let moving = self.with_descendants(&moving);
            let orig = self.board.items.iter().filter(|i| moving.contains(&i.id)).map(|i| (i.id, i.center())).collect();
            return Drag::Move { start: w, orig, moved: false, toggle_on_click: toggle };
        }
        self.active3d = None;
        let additive = mods.shift || mods.command;
        let base = if additive { self.sel.clone() } else { HashSet::new() };
        if !additive {
            self.sel.clear();
        }
        Drag::Marquee { start: w, additive, base }
    }

    // -----------------------------------------------------------------------------------------
    // Pintado
    // -----------------------------------------------------------------------------------------

    fn paint(&mut self, ctx: &egui::Context, painter: &egui::Painter) {
        let ppp = ctx.pixels_per_point();
        let clip = self.canvas;
        let near = clip.expand2(clip.size());
        let max_side = self.tex.max_side();
        let xf = self.xf();
        let hover_pos = ctx.input(|i| i.pointer.hover_pos()).filter(|p| clip.contains(*p));
        let hover_id = if matches!(self.drag, Drag::None) { hover_pos.and_then(|p| self.hit(self.to_world(p))) } else { None };
        let editing_id = self.editing.as_ref().map(|e| e.id);
        self.controls.clear();
        self.frame_titles.clear();
        if self.grid {
            paint_grid(painter, clip, xf, self.blueprint);
        }
        let mut want_thumbs: Vec<(f32, Request)> = Vec::new();
        let mut want_hi: Vec<(f32, Request)> = Vec::new();
        let mut want_near: Vec<(f32, Request)> = Vec::new();
        let mut overs: Vec<Over> = Vec::new();
        let mut measured: Vec<(usize, Vec2)> = Vec::new();
        let mut need_mesh: Vec<u64> = Vec::new();
        let mut need_links: Vec<(u64, u32)> = Vec::new();
        let font_tex = ctx.fonts(|f| f.font_image_size());

        for idx in 0..self.board.items.len() {
            let it = &self.board.items[idx];
            let wc = self.board.corners(it);
            let sc = wc.map(|p| xf.s(p));
            let bb = Rect::from_points(&sc);
            let on_screen = bb.intersects(clip);

            if let Some(f) = &it.frame {
                if on_screen {
                    let selected = self.sel.contains(&it.id);
                    if let Some(tr) = paint_frame(painter, f, &sc, selected, editing_id == Some(it.id)) {
                        self.frame_titles.push((tr, it.id));
                    }
                }
                continue;
            }
            if let Some(t) = &it.text {
                if on_screen || t.w == 0.0 {
                    let m = paint_text(painter, it, t, xf, ppp, font_tex, editing_id == Some(it.id));
                    measured.push((idx, m));
                }
                continue;
            }
            if let Some(k) = &it.ink {
                if on_screen {
                    paint_ink(painter, it, k, xf);
                }
                continue;
            }
            let Some(src) = self.board.sources.get(&it.src) else { continue };

            if src.kind == Kind::Audio {
                if on_screen {
                    let abb = Rect::from_points(&sc);
                    let (id, srcid) = (it.id, it.src);
                    self.ensure_audio(ctx, id);
                    self.paint_audio(painter, id, srcid, abb);
                }
                continue;
            }
            if src.kind == Kind::Model {
                if !on_screen {
                    continue;
                }
                let mbb = Rect::from_points(&sc);
                painter.rect_filled(mbb, CornerRadius::same(4), Color32::from_rgb(38, 40, 46));
                match self.meshes.get_mut(&it.src) {
                    None => need_mesh.push(it.src),
                    Some(MeshState::Loading) => {
                        painter.text(mbb.center(), egui::Align2::CENTER_CENTER, t("Cargando 3D…"), FontId::proportional(13.0), Color32::from_gray(150));
                    }
                    Some(MeshState::Failed) => {
                        painter.line_segment([sc[0], sc[2]], Stroke::new(1.0, Color32::from_rgb(160, 60, 60)));
                        painter.line_segment([sc[1], sc[3]], Stroke::new(1.0, Color32::from_rgb(160, 60, 60)));
                    }
                    Some(MeshState::Ready { mesh, gpu, soft }) => {
                        let cam = it.cam3d.unwrap_or(CAM3D);
                        let base = if it.gray { [0.8, 0.8, 0.8] } else { [0.72, 0.78, 0.92] };
                        if !crate::gpu3d::failed(gpu) {
                            let c = crate::gpu3d::Cam { yaw: cam[0], pitch: cam[1], zoom: cam[2], persp: !it.ortho3d, base };
                            painter.add(crate::gpu3d::callback(mbb, mesh.clone(), gpu.clone(), c));
                            if crate::gpu3d::pending(gpu) {
                                ctx.request_repaint(); // si la GPU falla en la primera pasada se pasa al software
                            }
                        } else {
                            // sin GPU: el rasterizador por software de File Browser
                            let rw = ((mbb.width() * ppp) as usize).clamp(64, 900);
                            let rh = ((mbb.height() * ppp) as usize).clamp(64, 900);
                            let key = (rw, rh, (cam[0] * 1000.0) as i32, (cam[1] * 1000.0) as i32, (cam[2] * 1000.0) as i32);
                            if soft.as_ref().is_none_or(|(k, _)| *k != key) {
                                let px = crate::mesh3d::render(mesh, cam[0], cam[1], cam[2], rw, rh, [38, 40, 46], [184, 199, 235], true);
                                let img = egui::ColorImage::from_rgba_unmultiplied([rw, rh], &px);
                                *soft = Some((key, ctx.load_texture("3d", img, egui::TextureOptions::LINEAR)));
                            }
                            if let Some((_, t)) = soft {
                                painter.image(t.id(), mbb, Rect::from_min_max(Pos2::ZERO, pos2(1.0, 1.0)), Color32::WHITE);
                            }
                        }
                    }
                }
                let active = self.active3d == Some(it.id);
                if active || hover_id == Some(it.id) {
                    overs.push(Over::Model { id: it.id, bb: mbb, active, ortho: it.ortho3d });
                }
                continue;
            }

            // imagen, póster de vídeo o página de documento: texturas por niveles de detalle
            let page = if src.kind == Kind::Doc { it.page } else { 0 };
            let job_bytes = match src.kind {
                Kind::Video => src.poster.clone(),
                _ => Some(src.bytes.clone()),
            };
            let native = src.native();
            let make_job = |b: crate::decode::Bytes| if src.kind == Kind::Doc { Job::Doc(b) } else { Job::Image(b) };
            if !on_screen {
                if bb.intersects(near)
                    && let Some(b) = job_bytes
                {
                    let k = Key { src: it.src, page, level: 0, gray: it.gray };
                    if !self.tex.has(&k) {
                        want_near.push((-bb.center().distance(clip.center()), Request { key: k, job: make_job(b), side: textures::level_side(0, native, max_side) }));
                    }
                }
                continue;
            }
            let k0 = Key { src: it.src, page, level: 0, gray: it.gray };
            // el nivel de detalle va por la imagen completa (recortada se ve un trozo a su escala)
            let side_px = (self.board.full_size(it).max_elem()) * self.zoom * ppp;
            let want = textures::level_for(side_px, native, max_side);
            let area = bb.area();
            if let Some(b) = &job_bytes {
                if !self.tex.has(&k0) {
                    want_thumbs.push((area, Request { key: k0, job: make_job(b.clone()), side: textures::level_side(0, native, max_side) }));
                }
                let k = Key { level: want, ..k0 };
                if want > 0 && !self.tex.has(&k) {
                    want_hi.push((area, Request { key: k, job: make_job(b.clone()), side: textures::level_side(want, native, max_side) }));
                }
            }
            let failed = self.tex.failed(&k0);
            // un vídeo en marcha (o en pausa tras reproducir) pinta su fotograma
            let video_tex = self.players.get(&it.id).and_then(|p| p.tex.as_ref()).map(|t| t.id());
            let tex = video_tex.or_else(|| self.tex.best(it.src, page, it.gray, want).or_else(|| self.tex.best(it.src, page, !it.gray, want)).map(|t| t.0));
            match tex {
                Some(tid) => {
                    // en modo recorte, la imagen entera atenuada debajo de la parte que queda
                    if self.crop_mode == Some(it.id) {
                        let full = [vec2(0.0, 0.0), vec2(1.0, 0.0), vec2(1.0, 1.0), vec2(0.0, 1.0)].map(|u| xf.s(self.board.frac_to_world(it, u)));
                        draw_quad(painter, tid, full, it.flip, [0.0, 0.0, 1.0, 1.0], Color32::from_gray(70));
                    }
                    draw_quad(painter, tid, sc, it.flip, crate::board::crop_of(it), Color32::WHITE);
                }
                None => {
                    let fill = if failed { Color32::from_rgb(70, 30, 30) } else { Color32::from_gray(48) };
                    painter.add(egui::Shape::convex_polygon(sc.to_vec(), fill, Stroke::new(1.0, Color32::from_gray(64))));
                    if failed {
                        painter.line_segment([sc[0], sc[2]], Stroke::new(1.0, Color32::from_rgb(160, 60, 60)));
                        painter.line_segment([sc[1], sc[3]], Stroke::new(1.0, Color32::from_rgb(160, 60, 60)));
                    }
                }
            }
            if src.kind == Kind::Doc && hover_id == Some(it.id) {
                need_links.push((it.src, it.page));
            }
            let shown = hover_id == Some(it.id) || self.sel.contains(&it.id);
            match src.kind {
                Kind::Video if shown || self.players.get(&it.id).is_some_and(|p| p.playing()) => overs.push(Over::Video { id: it.id, bb }),
                Kind::Doc if shown && src.pages > 1 => overs.push(Over::Doc { id: it.id, bb, page: it.page, pages: src.pages }),
                _ => {}
            }
        }

        for (idx, m) in measured {
            if let Some(t) = self.board.items.get_mut(idx).and_then(|i| i.text.as_mut()) {
                t.w = m.x;
                t.h = m.y;
            }
        }
        for src in need_mesh {
            self.want_mesh(ctx, src);
        }
        for (src, page) in need_links {
            self.want_doc_links(ctx, src, page);
        }
        // con Ctrl pulsado, las zonas de enlace del documento bajo el cursor se marcan
        if ctx.input(|i| i.modifiers.command)
            && let Some(it) = hover_id.and_then(|id| self.item(id))
            && let Some(links) = self.doc_links.get(&(it.src, it.page)).filter(|l| !l.is_empty())
        {
            let size = self.board.size(it);
            let rot = egui::emath::Rot2::from_angle(it.rot);
            for k in links {
                let corner = |fx: f32, fy: f32| {
                    let mut l = vec2(fx, fy) * size - size * 0.5;
                    if it.flip {
                        l.x = -l.x;
                    }
                    xf.s(it.center() + rot * l)
                };
                let pts = vec![corner(k.x, k.y), corner(k.x + k.w, k.y), corner(k.x + k.w, k.y + k.h), corner(k.x, k.y + k.h)];
                painter.add(egui::Shape::convex_polygon(pts, ACCENT.gamma_multiply(0.15), Stroke::new(1.0, ACCENT)));
            }
        }

        // cola: miniaturas de lo visible (por área), niveles altos y lo que está cerca
        let by = |v: &mut Vec<(f32, Request)>| v.sort_by(|a, b| b.0.total_cmp(&a.0));
        by(&mut want_thumbs);
        by(&mut want_hi);
        by(&mut want_near);
        let wanted: Vec<Request> = want_thumbs.into_iter().chain(want_hi).chain(want_near.into_iter().take(64)).map(|(_, r)| r).collect();
        self.tex.set_wanted(wanted);

        // marco en curso
        if let (Drag::FrameNew(start), Some(p)) = (&self.drag, hover_pos.or(ctx.input(|i| i.pointer.latest_pos()))) {
            let r = Rect::from_two_pos(xf.s(*start), p);
            painter.rect(r, CornerRadius::same(6), Color32::from_white_alpha(10), Stroke::new(1.5, ACCENT), StrokeKind::Inside);
        }
        // trazo en curso
        if let Drag::Ink(pts) = &self.drag {
            let (ink, c) = Ink::from_world(pts, self.ink_width / self.zoom, self.ink_color);
            let mut it = Item::new(0, 0, c, 1.0);
            it.ink = Some(ink.clone());
            paint_ink(painter, &it, &ink, xf);
        }

        // controles de vídeo, documentos y 3D
        for o in overs {
            self.paint_over(ctx, painter, o, hover_pos);
        }

        // selección
        for it in self.board.items.iter().filter(|i| self.sel.contains(&i.id) && Some(i.id) != editing_id) {
            let sc = self.board.corners(it).map(|p| xf.s(p));
            painter.add(egui::Shape::closed_line(sc.to_vec(), Stroke::new(1.5, ACCENT)));
        }
        self.paint_crop(painter, xf);
        if self.tool == Tool::Select
            && editing_id.is_none()
            && self.crop_mode.is_none()
            && let Some((c, single)) = self.frame_corners()
        {
            let sc = c.map(|p| xf.s(p));
            match single {
                None => {
                    painter.add(egui::Shape::dashed_line(&[sc[0], sc[1], sc[2], sc[3], sc[0]], Stroke::new(1.0, ACCENT), 6.0, 4.0));
                }
                Some(id) if self.can_rotate(id) => {
                    let rh = self.rot_handle(&c);
                    let top = sc[0].lerp(sc[1], 0.5);
                    painter.line_segment([top, rh], Stroke::new(1.0, ACCENT));
                    painter.circle(rh, 5.0, Color32::WHITE, Stroke::new(1.5, ACCENT));
                }
                _ => {}
            }
            for p in sc {
                painter.rect(Rect::from_center_size(p, Vec2::splat(8.0)), CornerRadius::same(1), Color32::WHITE, Stroke::new(1.5, ACCENT), StrokeKind::Middle);
            }
        }
        self.paint_links(painter, xf, hover_pos);
        if let (Drag::Marquee { start, .. }, Some(p)) = (&self.drag, ctx.input(|i| i.pointer.latest_pos())) {
            let r = Rect::from_two_pos(xf.s(*start), p);
            painter.rect(r, CornerRadius::ZERO, ACCENT.gamma_multiply(0.12), Stroke::new(1.0, ACCENT), StrokeKind::Inside);
        }
    }

    /// Emparentado: líneas discontinuas entre lo seleccionado y lo que va unido a ello (si se activan
    /// en Ajustes; el botón para unir o separar está en la barra superior).
    fn paint_links(&mut self, painter: &egui::Painter, xf: Xf, _hover: Option<Pos2>) {
        if self.tool != Tool::Select || self.editing.is_some() || !matches!(self.drag, Drag::None) || self.sel.is_empty() {
            return;
        }
        if !self.cfg.show_links {
            return;
        }
        let line = Stroke::new(1.2, ACCENT.gamma_multiply(0.7));
        for it in &self.board.items {
            let Some(p) = it.parent.and_then(|p| self.item(p)) else { continue };
            if self.sel.contains(&it.id) || self.sel.contains(&p.id) {
                let (a, b) = (xf.s(it.center()), xf.s(p.center()));
                painter.add(egui::Shape::dashed_line(&[a, b], line, 5.0, 4.0));
                painter.circle_filled(a, 3.0, ACCENT);
            }
        }
    }

    /// Ajusta el recorte mientras se arrastra un tirador; la imagen completa no se mueve.
    fn drag_crop(&mut self, id: u64, handle: u8, start: [f32; 4], from: Vec2, w: Pos2) {
        let Some(it) = self.item(id) else { return };
        let full_center = self.board.full_center(it);
        let u = self.board.world_to_frac(it, w);
        let (ux, uy) = (u.x.clamp(0.0, 1.0), u.y.clamp(0.0, 1.0));
        let min = 0.02;
        let [mut x0, mut y0, mut x1, mut y1] = start;
        match handle {
            8 => {
                let d = u - from;
                let (cw, ch) = (start[2] - start[0], start[3] - start[1]);
                x0 = (start[0] + d.x).clamp(0.0, 1.0 - cw);
                y0 = (start[1] + d.y).clamp(0.0, 1.0 - ch);
                x1 = x0 + cw;
                y1 = y0 + ch;
            }
            _ => {
                if matches!(handle, 0 | 6 | 7) {
                    x0 = ux.min(x1 - min);
                }
                if matches!(handle, 2..=4) {
                    x1 = ux.max(x0 + min);
                }
                if matches!(handle, 0..=2) {
                    y0 = uy.min(y1 - min);
                }
                if matches!(handle, 4..=6) {
                    y1 = uy.max(y0 + min);
                }
            }
        }
        let crop = [x0, y0, x1, y1];
        let (f, rot, flip) = (self.board.full_size(it), it.rot, it.flip);
        let mut off = vec2(((x0 + x1) * 0.5 - 0.5) * f.x, ((y0 + y1) * 0.5 - 0.5) * f.y);
        if flip {
            off.x = -off.x;
        }
        let center = full_center + egui::emath::Rot2::from_angle(rot) * off;
        if let Some(it) = self.item_mut(id) {
            it.crop = if crop == [0.0, 0.0, 1.0, 1.0] { None } else { Some(crop) };
            it.set_center(center);
        }
        self.dirty = true;
    }

    /// Marco del recorte con sus ocho tiradores.
    fn paint_crop(&self, painter: &egui::Painter, xf: Xf) {
        let Some(it) = self.crop_mode.and_then(|id| self.item(id)) else { return };
        let c = crate::board::crop_of(it);
        let s = |u: Vec2| xf.s(self.board.frac_to_world(it, u));
        let box_ = [vec2(c[0], c[1]), vec2(c[2], c[1]), vec2(c[2], c[3]), vec2(c[0], c[3])].map(s);
        painter.add(egui::Shape::closed_line(box_.to_vec(), Stroke::new(1.5, Color32::WHITE)));
        // tercios, como guía
        for k in 1..3 {
            let f = k as f32 / 3.0;
            let (x, y) = (c[0] + (c[2] - c[0]) * f, c[1] + (c[3] - c[1]) * f);
            painter.line_segment([s(vec2(x, c[1])), s(vec2(x, c[3]))], Stroke::new(1.0, Color32::from_white_alpha(70)));
            painter.line_segment([s(vec2(c[0], y)), s(vec2(c[2], y))], Stroke::new(1.0, Color32::from_white_alpha(70)));
        }
        for h in crop_handles(c) {
            painter.rect(Rect::from_center_size(s(h), Vec2::splat(9.0)), CornerRadius::same(2), Color32::WHITE, Stroke::new(1.5, ACCENT), StrokeKind::Middle);
        }
    }

    /// Mini reproductor de audio: botón, título, onda (clic para saltar), tiempo y carátula.
    fn paint_audio(&mut self, painter: &egui::Painter, id: u64, src: u64, bb: Rect) {
        let (playing, pos, dur, title, cover) = match self.audio_players.get(&id) {
            Some(p) => (p.playing(), p.position(), p.duration, if p.title.is_empty() { None } else { Some(format!("{}{}", p.title, if p.artist.is_empty() { String::new() } else { format!(" — {}", p.artist) })) }, p.tex.as_ref().map(|t| t.id())),
            None => (false, 0.0, 0.0, None, None),
        };
        let s = self.board.sources.get(&src);
        let dur = if dur > 0.0 { dur } else { s.map(|s| s.duration as f64).unwrap_or(0.0) };
        let name = title.unwrap_or_else(|| s.map(|s| s.name.clone()).unwrap_or_default());
        let h = bb.height();
        let round = CornerRadius::same((h * 0.12).clamp(2.0, 18.0) as u8);
        painter.rect_filled(bb, round, Color32::from_rgb(38, 38, 44));
        painter.rect_stroke(bb, round, Stroke::new(1.0, Color32::from_white_alpha(20)), StrokeKind::Inside);
        if h < 24.0 {
            return;
        }
        let pad = h * 0.12;
        // carátula a la izquierda si la hay; el botón de reproducir encima
        let art = Rect::from_min_size(bb.min + vec2(pad, pad), Vec2::splat(h - 2.0 * pad));
        match cover {
            Some(t) => {
                painter.image(t, art, Rect::from_min_max(Pos2::ZERO, pos2(1.0, 1.0)), Color32::WHITE);
            }
            None => {
                painter.rect_filled(art, CornerRadius::same(round.nw / 2), Color32::from_rgb(56, 52, 90));
            }
        }
        let c = art.center();
        let r = art.width() * 0.3;
        painter.circle_filled(c, r, Color32::from_black_alpha(150));
        if playing {
            for dx in [-r * 0.28, r * 0.28] {
                painter.rect_filled(Rect::from_center_size(c + vec2(dx, 0.0), vec2(r * 0.22, r * 0.9)), CornerRadius::ZERO, Color32::WHITE);
            }
        } else {
            play_icon(painter, c + vec2(r * 0.1, 0.0), r * 0.5, Color32::WHITE);
        }
        self.controls.push((art, Control::PlayPause(id)));
        let x0 = art.right() + pad;
        let x1 = bb.right() - pad;
        if x1 - x0 < 30.0 {
            return;
        }
        let fs = (h * 0.15).clamp(8.0, 22.0);
        let clip = painter.with_clip_rect(Rect::from_min_max(pos2(x0, bb.top()), pos2(x1, bb.bottom())).intersect(painter.clip_rect()));
        let time = format!("{} / {}", media::fmt_time(pos), media::fmt_time(dur));
        let tg = painter.layout_no_wrap(time, FontId::proportional(fs * 0.85), Color32::from_gray(170));
        clip.text(pos2(x0, bb.top() + pad), egui::Align2::LEFT_TOP, name, FontId::proportional(fs), Color32::from_gray(230));
        clip.galley(pos2(x1 - tg.size().x, bb.bottom() - pad - tg.size().y), tg, Color32::WHITE);
        // forma de onda: lo reproducido en el color de acento
        let wave = Rect::from_min_max(pos2(x0, bb.top() + pad + fs * 1.5), pos2(x1, bb.bottom() - pad - fs * 1.2));
        if wave.height() > 4.0 {
            let frac = if dur > 0.0 { (pos / dur).clamp(0.0, 1.0) as f32 } else { 0.0 };
            let peaks: &[u8] = s.map(|s| s.peaks.as_slice()).unwrap_or(&[]);
            let n = if peaks.is_empty() { 80 } else { peaks.len() };
            let bw = wave.width() / n as f32;
            for k in 0..n {
                let v = peaks.get(k).map(|p| *p as f32 / 255.0).unwrap_or(0.15).max(0.04);
                let x = wave.left() + (k as f32 + 0.5) * bw;
                let hh = wave.height() * v;
                let col = if (k as f32 + 0.5) / n as f32 <= frac { ACCENT } else { Color32::from_gray(95) };
                let r = Rect::from_center_size(pos2(x, wave.center().y), vec2((bw * 0.6).max(1.0), hh.max(1.0)));
                painter.rect_filled(r, CornerRadius::ZERO, col);
            }
            self.controls.push((wave, Control::Seek(id)));
        }
    }

    fn paint_over(&mut self, ctx: &egui::Context, painter: &egui::Painter, o: Over, hover: Option<Pos2>) {
        let small = FontId::proportional(12.0);
        match o {
            Over::Video { id, bb } => {
                if bb.width() < 120.0 || bb.height() < 60.0 {
                    return;
                }
                let (playing, pos, dur, muted) = match self.players.get(&id) {
                    Some(p) => (p.playing(), p.position(), p.duration(), p.muted),
                    None => {
                        let d = self.item(id).and_then(|i| self.board.sources.get(&i.src)).map(|s| s.duration as f64).unwrap_or(0.0);
                        (false, 0.0, d, false)
                    }
                };
                // botón grande en el centro mientras está parado
                if !playing {
                    let c = bb.center();
                    painter.circle_filled(c, 26.0, Color32::from_black_alpha(150));
                    play_icon(painter, c, 11.0, Color32::WHITE);
                    self.controls.push((Rect::from_center_size(c, Vec2::splat(52.0)), Control::PlayPause(id)));
                }
                let bar = Rect::from_min_max(pos2(bb.left() + 8.0, bb.bottom() - 36.0), pos2(bb.right() - 8.0, bb.bottom() - 8.0));
                painter.rect_filled(bar, CornerRadius::same(6), Color32::from_black_alpha(170));
                let pp = Rect::from_min_size(bar.min, Vec2::splat(bar.height()));
                if playing {
                    for dx in [-4.0, 3.0] {
                        painter.rect_filled(Rect::from_center_size(pp.center() + vec2(dx, 0.0), vec2(3.5, 12.0)), CornerRadius::ZERO, Color32::WHITE);
                    }
                } else {
                    play_icon(painter, pp.center(), 6.0, Color32::WHITE);
                }
                self.controls.push((pp, Control::PlayPause(id)));
                let mute = Rect::from_min_size(pos2(bar.right() - bar.height(), bar.top()), Vec2::splat(bar.height()));
                speaker_icon(painter, mute.center(), muted);
                self.controls.push((mute, Control::Mute(id)));
                let t = format!("{} / {}", media::fmt_time(pos), media::fmt_time(dur));
                let g = painter.layout_no_wrap(t, small.clone(), Color32::from_gray(220));
                let show_time = bar.width() > 260.0;
                let tw = if show_time { g.size().x + 12.0 } else { 0.0 };
                let track = Rect::from_min_max(pos2(pp.right() + 4.0, bar.top()), pos2(mute.left() - 4.0 - tw, bar.bottom()));
                if show_time {
                    painter.galley(pos2(track.right() + 6.0, bar.center().y - g.size().y * 0.5), g, Color32::WHITE);
                }
                if track.width() > 20.0 {
                    let line = Rect::from_center_size(track.center(), vec2(track.width(), 4.0));
                    painter.rect_filled(line, CornerRadius::same(2), Color32::from_gray(90));
                    let mut frac = if dur > 0.0 { (pos / dur).clamp(0.0, 1.0) as f32 } else { 0.0 };
                    if let (Drag::Seek { id: sid, .. }, Some(h)) = (&self.drag, hover)
                        && *sid == id
                    {
                        frac = ((h.x - track.left()) / track.width()).clamp(0.0, 1.0);
                    }
                    let done = Rect::from_min_max(line.min, pos2(line.left() + line.width() * frac, line.bottom()));
                    painter.rect_filled(done, CornerRadius::same(2), ACCENT);
                    painter.circle_filled(pos2(done.right(), line.center().y), 5.0, Color32::WHITE);
                    self.controls.push((track, Control::Seek(id)));
                }
            }
            Over::Doc { id, bb, page, pages } => {
                if bb.width() < 110.0 || bb.height() < 50.0 {
                    return;
                }
                let pill = Rect::from_center_size(pos2(bb.center().x, bb.bottom() - 22.0), vec2(130.0, 26.0));
                painter.rect_filled(pill, CornerRadius::same(13), Color32::from_black_alpha(180));
                let prev = Rect::from_min_size(pill.min, vec2(32.0, pill.height()));
                let next = Rect::from_min_size(pos2(pill.right() - 32.0, pill.top()), vec2(32.0, pill.height()));
                let dim = |on: bool| if on { Color32::WHITE } else { Color32::from_gray(90) };
                painter.text(prev.center(), egui::Align2::CENTER_CENTER, "‹", FontId::proportional(20.0), dim(page > 0));
                painter.text(next.center(), egui::Align2::CENTER_CENTER, "›", FontId::proportional(20.0), dim(page + 1 < pages));
                painter.text(pill.center(), egui::Align2::CENTER_CENTER, format!("{} / {}", page + 1, pages), small, Color32::WHITE);
                self.controls.push((prev, Control::PagePrev(id)));
                self.controls.push((next, Control::PageNext(id)));
            }
            Over::Model { id, bb, active, ortho } => {
                // las etiquetas se quedan dentro de la parte visible del marco
                let vis = bb.intersect(self.canvas);
                if active {
                    painter.rect_stroke(bb, CornerRadius::same(4), Stroke::new(2.0, ACTIVE3D), StrokeKind::Outside);
                    painter.text(vis.left_top() + vec2(10.0, 8.0), egui::Align2::LEFT_TOP, t("Arrastra: girar · Rueda: zoom · Esc: salir"), small.clone(), ACTIVE3D);
                } else if vis.width() > 120.0 {
                    let r = Rect::from_min_size(vis.left_top() + vec2(8.0, 8.0), vec2(150.0, 22.0));
                    painter.rect_filled(r, CornerRadius::same(11), Color32::from_black_alpha(160));
                    painter.text(r.center(), egui::Align2::CENTER_CENTER, t("3D · doble clic: girar"), small.clone(), Color32::WHITE);
                }
                // píldora para cambiar de proyección
                if vis.width() > 200.0 && vis.height() > 60.0 {
                    let label = if ortho { t("Ortográfica") } else { t("Perspectiva") };
                    let g = painter.layout_no_wrap(label.to_string(), small, Color32::WHITE);
                    let r = Rect::from_min_size(pos2(vis.right() - g.size().x - 28.0, vis.top() + 8.0), vec2(g.size().x + 20.0, 22.0));
                    let hot = hover.is_some_and(|h| r.contains(h));
                    painter.rect_filled(r, CornerRadius::same(11), if hot { ACCENT } else { Color32::from_black_alpha(160) });
                    painter.galley(r.center() - g.size() * 0.5, g, Color32::WHITE);
                    self.controls.push((r, Control::Projection(id)));
                }
                let _ = ctx;
            }
        }
    }

    // -----------------------------------------------------------------------------------------
    // Texto en edición
    // -----------------------------------------------------------------------------------------

    fn text_editor(&mut self, ctx: &egui::Context) {
        let Some(e) = self.editing.as_mut() else { return };
        let Some(it) = self.board.items.iter_mut().find(|i| i.id == e.id) else {
            self.editing = None;
            return;
        };
        let (center_w, scale) = (it.center(), it.scale);
        let zoom = self.zoom;
        let center = self.canvas.center() + (center_w - self.cam) * zoom;

        // título de un marco: una línea sobre su borde superior
        if e.title {
            let Some(f) = it.frame.as_ref() else { return };
            let tl = center - vec2(f.w, f.h) * scale * zoom * 0.5 - vec2(0.0, 26.0);
            egui::Area::new(egui::Id::new("frame-title-edit")).fixed_pos(tl).order(egui::Order::Foreground).show(ctx, |ui| {
                let r = ui.add(egui::TextEdit::singleline(&mut e.buf).font(FontId::proportional(14.0)).desired_width(220.0).hint_text(t("Título del marco")));
                if e.focus {
                    r.request_focus();
                    e.focus = false;
                }
                if r.lost_focus() && ui.input(|i| i.key_pressed(K::Enter)) {
                    e.focus = false;
                    ui.ctx().data_mut(|d| d.insert_temp(egui::Id::new("commit-title"), true));
                }
            });
            if ctx.data_mut(|d| d.remove_temp::<bool>(egui::Id::new("commit-title"))).is_some() {
                self.commit_text();
            }
            return;
        }

        // el texto crece desde su borde (el izquierdo si está alineado a la izquierda), no desde el
        // centro: si no, al escribir se iba desplazando hacia la izquierda
        let Some((tw, th, align)) = it.text.as_ref().map(|t| (t.w, t.h, t.align)) else { return };
        let rot = egui::emath::Rot2::from_angle(it.rot);
        let sx = if it.flip { -1.0 } else { 1.0 };
        let local = vec2(sx * match align {
            1 => 0.0,
            2 => tw * 0.5,
            _ => -tw * 0.5,
        }, -th * 0.5) * scale;
        match e.anchor {
            None => e.anchor = Some(center_w + rot * local),
            Some(a) => it.set_center(a - rot * local),
        }
        let center = self.canvas.center() + (it.center() - self.cam) * zoom;
        let Some(txt) = it.text.as_mut() else { return };
        // el texto se actualiza mientras se escribe (guardar a mitad no pierde nada)
        txt.s = e.buf.clone();
        let fs = (txt.size * scale * zoom).clamp(8.0, 120.0);
        let half = vec2(txt.w, txt.h) * scale * 0.5;
        let tl = center - half * zoom - vec2(6.0, 4.0);
        let width = (txt.w * scale * zoom).max(fs * 4.0) + fs * 2.0;
        let fmt = txt.clone();
        let mut layouter = move |ui: &egui::Ui, buf: &dyn egui::TextBuffer, _w: f32| {
            let mut job = text_job(&fmt, buf.as_str(), fs);
            job.halign = egui::Align::LEFT;
            ui.ctx().fonts_mut(|f| f.layout_job(job))
        };
        let ar = egui::Area::new(egui::Id::new("text-edit")).fixed_pos(tl).order(egui::Order::Foreground).show(ctx, |ui| {
            egui::Frame::new().fill(Color32::from_black_alpha(140)).stroke(Stroke::new(1.0, ACCENT)).corner_radius(CornerRadius::same(4)).inner_margin(egui::Margin::symmetric(6, 4)).show(ui, |ui| {
                let r = ui.add(
                    egui::TextEdit::multiline(&mut e.buf)
                        .layouter(&mut layouter)
                        .frame(egui::Frame::NONE)
                        .desired_rows(1)
                        .desired_width(width)
                        .hint_text(t("Escribe…")),
                );
                if e.focus {
                    r.request_focus();
                    e.focus = false;
                }
            });
        });
        let id = e.id;
        self.format_bar(ctx, id, ar.response.rect);
    }

    fn overlays(&mut self, ctx: &egui::Context, painter: &egui::Painter) {
        let r = self.canvas;
        let dim = Color32::from_gray(150);
        if self.board.items.is_empty() && self.batches.is_empty() {
            painter.text(r.center() - vec2(0.0, 14.0), egui::Align2::CENTER_CENTER, t("Suelta imágenes, vídeos, audio, PDF o modelos 3D aquí, o pega con Ctrl+V"), egui::FontId::proportional(20.0), dim);
            painter.text(r.center() + vec2(0.0, 16.0), egui::Align2::CENTER_CENTER, t("Ctrl+O abre un tablero · T texto · D dibujar · F1 atajos · clic derecho: menú"), egui::FontId::proportional(14.0), Color32::from_gray(110));
        }
        let status = if !self.cfg.show_status {
            String::new()
        } else {
            format!(
            "{} · {:.0}% · VRAM {} MB{}",
            tn("{} elemento|{} elementos", self.board.items.len(), &[]),
            self.zoom * ctx.pixels_per_point() * 100.0,
            self.tex.vram() >> 20,
            if self.tex.busy() || !self.batches.is_empty() { format!(" · {}", t("cargando…")) } else { String::new() }
            )
        };
        painter.text(r.left_bottom() + vec2(10.0, -8.0), egui::Align2::LEFT_BOTTOM, status, egui::FontId::proportional(12.0), Color32::from_gray(120));

        let now = ctx.input(|i| i.time);
        if let Some((msg, until)) = &self.toast {
            if now < *until {
                let g = painter.layout_no_wrap(msg.clone(), egui::FontId::proportional(14.0), Color32::WHITE);
                let pr = Rect::from_center_size(pos2(r.center().x, r.bottom() - 40.0), g.size() + vec2(24.0, 12.0));
                painter.rect_filled(pr, CornerRadius::same(6), Color32::from_black_alpha(200));
                painter.galley(pr.center() - g.size() * 0.5, g, Color32::WHITE);
            } else {
                self.toast = None;
            }
        }

        if !self.errors.is_empty() {
            let mut close = false;
            egui::Window::new(t("Avisos")).collapsible(false).resizable(false).anchor(egui::Align2::RIGHT_TOP, vec2(-12.0, 12.0)).show(ctx, |ui| {
                for e in self.errors.iter().rev().take(8) {
                    ui.label(e);
                }
                if self.errors.len() > 8 {
                    ui.label(tf("… y {} más", &[&(self.errors.len() - 8)]));
                }
                close = ui.button(t("Cerrar")).clicked();
            });
            if close {
                self.errors.clear();
            }
        }

        if self.help {
            egui::Window::new(t("Atajos de teclado")).collapsible(false).resizable(false).anchor(egui::Align2::CENTER_CENTER, Vec2::ZERO).show(ctx, |ui| {
                egui::Grid::new("help").num_columns(2).spacing([24.0, 4.0]).show(ui, |ui| {
                    for (k, d) in HELP {
                        ui.strong(t(k));
                        ui.label(t(d));
                        ui.end_row();
                    }
                });
            });
        }
    }

    /// Menú contextual: sólo lo que se hace con el tablero y sus elementos; abrir, guardar e
    /// importar están en el menú principal. Al final, el submenú de ajustes.
    fn context_menu(&mut self, ctx: &egui::Context, resp: &egui::Response) {
        let cmd = std::cell::Cell::new(None);
        let color_frame = std::cell::Cell::new(None);
        let has_sel = !self.sel.is_empty();
        let frame_sel = (self.sel.len() == 1).then(|| *self.sel.iter().next().unwrap()).filter(|id| self.item(*id).is_some_and(|i| i.frame.is_some()));
        let kinds: HashSet<Kind> = self.board.items.iter().filter(|i| self.sel.contains(&i.id)).filter_map(|i| self.kind_of(i)).collect();
        let linked = self.selection_linked();
        resp.context_menu(|ui| {
            ui.set_min_width(240.0);
            let item = |ui: &mut egui::Ui, label: &str, short: &str, c: Cmd, enabled: bool| {
                if ui.add_enabled(enabled, egui::Button::new(label).shortcut_text(short)).clicked() {
                    cmd.set(Some(c));
                    ui.close();
                }
            };
            item(ui, t("Pegar"), "Ctrl+V", Cmd::Paste, true);
            item(ui, t("Copiar"), "Ctrl+C", Cmd::Copy, has_sel);
            item(ui, t("Duplicar"), "Ctrl+D", Cmd::Duplicate, has_sel);
            item(ui, t("Eliminar"), t("Supr"), Cmd::Delete, has_sel);
            ui.separator();
            item(ui, t("Texto"), "T", Cmd::Tool(Tool::Text), true);
            item(ui, t("Dibujar"), "D", Cmd::Tool(Tool::Draw), true);
            item(ui, t("Marco"), "K", Cmd::Tool(Tool::Frame), true);
            item(ui, t("Enmarcar selección"), "Ctrl+G", Cmd::FrameSelection, has_sel);
            if let Some(fid) = frame_sel
                && ui.button(t("Color del marco…")).clicked()
            {
                color_frame.set(Some(fid));
                ui.close();
            }
            item(ui, if linked { t("Romper emparentado") } else { t("Emparentar") }, "Ctrl+L", Cmd::Link, linked || self.sel.len() >= 2);
            if kinds.contains(&Kind::Video) || kinds.contains(&Kind::Audio) {
                item(ui, t("Reproducir / pausa"), "P", Cmd::PlayPause, true);
            }
            if kinds.contains(&Kind::Video) {
                item(ui, t("Silenciar"), t("Mayús+M"), Cmd::Mute, true);
            }
            if kinds.contains(&Kind::Doc) {
                item(ui, t("Página anterior"), ",", Cmd::Page(-1), true);
                item(ui, t("Página siguiente"), ".", Cmd::Page(1), true);
                item(ui, t("Expandir todas las páginas"), "E", Cmd::ExpandPages, true);
            }
            ui.separator();
            item(ui, t("Ordenar"), "A", Cmd::Arrange, true);
            ui.add_enabled_ui(self.sel.len() >= 2, |ui| {
                ui.menu_button(t("Alinear"), |ui| {
                    item(ui, t("A la izquierda"), "Alt+←", Cmd::Align(0), true);
                    item(ui, t("Centro horizontal"), "Alt+H", Cmd::Align(1), true);
                    item(ui, t("A la derecha"), "Alt+→", Cmd::Align(2), true);
                    ui.separator();
                    item(ui, t("Arriba"), "Alt+↑", Cmd::Align(3), true);
                    item(ui, t("Centro vertical"), "Alt+V", Cmd::Align(4), true);
                    item(ui, t("Abajo"), "Alt+↓", Cmd::Align(5), true);
                    ui.separator();
                    item(ui, t("Repartir en horizontal"), "", Cmd::Align(6), self.sel.len() >= 3);
                    item(ui, t("Repartir en vertical"), "", Cmd::Align(7), self.sel.len() >= 3);
                });
            });
            item(ui, t("Igualar alturas"), "N", Cmd::Normalize, true);
            item(ui, t("Encuadrar"), "F", Cmd::Fit, true);
            item(ui, t("Tamaño real"), "1", Cmd::ActualPixels, true);
            ui.separator();
            item(ui, t("Recortar"), "C", Cmd::Crop, self.croppable_selected().is_some() || self.crop_mode.is_some());
            item(ui, t("Quitar recorte"), "Alt+C", Cmd::CropReset, self.board.items.iter().any(|i| self.sel.contains(&i.id) && i.crop.is_some()));
            item(ui, t("Espejo"), "M", Cmd::Flip, has_sel);
            item(ui, t("Escala de grises"), "G", Cmd::Gray, has_sel);
            item(ui, t("Girar 90°"), "R", Cmd::Rotate(1), has_sel);
            item(ui, t("Restablecer"), "Ctrl+R", Cmd::ResetTransform, has_sel);
            item(ui, t("Traer al frente"), "]", Cmd::Front, has_sel);
            item(ui, t("Enviar al fondo"), "[", Cmd::Back, has_sel);
            ui.separator();
            ui.menu_button(t("Ajustes"), |ui| {
                if let Some(c) = self.settings_menu(ctx, ui) {
                    cmd.set(Some(c));
                }
            });
        });
        if let Some(c) = cmd.get() {
            self.cmd(ctx, c);
        }
        if let Some(fid) = color_frame.get() {
            let at = ctx.input(|i| i.pointer.hover_pos()).unwrap_or(self.canvas.center());
            self.color_popup = Some((crate::app::ColorTarget::Frame(fid), at, ctx.input(|i| i.time).to_bits()));
        }
    }

    /// Ajustes (en el menú principal y al final del contextual): idioma, opacidad, estado, fondo,
    /// cuadrícula, siempre encima y atajos.
    pub(crate) fn settings_menu(&mut self, ctx: &egui::Context, ui: &mut egui::Ui) -> Option<Cmd> {
        let mut cmd = None;
        ui.set_min_width(250.0);
        ui.menu_button(t("Idioma"), |ui| {
            // los nombres en chino, japonés y coreano necesitan su fuente aunque no sea el idioma activo
            if !self.cjk_loaded {
                self.cjk_loaded = true;
                crate::i18n::install_fonts_with_cjk(ctx, crate::i18n::current());
            }
            let cur = self.cfg.lang.clone();
            if ui.radio(cur == "auto", t("Automático (sistema)")).clicked() {
                self.set_lang(ctx, "auto");
                ui.close();
            }
            ui.separator();
            for (code, name) in crate::i18n::LANGS {
                if ui.radio(cur == code, name).clicked() {
                    self.set_lang(ctx, code);
                    ui.close();
                }
            }
        });
        ui.horizontal(|ui| {
            ui.label(t("Opacidad"));
            let mut o = self.cfg.opacity * 100.0;
            if ui.add(egui::Slider::new(&mut o, 10.0..=100.0).suffix(" %").max_decimals(0)).changed() {
                self.cfg.opacity = o / 100.0;
                self.cfg.save();
            }
        });
        let mut st = self.cfg.show_status;
        if ui.checkbox(&mut st, t("Mostrar zoom y VRAM")).clicked() {
            cmd = Some(Cmd::ToggleStatus);
        }
        let mut sl = self.cfg.show_links;
        if ui.checkbox(&mut sl, t("Mostrar los vínculos")).clicked() {
            self.cfg.show_links = sl;
            self.cfg.save();
        }
        let mut grid = self.grid;
        if ui.checkbox(&mut grid, t("Cuadrícula (Mayús+G)")).clicked() {
            cmd = Some(Cmd::Grid);
        }
        let mut bp = self.blueprint;
        if ui.checkbox(&mut bp, t("Tema blueprint (Ctrl+Mayús+G)")).clicked() {
            cmd = Some(Cmd::Blueprint);
        }
        let mut top = self.on_top;
        if ui.checkbox(&mut top, t("Siempre encima (Ctrl+T)")).clicked() {
            cmd = Some(Cmd::OnTop);
        }
        // el selector de egui se cerraba con el menú: se usa el selector redondo propio, que queda abierto
        let r = ui.horizontal(|ui| {
            ui.label(t("Color de fondo"));
            let (r, resp) = ui.allocate_exact_size(Vec2::splat(22.0), Sense::click());
            let b = self.bg;
            ui.painter().circle_filled(r.center(), 10.0, Color32::from_rgb(b[0], b[1], b[2]));
            ui.painter().circle_stroke(r.center(), 10.0, Stroke::new(if resp.hovered() { 2.0 } else { 1.5 }, Color32::from_white_alpha(if resp.hovered() { 255 } else { 110 })));
            (resp.clicked(), r)
        });
        if r.inner.0 {
            let at = r.inner.1.right_bottom() + vec2(8.0, -10.0);
            self.color_popup = Some((crate::app::ColorTarget::Background, at, ctx.input(|i| i.time).to_bits()));
            ui.close();
        }
        ui.separator();
        if ui.add(egui::Button::new(t("Atajos de teclado")).shortcut_text("F1")).clicked() {
            cmd = Some(Cmd::Help);
        }
        if cmd.is_some() {
            ui.close();
        }
        cmd
    }

    /// Wayland no deja que una aplicación se ponga siempre encima: se explica y se ofrece el modo
    /// compatible (XWayland), donde sí funciona.
    fn ontop_dialog(&mut self, ctx: &egui::Context) {
        if !self.ontop_prompt {
            return;
        }
        let mut choice = 0;
        egui::Modal::new(egui::Id::new("ontop")).show(ctx, |ui| {
            ui.set_max_width(420.0);
            ui.heading(t("Siempre encima en Wayland"));
            ui.label(t("Wayland no permite que una aplicación se ponga encima de las demás por sí misma."));
            ui.add_space(4.0);
            let gnome = std::env::var("XDG_CURRENT_DESKTOP").unwrap_or_default().to_ascii_lowercase().contains("gnome");
            if gnome {
                ui.label(t("En GNOME puedes hacerlo tú: pulsa Alt+Espacio con la ventana activa y elige «Siempre encima»."));
            } else {
                ui.label(t("Tu escritorio puede tener la opción en el menú de la ventana (Alt+Espacio) o en sus reglas de ventana."));
            }
            ui.label(t("O bien Mosaic se reinicia en modo compatible (XWayland), donde sí funciona desde aquí. Se recordará para las próximas veces."));
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if ui.button(t("Reiniciar en modo compatible")).clicked() {
                    choice = 1;
                }
                if ui.button(t("Cancelar")).clicked() {
                    choice = 2;
                }
            });
        });
        match choice {
            1 => {
                self.ontop_prompt = false;
                self.restart_compat_on_top(ctx);
            }
            2 => self.ontop_prompt = false,
            _ => {}
        }
    }

    fn close_guard(&mut self, ctx: &egui::Context) {
        if ctx.input(|i| i.viewport().close_requested()) && self.dirty && !self.allow_close && !self.board.items.is_empty() {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            self.confirm_close = true;
        }
        if !self.confirm_close && self.pending_open.is_none() {
            return;
        }
        let mut choice = 0;
        egui::Modal::new(egui::Id::new("close")).show(ctx, |ui| {
            ui.heading(t("¿Guardar los cambios?"));
            ui.label(t("El tablero tiene cambios sin guardar."));
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if ui.button(t("Guardar")).clicked() {
                    choice = 1;
                }
                if ui.button(t("Descartar")).clicked() {
                    choice = 2;
                }
                if ui.button(t("Cancelar")).clicked() {
                    choice = 3;
                }
            });
        });
        // la misma pregunta sirve para cerrar la ventana y para abrir otro tablero
        let opening = self.pending_open.take();
        match choice {
            1 => {
                self.confirm_close = false;
                match self.path.clone() {
                    Some(p) => {
                        self.save_to(ctx, p);
                        if !self.dirty {
                            match opening {
                                Some(o) => self.open_board(o),
                                None => ctx.send_viewport_cmd(egui::ViewportCommand::Close),
                            }
                        }
                    }
                    None => self.dialog(1),
                }
            }
            2 => {
                self.confirm_close = false;
                match opening {
                    Some(o) => {
                        self.dirty = false;
                        self.open_board(o);
                    }
                    None => {
                        self.allow_close = true;
                        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                    }
                }
            }
            3 => self.confirm_close = false,
            _ => self.pending_open = opening,
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Pintado de cada tipo
// ---------------------------------------------------------------------------------------------

fn draw_quad(painter: &egui::Painter, tid: egui::TextureId, sc: [Pos2; 4], flip: bool, crop: [f32; 4], tint: Color32) {
    let mut m = egui::Mesh::with_texture(tid);
    let [x0, y0, x1, y1] = crop;
    let uv = if flip { [pos2(x1, y0), pos2(x0, y0), pos2(x0, y1), pos2(x1, y1)] } else { [pos2(x0, y0), pos2(x1, y0), pos2(x1, y1), pos2(x0, y1)] };
    for k in 0..4 {
        m.vertices.push(egui::epaint::Vertex { pos: sc[k], uv: uv[k], color: tint });
    }
    m.add_triangle(0, 1, 2);
    m.add_triangle(0, 2, 3);
    painter.add(egui::Shape::mesh(m));
}

/// Tiradores del recorte (fracciones): esquinas y centros de los lados, en el sentido de las agujas.
fn crop_handles(c: [f32; 4]) -> [Vec2; 8] {
    let (mx, my) = ((c[0] + c[2]) * 0.5, (c[1] + c[3]) * 0.5);
    [vec2(c[0], c[1]), vec2(mx, c[1]), vec2(c[2], c[1]), vec2(c[2], my), vec2(c[2], c[3]), vec2(mx, c[3]), vec2(c[0], c[3]), vec2(c[0], my)]
}

/// Maquetación del texto con su formato (familia, cursiva, subrayado, tachado, alineación).
pub(crate) fn text_job(t: &Text, s: &str, fs: f32) -> egui::text::LayoutJob {
    let family = if t.mono { egui::FontFamily::Monospace } else { egui::FontFamily::Proportional };
    let color = c32(t.color);
    let line = Stroke::new((fs * 0.07).max(1.0), color);
    let fmt = egui::TextFormat {
        font_id: FontId::new(fs, family),
        color,
        italics: t.italic,
        underline: if t.underline { line } else { Stroke::NONE },
        strikethrough: if t.strike { line } else { Stroke::NONE },
        ..Default::default()
    };
    // las direcciones web, subrayadas y en azul
    let urls = media::url_ranges(s);
    let mut job = egui::text::LayoutJob::default();
    if urls.is_empty() {
        job.append(s, 0.0, fmt);
    } else {
        let link = egui::TextFormat { color: Color32::from_rgb(110, 175, 255), underline: Stroke::new((fs * 0.06).max(1.0), Color32::from_rgb(110, 175, 255)), ..fmt.clone() };
        let chars: Vec<char> = s.chars().collect();
        let mut at = 0;
        for (a, b, _) in &urls {
            if *a > at {
                job.append(&chars[at..*a].iter().collect::<String>(), 0.0, fmt.clone());
            }
            job.append(&chars[*a..*b].iter().collect::<String>(), 0.0, link.clone());
            at = *b;
        }
        if at < chars.len() {
            job.append(&chars[at..].iter().collect::<String>(), 0.0, fmt);
        }
    }
    job.halign = match t.align {
        1 => egui::Align::Center,
        2 => egui::Align::RIGHT,
        _ => egui::Align::LEFT,
    };
    job.wrap.max_width = f32::INFINITY;
    job
}

/// Texto a cualquier zoom: se maqueta a un tamaño de letra acotado (el atlas de glifos no crece
/// sin límite) y la malla resultante se escala y gira hasta su tamaño en pantalla. La negrita se
/// simula pintando el texto dos veces con un desplazamiento mínimo (las fuentes de egui no traen
/// variante negrita). Devuelve la caja medida en el mundo (a escala 1).
fn paint_text(painter: &egui::Painter, it: &Item, t: &Text, xf: Xf, ppp: f32, font_tex: [usize; 2], editing: bool) -> Vec2 {
    let screen_fs = t.size * it.scale * xf.zoom;
    let fs = screen_fs.clamp(6.0, 128.0);
    let fs = if fs < 24.0 { fs.round() } else { (fs / 4.0).round() * 4.0 };
    let s = if t.s.is_empty() { " " } else { t.s.as_str() };
    let galley = painter.layout_job(text_job(t, s, fs));
    let gr = galley.rect;
    let measured = gr.size() / fs * t.size;
    if editing || screen_fs < 1.0 {
        return measured;
    }
    let k = screen_fs / fs;
    let rot = egui::emath::Rot2::from_angle(it.rot);
    let c = xf.s(it.center());
    let place = |v: Vec2| -> Pos2 {
        let mut l = (v - gr.center().to_vec2()) * k;
        if it.flip {
            l.x = -l.x;
        }
        c + rot * l
    };
    if let Some(bg) = t.bg {
        let pad = vec2(0.3, 0.12) * fs;
        let (a, b) = (gr.min.to_vec2() - pad, gr.max.to_vec2() + pad);
        let r = [vec2(a.x, a.y), vec2(b.x, a.y), vec2(b.x, b.y), vec2(a.x, b.y)].map(place);
        painter.add(egui::Shape::convex_polygon(r.to_vec(), c32(bg), Stroke::NONE));
    }
    let shape = egui::epaint::TextShape::new(Pos2::ZERO, galley, c32(t.color));
    let mut mesh = egui::Mesh::default();
    let mut tess = egui::epaint::Tessellator::new(ppp, egui::epaint::TessellationOptions::default(), font_tex, vec![]);
    tess.tessellate_text(&shape, &mut mesh);
    if t.bold {
        let mut extra = mesh.clone();
        let dx = fs * 0.045;
        for v in extra.vertices.iter_mut() {
            v.pos.x += dx;
        }
        mesh.append(extra);
    }
    for v in mesh.vertices.iter_mut() {
        v.pos = place(v.pos.to_vec2());
    }
    painter.add(egui::Shape::mesh(mesh));
    measured
}

/// Cuadrícula «blueprint»: líneas finas cada paso redondo (1, 2, 5 × 10ⁿ) que quede a ≥ 18 px
/// en pantalla, y una de cada cinco más marcada.
fn paint_grid(painter: &egui::Painter, clip: Rect, xf: Xf, blueprint: bool) {
    let world_min = xf.cam + (clip.min - xf.center) / xf.zoom;
    let world_max = xf.cam + (clip.max - xf.center) / xf.zoom;
    let target = 18.0 / xf.zoom;
    let mut step = 10f32.powf(target.log10().floor());
    for m in [1.0, 2.0, 5.0, 10.0] {
        if step * m >= target {
            step *= m;
            break;
        }
    }
    // sobre el fondo normal, líneas grises discretas; en el tema de plano, azul claro
    let (minor, major) = if blueprint {
        (Color32::from_rgba_unmultiplied(150, 200, 255, 22), Color32::from_rgba_unmultiplied(170, 215, 255, 48))
    } else {
        (Color32::from_white_alpha(10), Color32::from_white_alpha(24))
    };
    let (minor, major) = (Stroke::new(1.0, minor), Stroke::new(1.0, major));
    let i0 = (world_min.x / step).floor() as i64;
    let i1 = (world_max.x / step).ceil() as i64;
    for i in i0..=i1 {
        let x = xf.s(pos2(i as f32 * step, 0.0)).x;
        painter.line_segment([pos2(x, clip.top()), pos2(x, clip.bottom())], if i % 5 == 0 { major } else { minor });
    }
    let j0 = (world_min.y / step).floor() as i64;
    let j1 = (world_max.y / step).ceil() as i64;
    for j in j0..=j1 {
        let y = xf.s(pos2(0.0, j as f32 * step)).y;
        painter.line_segment([pos2(clip.left(), y), pos2(clip.right(), y)], if j % 5 == 0 { major } else { minor });
    }
}

/// Marco: fondo translúcido, borde y título encima. Devuelve el rectángulo del título en pantalla.
fn paint_frame(painter: &egui::Painter, f: &crate::board::FrameBox, sc: &[Pos2; 4], selected: bool, editing: bool) -> Option<Rect> {
    let r = Rect::from_points(sc);
    painter.rect_filled(r, CornerRadius::same(8), c32(f.color));
    let border = if selected { ACCENT } else { Color32::from_white_alpha(40) };
    painter.rect_stroke(r, CornerRadius::same(8), Stroke::new(1.0, border), StrokeKind::Inside);
    if editing || r.width() < 40.0 {
        return None;
    }
    let title = if f.title.is_empty() { t("Marco") } else { f.title.as_str() };
    let g = painter.layout_no_wrap(title.to_string(), FontId::proportional(13.0), Color32::from_gray(200));
    let w = g.size().x.min(r.width());
    let tr = Rect::from_min_size(r.left_top() - vec2(0.0, g.size().y + 6.0), vec2(w + 4.0, g.size().y + 4.0));
    let clip = painter.with_clip_rect(tr.intersect(painter.clip_rect()));
    clip.galley(tr.min + vec2(2.0, 2.0), g, Color32::from_gray(200));
    Some(tr)
}

fn paint_ink(painter: &egui::Painter, it: &Item, k: &Ink, xf: Xf) {
    let rot = egui::emath::Rot2::from_angle(it.rot);
    let c = xf.s(it.center());
    let s = it.scale * xf.zoom;
    let w = (k.width * s).max(0.6);
    let color = c32(k.color);
    let mut pts: Vec<Pos2> = Vec::with_capacity(k.pts.len());
    for p in &k.pts {
        let mut l = vec2(p[0], p[1]) * s;
        if it.flip {
            l.x = -l.x;
        }
        let q = c + rot * l;
        // menos puntos al alejarse: ninguno a menos de 0.75 px del anterior
        if pts.last().is_none_or(|l: &Pos2| l.distance(q) >= 0.75) {
            pts.push(q);
        }
    }
    if pts.len() == 1 {
        painter.circle_filled(pts[0], w * 0.5, color);
        return;
    }
    // uniones y extremos redondeados en los trazos gruesos
    if w > 3.0 && pts.len() < 4000 {
        for p in &pts {
            painter.circle_filled(*p, w * 0.5, color);
        }
    }
    painter.add(egui::Shape::line(pts, Stroke::new(w, color)));
}

/// Cadena (dos eslabones); rota, con los eslabones separados y dos chispas.
pub(crate) fn glyph_link(p: &egui::Painter, c: Pos2, col: Color32, broken: bool) {
    let rot = egui::emath::Rot2::from_angle(-std::f32::consts::FRAC_PI_4);
    let gap = if broken { 2.5 } else { 0.0 };
    for (dx, sgn) in [(-3.6 - gap, -1.0), (3.6 + gap, 1.0)] {
        let center = c + rot * vec2(dx, 0.0);
        let half = rot * vec2(3.2, 0.0);
        let n = rot * vec2(0.0, 2.6);
        let _ = sgn;
        // eslabón: dos rectas y dos semicírculos
        let pts: Vec<Pos2> = (0..=16)
            .map(|k| {
                let a = std::f32::consts::PI * (k as f32 / 16.0) - std::f32::consts::FRAC_PI_2;
                center + half + rot * vec2(a.cos() * 2.6, a.sin() * 2.6)
            })
            .chain((0..=16).map(|k| {
                let a = std::f32::consts::PI * (k as f32 / 16.0) + std::f32::consts::FRAC_PI_2;
                center - half + rot * vec2(a.cos() * 2.6, a.sin() * 2.6)
            }))
            .collect();
        let _ = n;
        p.add(egui::Shape::closed_line(pts, Stroke::new(1.6, col)));
    }
    if broken {
        for v in [vec2(-1.5, -5.5), vec2(1.5, 5.5)] {
            p.line_segment([c + v * 0.8, c + v * 1.25], Stroke::new(1.4, col));
        }
    }
}

fn play_icon(painter: &egui::Painter, c: Pos2, r: f32, color: Color32) {
    let pts = vec![c + vec2(-r * 0.6, -r), c + vec2(r, 0.0), c + vec2(-r * 0.6, r)];
    painter.add(egui::Shape::convex_polygon(pts, color, Stroke::NONE));
}

fn speaker_icon(painter: &egui::Painter, c: Pos2, muted: bool) {
    let col = Color32::from_gray(230);
    let body = vec![c + vec2(-7.0, -3.0), c + vec2(-3.0, -3.0), c + vec2(2.0, -7.0), c + vec2(2.0, 7.0), c + vec2(-3.0, 3.0), c + vec2(-7.0, 3.0)];
    painter.add(egui::Shape::convex_polygon(body, col, Stroke::NONE));
    if muted {
        painter.line_segment([c + vec2(4.0, -4.0), c + vec2(10.0, 4.0)], Stroke::new(1.5, col));
        painter.line_segment([c + vec2(10.0, -4.0), c + vec2(4.0, 4.0)], Stroke::new(1.5, col));
    } else {
        painter.line_segment([c + vec2(5.0, -3.0), c + vec2(5.0, 3.0)], Stroke::new(1.5, col));
        painter.line_segment([c + vec2(8.0, -6.0), c + vec2(8.0, 6.0)], Stroke::new(1.5, col));
    }
}

const HELP: &[(&str, &str)] = &[
    ("Rueda / pellizco", "Zoom alrededor del cursor"),
    ("Botón central, Espacio o Alt + arrastrar", "Desplazar la vista"),
    ("V · T · D · K", "Mover · texto · dibujar a mano alzada · marco"),
    ("Ctrl+G", "Rodear la selección con un marco (lo de dentro se mueve con él)"),
    ("Ctrl+L", "Emparentar la selección al primer elemento seleccionado, o romper el emparentado"),
    ("A con un marco seleccionado", "Ordenar su contenido y ajustar el marco"),
    ("Clic / Mayús+clic / arrastrar en vacío", "Seleccionar"),
    ("Esquinas · círculo superior", "Escalar · girar (Mayús: pasos de 15°)"),
    ("Arrastrar fuera de la ventana", "Llevar a otra aplicación (Krita, Blender…)"),
    ("Doble clic", "Encuadrar · vídeo: reproducir · texto: editar · 3D: girar"),
    ("P · Mayús+M", "Reproducir/pausa (vídeo y audio) · silenciar vídeo"),
    ("Ctrl+Mayús+↑ / ↓", "Más o menos opacidad de la ventana (transparencia)"),
    (", · . · E", "Página anterior · siguiente · expandir páginas (documentos)"),
    ("O", "Perspectiva u ortográfica (modelos 3D)"),
    ("C · Alt+C", "Recortar (Intro acepta, Esc cancela) · quitar el recorte"),
    ("F · Inicio · 1", "Encuadrar selección · todo · tamaño real"),
    ("A · N", "Ordenar · igualar alturas"),
    ("Alt+← → ↑ ↓ · Alt+H · Alt+V", "Alinear la selección a un lado · centrar (menú: repartir)"),
    ("M (o H) · G · R / Mayús+R", "Espejo · grises · girar 90°"),
    ("[ · ]", "Enviar al fondo · traer al frente"),
    ("Ctrl+C · Ctrl+V", "Copiar · pegar imagen, archivos o URL"),
    ("Ctrl+clic en un enlace", "Abrirlo en el navegador (direcciones web en textos y enlaces de PDF)"),
    ("Ctrl+D · Supr", "Duplicar · eliminar"),
    ("Ctrl+Z · Ctrl+Y", "Deshacer · rehacer"),
    ("Ctrl+O · Ctrl+S · Ctrl+I", "Abrir · guardar · importar"),
    ("Ctrl+T", "Ventana siempre encima (en Wayland: modo compatible)"),
    ("Mayús+G · Ctrl+Mayús+G", "Cuadrícula de fondo · tema blueprint"),
    ("Arrastrar la franja superior", "Mover la ventana (doble clic: maximizar)"),
    ("Doble clic en la cabecera", "Maximizar o restaurar la ventana"),
    ("Flechas (Mayús: ×10)", "Mover la selección"),
];

// ---------------------------------------------------------------------------------------------
// Arnés de capturas (desarrollo)
// ---------------------------------------------------------------------------------------------

#[cfg(feature = "selftest")]
impl App {
    /// `REFS_SHOT=captura.png [REFS_WAIT=ms] [REFS_ZOOM=f] [REFS_SEL=1] [REFS_SAVE=t.refs]
    /// [REFS_DEMO=1] [REFS_PLAY=1] [REFS_ACTIVE3D=1]`: espera a que no quede nada por cargar,
    /// encuadra, guarda una captura y sale. Imprime tiempos en stderr.
    fn selftest(&mut self, ctx: &egui::Context) {
        let Some(out) = std::env::var_os("REFS_SHOT") else { return };
        let t = ctx.input(|i| i.time);
        let wait = std::env::var("REFS_WAIT").ok().and_then(|v| v.parse::<f64>().ok()).unwrap_or(1500.0) / 1000.0;
        let st = ctx.data_mut(|d| *d.get_temp_mut_or(egui::Id::new("selftest"), 0u8));
        let set = |v: u8| ctx.data_mut(|d| d.insert_temp(egui::Id::new("selftest"), v));
        let env = |k: &str| std::env::var_os(k).is_some();
        let idle = !self.tex.busy() && self.batches.is_empty() && !self.meshes.values().any(|m| matches!(m, MeshState::Loading));
        match st {
            // tablero vacío con REFS_EMPTY: captura del mensaje de bienvenida
            0 if t > 1.0 && env("REFS_EMPTY") => {
                if env("REFS_COLOR") {
                    self.color_popup = Some((crate::app::ColorTarget::Background, self.canvas.center() - vec2(120.0, 160.0), 0));
                }
                set(2);
                ctx.request_repaint();
            }
            0 if t > 0.3 && idle && !self.board.items.is_empty() => {
                eprintln!("[trace] {:.0} ms: {} elementos listos, VRAM {} MB", t * 1000.0, self.board.items.len(), self.tex.vram() >> 20);
                if env("REFS_DEMO") {
                    // un texto y un trazo junto a lo cargado
                    let r = self.bounds(false).unwrap_or(Rect::from_center_size(Pos2::ZERO, Vec2::splat(100.0)));
                    let size = r.height() * 0.08;
                    let id = self.board.new_id();
                    let mut it = Item::new(id, 0, pos2(r.center().x, r.top() - size), 1.0);
                    let mut tx = Text::new("Referencias · notas".into(), size, [255, 220, 120, 255]);
                    tx.bold = true;
                    it.text = Some(tx);
                    self.board.items.push(it);
                    let pts: Vec<Pos2> = (0..60).map(|k| {
                        let a = k as f32 / 59.0 * std::f32::consts::TAU;
                        pos2(r.center().x + a.cos() * r.width() * 0.2, r.center().y + a.sin() * r.height() * 0.2)
                    }).collect();
                    let (ink, c) = Ink::from_world(&pts, r.height() * 0.01, [255, 90, 70, 255]);
                    let id = self.board.new_id();
                    let mut k = Item::new(id, 0, c, 1.0);
                    k.ink = Some(ink);
                    self.board.items.push(k);
                }
                if env("REFS_FRAME") {
                    // un marco alrededor de todo lo cargado, y el texto seleccionado (barra de formato)
                    self.sel = self.board.items.iter().filter(|i| i.text.is_none()).map(|i| i.id).collect();
                    self.cmd(ctx, Cmd::FrameSelection);
                    if let Some(id) = self.board.items.iter().find(|i| i.text.is_some()).map(|i| i.id) {
                        if let Some(t) = self.item_mut(id).and_then(|i| i.text.as_mut()) {
                            t.underline = true;
                            t.align = 1;
                            t.bg = Some([30, 30, 60, 200]);
                        }
                        self.sel = HashSet::from([id]);
                    }
                }
                if env("REFS_CROP")
                    && let Some(id) = self.board.items.iter().find(|i| self.kind_of(i) == Some(Kind::Image)).map(|i| i.id)
                {
                    self.sel = HashSet::from([id]);
                    self.cmd(ctx, Cmd::Crop);
                    if let Some(it) = self.item(id) {
                        let w = self.board.frac_to_world(it, vec2(0.6, 0.65));
                        self.drag_crop(id, 4, [0.0, 0.0, 1.0, 1.0], vec2(1.0, 1.0), w);
                        let w = self.board.frac_to_world(self.item(id).unwrap(), vec2(0.2, 0.15));
                        let c = crate::board::crop_of(self.item(id).unwrap());
                        self.drag_crop(id, 0, c, vec2(0.0, 0.0), w);
                    }
                }
                                if env("REFS_LINK") {
                    self.cmd(ctx, Cmd::Link);
                }
                if env("REFS_GRID") {
                    self.grid = true;
                }
                if env("REFS_BLUEPRINT") {
                    self.cmd(ctx, Cmd::Blueprint);
                }
                                if env("REFS_SEL")
                    && let Some(it) = self.board.items.first()
                {
                    self.sel = HashSet::from([it.id]);
                }
                if let Some(r) = self.bounds(false) {
                    self.fit_rect(r, false);
                }
                if let Some(z) = std::env::var("REFS_ZOOM").ok().and_then(|v| v.parse::<f32>().ok()) {
                    self.zoom *= z;
                }
                // REFS_PAN=dx,dy desplaza la vista (px de pantalla)
                if let Some((dx, dy)) = std::env::var("REFS_PAN").ok().and_then(|v| v.split_once(',').and_then(|(a, b)| Some((a.parse::<f32>().ok()?, b.parse::<f32>().ok()?)))) {
                    self.cam -= vec2(dx, dy) / self.zoom;
                }
                if env("REFS_ORTHO") {
                    for it in self.board.items.iter_mut() {
                        it.ortho3d = true;
                    }
                }
                if env("REFS_PLAY") {
                    let vids: Vec<u64> = self.board.items.iter().filter(|i| matches!(self.kind_of(i), Some(Kind::Video) | Some(Kind::Audio))).map(|i| i.id).collect();
                    for id in vids {
                        self.toggle_play(ctx, id);
                    }
                }
                if env("REFS_ACTIVE3D") {
                    self.active3d = self.board.items.iter().find(|i| self.kind_of(i) == Some(Kind::Model)).map(|i| i.id);
                }
                set(1);
                ctx.request_repaint();
            }
            1 if idle => {
                eprintln!("[trace] {:.0} ms: tras encuadrar, VRAM {} MB", t * 1000.0, self.tex.vram() >> 20);
                if let Some(p) = std::env::var_os("REFS_SAVE") {
                    let t0 = std::time::Instant::now();
                    self.save_to(ctx, p.into());
                    eprintln!("[trace] guardado en {:.0} ms", t0.elapsed().as_secs_f64() * 1000.0);
                }
                set(2);
                ctx.request_repaint_after(std::time::Duration::from_secs_f64(wait));
            }
            2 if t > wait + 0.3 => {
                ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(Default::default()));
                set(3);
            }
            3 => {
                let img = ctx.input(|i| i.events.iter().find_map(|e| if let egui::Event::Screenshot { image, .. } = e { Some(image.clone()) } else { None }));
                if let Some(img) = img {
                    if let Ok(st) = std::fs::read_to_string("/proc/self/status") {
                        let get = |k: &str| st.lines().find(|l| l.starts_with(k)).map(|l| l.split_whitespace().nth(1).unwrap_or("0").parse::<u64>().unwrap_or(0) / 1024).unwrap_or(0);
                        eprintln!("[trace] memoria: RSS {} MB (anónima {} MB, archivos {} MB), pico {} MB, VRAM {} MB", get("VmRSS:"), get("RssAnon:"), get("RssFile:"), get("VmHWM:"), self.tex.vram() >> 20);
                    }
                    let [w, h] = img.size;
                    let raw: Vec<u8> = img.pixels.iter().flat_map(|c| c.to_array()).collect();
                    let _ = image::save_buffer(std::path::Path::new(&out), &raw, w as u32, h as u32, image::ExtendedColorType::Rgba8);
                    self.allow_close = true;
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                }
            }
            _ => {}
        }
        if st < 3 {
            ctx.request_repaint_after(std::time::Duration::from_millis(50));
        }
    }
}
