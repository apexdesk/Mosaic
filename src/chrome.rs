//! Ventana sin decoración del sistema (como File Browser): cabecera propia con el menú y el título a
//! la izquierda, las herramientas con iconos en el centro y minimizar/maximizar/cerrar a la derecha;
//! bordes para redimensionar; selector de color redondo y barra de formato del texto.
use crate::i18n::t;
use crate::app::{App, Cmd, ColorTarget, Tool};
use egui::{Color32, CornerRadius, FontId, Pos2, Rect, Sense, Stroke, StrokeKind, Vec2, pos2, vec2};

const ACCENT: Color32 = Color32::from_rgb(0x4d, 0x9c, 0xff);
const PILL_H: f32 = 36.0;
const TEXT: Color32 = Color32::from_gray(225);
const DIM: Color32 = Color32::from_gray(150);

/// Colores rápidos del selector.
const PALETTE: [[u8; 4]; 14] = [
    [255, 255, 255, 255],
    [190, 190, 196, 255],
    [110, 110, 118, 255],
    [24, 24, 28, 255],
    [255, 90, 70, 255],
    [255, 159, 28, 255],
    [255, 214, 90, 255],
    [120, 210, 110, 255],
    [40, 180, 160, 255],
    [77, 156, 255, 255],
    [130, 110, 255, 255],
    [230, 110, 200, 255],
    [150, 100, 60, 255],
    [245, 222, 190, 255],
];

fn c32(c: [u8; 4]) -> Color32 {
    Color32::from_rgba_unmultiplied(c[0], c[1], c[2], c[3])
}

/// Acciones de la barra de formato (se aplican después, fuera del cierre de egui).
enum Fmt {
    Mono(bool),
    Size(f32),
    Bold,
    Italic,
    Underline,
    Strike,
    Align(u8),
    Color(ColorTarget, Pos2),
}

impl App {
    // -----------------------------------------------------------------------------------------
    // Bordes de la ventana
    // -----------------------------------------------------------------------------------------

    /// Sin decoración del sistema, la ventana se redimensiona desde sus bordes (6 pt).
    pub(crate) fn window_edges(&mut self, ctx: &egui::Context) {
        use egui::{CursorIcon as C, ResizeDirection as D};
        if ctx.input(|i| i.viewport().maximized.unwrap_or(false) || i.viewport().fullscreen.unwrap_or(false)) {
            return;
        }
        let r = ctx.content_rect();
        let Some(q) = ctx.input(|i| i.pointer.hover_pos()) else { return };
        let m = 6.0;
        let (l, rt, t, b) = (q.x < r.left() + m, q.x > r.right() - m, q.y < r.top() + 4.0, q.y > r.bottom() - m);
        let hit = match (l, rt, t, b) {
            (true, _, true, _) => Some((D::NorthWest, C::ResizeNwSe)),
            (_, true, true, _) => Some((D::NorthEast, C::ResizeNeSw)),
            (true, _, _, true) => Some((D::SouthWest, C::ResizeNeSw)),
            (_, true, _, true) => Some((D::SouthEast, C::ResizeNwSe)),
            (true, ..) => Some((D::West, C::ResizeHorizontal)),
            (_, true, ..) => Some((D::East, C::ResizeHorizontal)),
            (_, _, true, _) => Some((D::North, C::ResizeVertical)),
            (_, _, _, true) => Some((D::South, C::ResizeVertical)),
            _ => None,
        };
        if let Some((dir, cur)) = hit {
            ctx.set_cursor_icon(cur);
            if ctx.input(|i| i.pointer.primary_pressed()) {
                ctx.send_viewport_cmd(egui::ViewportCommand::BeginResize(dir));
                self.win_op = true;
            }
        }
    }

    // -----------------------------------------------------------------------------------------
    // Cabecera
    // -----------------------------------------------------------------------------------------

    /// Cabecera sin fondo: tres pastillas flotando sobre el lienzo (como pequeñas barras de
    /// pestañas). Izquierda: menú, icono y nombre (arrastrar mueve la ventana, doble clic la
    /// maximiza). Centro: herramientas. Derecha: minimizar, maximizar y cerrar.
    pub(crate) fn header(&mut self, ctx: &egui::Context) {
        let mut cmd: Option<Cmd> = None;
        let full = ctx.content_rect();
        let maximized = ctx.input(|i| i.viewport().maximized.unwrap_or(false));
        let y = full.top() + 8.0;
        let h = PILL_H;

        // --- izquierda
        let name = self.path.as_ref().and_then(|p| p.file_stem()).map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| t("Sin título").into());
        let name_w = ctx.fonts_mut(|f| f.layout_no_wrap(name.clone(), FontId::proportional(13.0), DIM).size().x).min(260.0);
        let left_w = 6.0 + 32.0 + 8.0 + name_w + if self.dirty { 16.0 } else { 6.0 } + 8.0;
        // menú, título y botones de ventana se ocultan solos; vuelven al acercar el ratón arriba
        let now = ctx.input(|i| i.time);
        let near = ctx.input(|i| i.pointer.hover_pos()).is_some_and(|p| p.y < full.top() + 64.0);
        if near || egui::Popup::is_any_open(ctx) || self.confirm_close || self.ontop_prompt {
            self.chrome_until = now + 2.0;
        }
        let shown = now < self.chrome_until;
        if shown {
            ctx.request_repaint_after(std::time::Duration::from_secs_f64((self.chrome_until - now).max(0.05)));
        }
        let alpha = ctx.animate_bool_with_time(egui::Id::new("chrome-shown"), shown, 0.25);
        pill(ctx, "pill-left", pos2(full.left() + 8.0, y), vec2(left_w, h), alpha, |ui, r| {
            // fondo de la pastilla: mover la ventana
            let bg = ui.interact(r, egui::Id::new("pill-left-bg"), Sense::click_and_drag());
            if bg.drag_started() {
                ctx.send_viewport_cmd(egui::ViewportCommand::StartDrag);
            }
            if bg.double_clicked() {
                ctx.send_viewport_cmd(egui::ViewportCommand::Maximized(!maximized));
            }
            let p = ui.painter();
            let menu_r = Rect::from_min_size(pos2(r.left() + 4.0, r.top() + 3.0), vec2(32.0, h - 6.0));
            let menu = icon_button(ui, p, menu_r, "menu", false, t("Menú"), glyph_menu);
            egui::Popup::menu(&menu).show(|ui| {
                ui.set_min_width(230.0);
                let mut it = |ui: &mut egui::Ui, label: &str, short: &str, c: Cmd| {
                    if ui.add(egui::Button::new(label).shortcut_text(short)).clicked() {
                        cmd = Some(c);
                        ui.close();
                    }
                };
                it(ui, t("Abrir tablero…"), "Ctrl+O", Cmd::Open);
                let recent: Vec<std::path::PathBuf> = self.cfg.recent.iter().filter(|p| p.exists()).cloned().collect();
                let mut open_recent = None;
                let mut clear_recent = false;
                ui.add_enabled_ui(!recent.is_empty(), |ui| {
                    ui.menu_button(t("Recientes"), |ui| {
                        ui.set_min_width(260.0);
                        for p in &recent {
                            let name = p.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
                            let dir = p.parent().map(|d| d.display().to_string()).unwrap_or_default();
                            if ui.button(name).on_hover_text(&dir).clicked() {
                                open_recent = Some(p.clone());
                                ui.close();
                            }
                        }
                        ui.separator();
                        if ui.button(t("Limpiar la lista")).clicked() {
                            clear_recent = true;
                            ui.close();
                        }
                    });
                });
                if let Some(p) = open_recent {
                    self.request_open(p);
                }
                if clear_recent {
                    self.cfg.recent.clear();
                    self.cfg.save();
                }
                it(ui, t("Guardar"), "Ctrl+S", Cmd::Save);
                it(ui, t("Guardar como…"), t("Ctrl+Mayús+S"), Cmd::SaveAs);
                it(ui, t("Importar…"), "Ctrl+I", Cmd::Import);
                ui.separator();
                it(ui, t("Deshacer"), "Ctrl+Z", Cmd::Undo);
                it(ui, t("Rehacer"), "Ctrl+Y", Cmd::Redo);
                it(ui, t("Encuadrar todo"), t("Inicio"), Cmd::FitAll);
                ui.separator();
                let mut from_settings = None;
                ui.menu_button(t("Ajustes"), |ui| {
                    from_settings = self.settings_menu(ctx, ui);
                });
                it(ui, t("Salir"), "", Cmd::Quit);
                if from_settings.is_some() {
                    cmd = from_settings;
                }
            });
            // (sin el icono del programa: ya está en el dock y en la ventana)
            let tr = p.text(pos2(menu_r.right() + 8.0, r.center().y), egui::Align2::LEFT_CENTER, name, FontId::proportional(13.0), TEXT);
            if self.dirty {
                p.circle_filled(pos2(tr.right() + 9.0, r.center().y), 3.5, Color32::from_rgb(255, 159, 28));
            }
        });

        // --- centro: herramientas
        let tools: [(Tool, &str, &str); 4] = [(Tool::Select, "Mover (V)", "sel"), (Tool::Text, "Texto (T)", "txt"), (Tool::Draw, "Dibujar (D)", "draw"), (Tool::Frame, "Marco (K)", "frame")];
        let bw = 34.0;
        let slider_w = if matches!(self.tool, Tool::Draw | Tool::Text) { 150.0 } else { 0.0 };
        let center_w = 6.0 + bw * 4.0 + 4.0 * 3.0 + 13.0 + bw + 4.0 + bw + 4.0 + bw + 13.0 + 26.0 + if slider_w > 0.0 { 12.0 + slider_w } else { 0.0 } + 8.0;
        let cx = (full.center().x - center_w * 0.5).max(full.left() + left_w + 16.0);
        let (tool, grid, ink, mut ink_w, mut text_size) = (self.tool, self.grid, self.ink_color, self.ink_width, self.text_size);
        let linked = self.selection_linked();
        let can_link = self.sel.len() >= 2;
        let cropping = self.crop_mode.is_some();
        let can_crop = cropping || self.croppable_selected().is_some();
        let has_crop = self.board.items.iter().any(|i| self.sel.contains(&i.id) && i.crop.is_some());
        let mut open_color: Option<Rect> = None;
        pill(ctx, "pill-center", pos2(cx, y), vec2(center_w, h), 1.0, |ui, r| {
            let _ = ui.interact(r, egui::Id::new("pill-center-bg"), Sense::click());
            let p = ui.painter();
            let mut x = r.left() + 6.0;
            let by = r.top() + 3.0;
            for (tl, hint, id) in tools {
                let br = Rect::from_min_size(pos2(x, by), vec2(bw, h - 6.0));
                let resp = icon_button(ui, p, br, id, tool == tl, t(hint), move |p, c, col| match tl {
                    Tool::Select => glyph_select(p, c, col),
                    Tool::Text => glyph_text(p, c, col),
                    Tool::Draw => glyph_pen(p, c, col),
                    Tool::Frame => glyph_frame(p, c, col),
                });
                if resp.clicked() {
                    cmd = Some(Cmd::Tool(tl));
                }
                x += bw + 4.0;
            }
            x += 2.0;
            p.line_segment([pos2(x, r.top() + 9.0), pos2(x, r.bottom() - 9.0)], Stroke::new(1.0, Color32::from_white_alpha(30)));
            x += 7.0;
            let gr = Rect::from_min_size(pos2(x, by), vec2(bw, h - 6.0));
            let gresp = icon_button(ui, p, gr, "grid", grid, t("Cuadrícula de fondo (Mayús+G) · clic derecho: tema blueprint"), glyph_grid);
            if gresp.clicked() {
                cmd = Some(Cmd::Grid);
            }
            if gresp.secondary_clicked() {
                cmd = Some(Cmd::Blueprint);
            }
            x += bw + 4.0;
            // emparentar / romper: gris cuando no se puede usar
            let lr = Rect::from_min_size(pos2(x, by), vec2(bw, h - 6.0));
            let hint = if linked { t("Romper emparentado (Ctrl+L)") } else if can_link { t("Emparentar: lo seleccionado se moverá con el primer elemento seleccionado (Ctrl+L)") } else { t("Emparentar (selecciona dos o más elementos)") };
            let lresp = ui.interact(lr, egui::Id::new(("icon-btn", "link")), if linked || can_link { Sense::click() } else { Sense::hover() }).on_hover_text(hint);
            if (linked || can_link) && lresp.hovered() {
                p.rect_filled(lr, CornerRadius::same(8), Color32::from_white_alpha(16));
            }
            let lcol = if linked { ACCENT } else if can_link { TEXT } else { Color32::from_gray(85) };
            crate::canvas::glyph_link(p, lr.center(), lcol, linked);
            if lresp.clicked() {
                cmd = Some(Cmd::Link);
            }
            x += bw + 4.0;
            // recortar: clic entra o acepta; clic derecho quita el recorte; gris si no hay qué recortar
            let cr = Rect::from_min_size(pos2(x, by), vec2(bw, h - 6.0));
            let hint = if cropping {
                t("Aceptar el recorte (C o Intro) · Esc cancela")
            } else if can_crop {
                t("Recortar (C) · clic derecho: quitar el recorte (Alt+C)")
            } else {
                t("Recortar (selecciona una imagen, un vídeo o una página)")
            };
            let cresp = ui.interact(cr, egui::Id::new(("icon-btn", "crop")), if can_crop || has_crop { Sense::click() } else { Sense::hover() }).on_hover_text(hint);
            if cropping {
                p.rect_filled(cr, CornerRadius::same(8), ACCENT.gamma_multiply(0.4));
            } else if (can_crop || has_crop) && cresp.hovered() {
                p.rect_filled(cr, CornerRadius::same(8), Color32::from_white_alpha(16));
            }
            let ccol = if cropping { Color32::WHITE } else if can_crop { TEXT } else { Color32::from_gray(85) };
            glyph_crop(p, cr.center(), ccol);
            if cresp.clicked() && can_crop {
                cmd = Some(Cmd::Crop);
            }
            if cresp.secondary_clicked() && has_crop {
                cmd = Some(Cmd::CropReset);
            }
            x += bw + 6.0;
            p.line_segment([pos2(x, r.top() + 9.0), pos2(x, r.bottom() - 9.0)], Stroke::new(1.0, Color32::from_white_alpha(30)));
            x += 7.0;
            // color redondo
            let sw = Rect::from_min_size(pos2(x, r.center().y - 12.0), Vec2::splat(24.0));
            if swatch(ui, p, sw, "ink-color", Some(c32(ink))).on_hover_text(t("Color del texto, del trazo y de lo seleccionado")).clicked() {
                open_color = Some(sw);
            }
            x += 26.0;
            if slider_w > 0.0 {
                x += 12.0;
                let sr = Rect::from_min_size(pos2(x, r.center().y - 10.0), vec2(slider_w, 20.0));
                ui.spacing_mut().slider_width = slider_w - 44.0;
                if tool == Tool::Draw {
                    ui.put(sr, egui::Slider::new(&mut ink_w, 1.0..=60.0).logarithmic(true).max_decimals(0)).on_hover_text(t("Grosor del trazo"));
                } else {
                    ui.put(sr, egui::Slider::new(&mut text_size, 8.0..=200.0).logarithmic(true).max_decimals(0)).on_hover_text(t("Tamaño del texto nuevo"));
                }
            }
        });
        self.ink_width = ink_w;
        self.text_size = text_size;
        if let Some(sw) = open_color {
            self.open_color(ctx, ColorTarget::Ink, sw.left_bottom() + vec2(-80.0, 10.0));
        }

        // --- derecha: ventana
        let wb = 40.0;
        let right_w = wb * 3.0 + 8.0;
        pill(ctx, "pill-right", pos2(full.right() - 8.0 - right_w, y), vec2(right_w, h), alpha, |ui, r| {
            let p = ui.painter();
            let mut bx = r.left() + 4.0;
            for k in 0..3 {
                let br = Rect::from_min_size(pos2(bx, r.top() + 3.0), vec2(wb, h - 6.0));
                let id = ["win-min", "win-max", "win-close"][k];
                let hint = match k {
                    0 => t("Minimizar"),
                    1 if maximized => t("Restaurar"),
                    1 => t("Maximizar"),
                    _ => t("Cerrar"),
                };
                let resp = ui.interact(br, egui::Id::new(id), Sense::click()).on_hover_text(hint);
                if resp.hovered() {
                    let fill = if k == 2 { Color32::from_rgb(0xc4, 0x2b, 0x1c) } else { Color32::from_white_alpha(18) };
                    p.rect_filled(br, CornerRadius::same(8), fill);
                }
                let col = if k == 2 && resp.hovered() { Color32::WHITE } else { TEXT };
                let c = br.center();
                match k {
                    0 => glyph_min(p, c, col),
                    1 if maximized => glyph_restore(p, c, col),
                    1 => glyph_max(p, c, col),
                    _ => glyph_close(p, c, col),
                }
                if resp.clicked() {
                    match k {
                        0 => ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(true)),
                        1 => ctx.send_viewport_cmd(egui::ViewportCommand::Maximized(!maximized)),
                        _ => ctx.send_viewport_cmd(egui::ViewportCommand::Close),
                    }
                }
                bx += wb;
            }
        });
        if let Some(c) = cmd {
            self.cmd(ctx, c);
        }
    }

    // -----------------------------------------------------------------------------------------
    // Selector de color
    // -----------------------------------------------------------------------------------------

    fn open_color(&mut self, ctx: &egui::Context, t: ColorTarget, at: Pos2) {
        let now = ctx.input(|i| i.time).to_bits();
        self.color_popup = match self.color_popup {
            Some((cur, _, _)) if cur == t => None,
            _ => Some((t, at, now)),
        };
    }

    fn color_of(&self, t: ColorTarget) -> Option<[u8; 4]> {
        match t {
            ColorTarget::Ink => Some(self.ink_color),
            ColorTarget::TextColor(id) => self.item(id).and_then(|i| i.text.as_ref()).map(|t| t.color),
            ColorTarget::TextBg(id) => self.item(id).and_then(|i| i.text.as_ref()).and_then(|t| t.bg),
            ColorTarget::Frame(id) => self.item(id).and_then(|i| i.frame.as_ref()).map(|f| f.color),
            ColorTarget::Background => Some([self.bg[0], self.bg[1], self.bg[2], 255]),
        }
    }

    fn set_color(&mut self, t: ColorTarget, c: Option<[u8; 4]>) {
        match t {
            ColorTarget::Ink => {
                let Some(c) = c else { return };
                self.ink_color = c;
                // también los textos y trazos seleccionados
                let sel = self.sel.clone();
                for it in self.board.items.iter_mut().filter(|i| sel.contains(&i.id)) {
                    if let Some(t) = it.text.as_mut() {
                        t.color = c;
                        self.dirty = true;
                    }
                    if let Some(k) = it.ink.as_mut() {
                        k.color = c;
                        self.dirty = true;
                    }
                }
            }
            ColorTarget::TextColor(id) => {
                if let (Some(c), Some(t)) = (c, self.item_mut(id).and_then(|i| i.text.as_mut())) {
                    t.color = c;
                }
                self.dirty = true;
            }
            ColorTarget::TextBg(id) => {
                if let Some(t) = self.item_mut(id).and_then(|i| i.text.as_mut()) {
                    t.bg = c;
                }
                self.dirty = true;
            }
            ColorTarget::Background => {
                if let Some(c) = c {
                    self.bg = [c[0], c[1], c[2]];
                    // con el tema blueprint no se vería el cambio: se vuelve al fondo propio
                    self.blueprint = false;
                    self.dirty = true;
                }
            }
            ColorTarget::Frame(id) => {
                if let (Some(c), Some(f)) = (c, self.item_mut(id).and_then(|i| i.frame.as_mut())) {
                    // el marco se queda translúcido para que se vea el contenido
                    f.color = [c[0], c[1], c[2], c[3].min(70)];
                }
                self.dirty = true;
            }
        }
    }

    /// Ventana del selector: paleta de círculos, selector completo y, para el fondo del texto, «sin fondo».
    /// Color por defecto de cada cosa (`None` = sin color: el fondo del texto).
    fn default_color(target: ColorTarget) -> Option<[u8; 4]> {
        match target {
            ColorTarget::Ink | ColorTarget::TextColor(_) => Some(crate::app::DEFAULT_INK),
            ColorTarget::TextBg(_) => None,
            ColorTarget::Frame(_) => Some(crate::app::DEFAULT_FRAME),
            ColorTarget::Background => Some([crate::app::DEFAULT_BG[0], crate::app::DEFAULT_BG[1], crate::app::DEFAULT_BG[2], 255]),
        }
    }

    /// Selector de color de ancho fijo: título, «Por defecto», paleta en rejilla de 7 × 2 y el
    /// selector completo (con transparencia sólo donde tiene sentido).
    pub(crate) fn color_popup_ui(&mut self, ctx: &egui::Context) {
        let Some((target, at, opened)) = self.color_popup else { return };
        let mut cur = self.color_of(target).map(c32).unwrap_or(Color32::from_rgba_unmultiplied(0, 0, 0, 160));
        let mut set: Option<Option<[u8; 4]>> = None;
        let title = match target {
            ColorTarget::Ink => t("Color del texto, del trazo y de lo seleccionado"),
            ColorTarget::TextColor(_) => t("Color del texto"),
            ColorTarget::TextBg(_) => t("Fondo del texto"),
            ColorTarget::Frame(_) => t("Color del marco…"),
            ColorTarget::Background => t("Color de fondo"),
        };
        let alpha = if matches!(target, ColorTarget::TextBg(_) | ColorTarget::Frame(_)) { egui::color_picker::Alpha::OnlyBlend } else { egui::color_picker::Alpha::Opaque };
        let default = Self::default_color(target);
        const W: f32 = 236.0;
        // que no se salga de la ventana
        let screen = ctx.content_rect();
        let at = pos2(at.x.clamp(screen.left() + 8.0, screen.right() - W - 32.0), at.y.clamp(screen.top() + 8.0, (screen.bottom() - 420.0).max(screen.top() + 8.0)));
        let resp = egui::Area::new(egui::Id::new("color-popup")).fixed_pos(at).order(egui::Order::Foreground).show(ctx, |ui| {
            egui::Frame::popup(ui.style()).corner_radius(CornerRadius::same(12)).inner_margin(egui::Margin::same(12)).show(ui, |ui| {
                ui.set_width(W);
                ui.label(egui::RichText::new(title).size(13.0).color(TEXT));
                ui.add_space(6.0);
                // por defecto
                let (row, rresp) = ui.allocate_exact_size(vec2(W, 30.0), Sense::click());
                let hot = rresp.hovered();
                if hot {
                    ui.painter().rect_filled(row, CornerRadius::same(8), Color32::from_white_alpha(14));
                }
                let sw = Rect::from_center_size(pos2(row.left() + 15.0, row.center().y), Vec2::splat(24.0));
                swatch(ui, ui.painter(), sw, "pal-default", default.map(c32));
                let label = if default.is_some() { t("Por defecto") } else { t("Sin fondo") };
                ui.painter().text(pos2(sw.right() + 10.0, row.center().y), egui::Align2::LEFT_CENTER, label, FontId::proportional(13.0), TEXT);
                let is_default = self.color_of(target) == default;
                if is_default {
                    ui.painter().text(pos2(row.right() - 8.0, row.center().y), egui::Align2::RIGHT_CENTER, "✔", FontId::proportional(13.0), ACCENT);
                }
                if rresp.clicked() || ui.interact(sw, egui::Id::new(("swatch", "pal-default")), Sense::click()).clicked() {
                    set = Some(default);
                }
                ui.add_space(8.0);
                // paleta: rejilla exacta de 7 columnas
                let (cols, size) = (7usize, 26.0);
                let gap = (W - cols as f32 * size) / (cols as f32 - 1.0);
                let rows = PALETTE.len().div_ceil(cols);
                let (grid, _) = ui.allocate_exact_size(vec2(W, rows as f32 * size + (rows as f32 - 1.0) * gap), Sense::hover());
                for (k, c) in PALETTE.iter().enumerate() {
                    let (cx, cy) = ((k % cols) as f32, (k / cols) as f32);
                    let r = Rect::from_min_size(grid.min + vec2(cx * (size + gap), cy * (size + gap)), Vec2::splat(size));
                    if swatch(ui, ui.painter(), r, &format!("pal{k}"), Some(c32(*c))).clicked() {
                        set = Some(Some(*c));
                    }
                    if self.color_of(target).is_some_and(|cur| cur[..3] == c[..3]) {
                        ui.painter().circle_stroke(r.center(), size * 0.5 + 2.5, Stroke::new(1.5, ACCENT));
                    }
                }
                ui.add_space(10.0);
                ui.separator();
                ui.add_space(4.0);
                // selector completo, del mismo ancho
                ui.spacing_mut().slider_width = W;
                if egui::color_picker::color_picker_color32(ui, &mut cur, alpha) {
                    set = Some(Some(cur.to_srgba_unmultiplied()));
                }
            });
        });
        if let Some(c) = set {
            self.set_color(target, c);
        }
        let just_opened = ctx.input(|i| i.time).to_bits() == opened;
        if !just_opened && (resp.response.clicked_elsewhere() || ctx.input(|i| i.key_pressed(egui::Key::Escape))) {
            self.color_popup = None;
        }
    }

    // -----------------------------------------------------------------------------------------
    // Formato del texto
    // -----------------------------------------------------------------------------------------

    /// Con un solo texto seleccionado (sin editarlo) también se muestra su barra de formato.
    pub(crate) fn selected_text_format_bar(&mut self, ctx: &egui::Context) {
        if self.editing.is_some() || self.tool != Tool::Select || self.sel.len() != 1 || !matches!(self.drag, crate::app::Drag::None) {
            return;
        }
        let id = *self.sel.iter().next().unwrap();
        let Some(it) = self.item(id).filter(|i| i.text.is_some()) else { return };
        let r = self.board.aabb(it);
        let sr = Rect::from_two_pos(self.to_screen(r.min), self.to_screen(r.max));
        if !self.canvas.intersects(sr) {
            return;
        }
        self.format_bar(ctx, id, sr);
    }

    /// Barra flotante sobre el texto con todo su formato: familia, tamaño, negrita, cursiva,
    /// subrayado, tachado, alineación, color y fondo.
    pub(crate) fn format_bar(&mut self, ctx: &egui::Context, id: u64, over: Rect) {
        let Some(tx) = self.item(id).and_then(|i| i.text.clone()) else { return };
        let scale = self.item(id).map(|i| i.scale).unwrap_or(1.0);
        let mut acts: Vec<Fmt> = Vec::new();
        // encima del texto si cabe; si no, debajo
        // (por encima también del tirador de giro, que sale 26 pt sobre el borde)
        // (y por debajo de las pastillas de la cabecera, que flotan sobre el lienzo)
        let y = if over.top() - 80.0 >= self.canvas.top() + 52.0 { over.top() - 80.0 } else { over.bottom() + 8.0 };
        let pos = pos2(over.left().clamp(self.canvas.left() + 4.0, (self.canvas.right() - 520.0).max(self.canvas.left() + 4.0)), y);
        egui::Area::new(egui::Id::new("format-bar")).fixed_pos(pos).order(egui::Order::Foreground).show(ctx, |ui| {
            egui::Frame::new().fill(Color32::from_rgb(0x22, 0x22, 0x27)).stroke(Stroke::new(1.0, Color32::from_white_alpha(20))).corner_radius(CornerRadius::same(10)).inner_margin(egui::Margin::symmetric(6, 4)).show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 3.0;
                    if ui.selectable_label(!tx.mono, egui::RichText::new("Sans").size(13.0)).on_hover_text(t("Letra proporcional")).clicked() {
                        acts.push(Fmt::Mono(false));
                    }
                    if ui.selectable_label(tx.mono, egui::RichText::new("Mono").size(13.0).monospace()).on_hover_text(t("Letra de ancho fijo")).clicked() {
                        acts.push(Fmt::Mono(true));
                    }
                    ui.separator();
                    if ui.button("−").on_hover_text(t("Más pequeño")).clicked() {
                        acts.push(Fmt::Size(1.0 / 1.15));
                    }
                    ui.label(egui::RichText::new(format!("{:.0}", tx.size * scale)).size(13.0).color(TEXT)).on_hover_text(t("Tamaño de la letra (unidades del tablero)"));
                    if ui.button("+").on_hover_text(t("Más grande")).clicked() {
                        acts.push(Fmt::Size(1.15));
                    }
                    ui.separator();
                    if ui.selectable_label(tx.bold, egui::RichText::new("B").strong().size(15.0)).on_hover_text(t("Negrita")).clicked() {
                        acts.push(Fmt::Bold);
                    }
                    if ui.selectable_label(tx.italic, egui::RichText::new("I").italics().size(15.0)).on_hover_text(t("Cursiva")).clicked() {
                        acts.push(Fmt::Italic);
                    }
                    if ui.selectable_label(tx.underline, egui::RichText::new("U").underline().size(15.0)).on_hover_text(t("Subrayado")).clicked() {
                        acts.push(Fmt::Underline);
                    }
                    if ui.selectable_label(tx.strike, egui::RichText::new("S").strikethrough().size(15.0)).on_hover_text(t("Tachado")).clicked() {
                        acts.push(Fmt::Strike);
                    }
                    ui.separator();
                    for (a, hint) in [(0u8, "Alinear a la izquierda"), (1, "Centrar"), (2, "Alinear a la derecha")] {
                        let (r, resp) = ui.allocate_exact_size(vec2(26.0, 24.0), Sense::click());
                        let on = tx.align == a;
                        if on || resp.hovered() {
                            ui.painter().rect_filled(r, CornerRadius::same(6), if on { ACCENT.gamma_multiply(0.35) } else { Color32::from_white_alpha(14) });
                        }
                        glyph_align(ui.painter(), r.center(), a, TEXT);
                        if resp.on_hover_text(t(hint)).clicked() {
                            acts.push(Fmt::Align(a));
                        }
                    }
                    ui.separator();
                    let (r, _) = ui.allocate_exact_size(Vec2::splat(24.0), Sense::hover());
                    if swatch(ui, ui.painter(), r, "fmt-color", Some(c32(tx.color))).on_hover_text(t("Color del texto")).clicked() {
                        acts.push(Fmt::Color(ColorTarget::TextColor(id), r.left_bottom() + vec2(-60.0, 8.0)));
                    }
                    let (r, _) = ui.allocate_exact_size(Vec2::splat(24.0), Sense::hover());
                    if swatch(ui, ui.painter(), r, "fmt-bg", tx.bg.map(c32)).on_hover_text(t("Fondo del texto")).clicked() {
                        acts.push(Fmt::Color(ColorTarget::TextBg(id), r.left_bottom() + vec2(-60.0, 8.0)));
                    }
                });
            });
        });
        if acts.is_empty() {
            return;
        }
        // cada cambio se puede deshacer (salvo mientras se escribe: la edición ya guardó su estado)
        if self.editing.is_none() && acts.iter().any(|a| !matches!(a, Fmt::Color(..))) {
            self.snapshot();
        }
        for a in acts {
            if let Fmt::Color(target, at) = a {
                self.open_color(ctx, target, at);
                continue;
            }
            let Some(txm) = self.item_mut(id).and_then(|i| i.text.as_mut()) else { return };
            match a {
                Fmt::Mono(m) => txm.mono = m,
                Fmt::Size(f) => txm.size = (txm.size * f).max(0.01),
                Fmt::Bold => txm.bold = !txm.bold,
                Fmt::Italic => txm.italic = !txm.italic,
                Fmt::Underline => txm.underline = !txm.underline,
                Fmt::Strike => txm.strike = !txm.strike,
                Fmt::Align(v) => {
                    txm.align = v;
                    // el punto fijo del texto depende de la alineación
                    if let Some(e) = self.editing.as_mut() {
                        e.anchor = None;
                    }
                }
                Fmt::Color(..) => {}
            }
            self.dirty = true;
        }
        // el editor no debe perder el foco por pulsar la barra
        if let Some(e) = self.editing.as_mut() {
            e.focus = true;
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Controles e iconos (dibujados con vectores, sin fuentes de iconos)
// ---------------------------------------------------------------------------------------------

/// Pastilla flotante: un área propia con fondo translúcido redondeado.
fn pill(ctx: &egui::Context, id: &str, pos: Pos2, size: Vec2, alpha: f32, add: impl FnOnce(&mut egui::Ui, Rect)) {
    if alpha < 0.02 {
        return;
    }
    egui::Area::new(egui::Id::new(id)).fixed_pos(pos).order(egui::Order::Foreground).show(ctx, |ui| {
        ui.set_opacity(alpha);
        let (r, _) = ui.allocate_exact_size(size, Sense::hover());
        let p = ui.painter();
        p.rect_filled(r.translate(vec2(0.0, 2.0)), CornerRadius::same(12), Color32::from_black_alpha(60));
        p.rect_filled(r, CornerRadius::same(12), Color32::from_rgba_unmultiplied(28, 28, 33, 225));
        p.rect_stroke(r, CornerRadius::same(12), Stroke::new(1.0, Color32::from_white_alpha(16)), StrokeKind::Inside);
        add(ui, r);
    });
}

fn icon_button(ui: &egui::Ui, p: &egui::Painter, r: Rect, id: &str, active: bool, hint: &str, draw: impl Fn(&egui::Painter, Pos2, Color32)) -> egui::Response {
    let resp = ui.interact(r, egui::Id::new(("icon-btn", id)), Sense::click()).on_hover_text(hint);
    if active {
        p.rect_filled(r, CornerRadius::same(8), ACCENT.gamma_multiply(0.4));
    } else if resp.hovered() {
        p.rect_filled(r, CornerRadius::same(8), Color32::from_white_alpha(16));
    }
    draw(p, r.center(), if active { Color32::WHITE } else { TEXT });
    resp
}

/// Muestra de color redonda; `None` = sin color (círculo tachado).
fn swatch(ui: &egui::Ui, p: &egui::Painter, r: Rect, id: &str, c: Option<Color32>) -> egui::Response {
    let resp = ui.interact(r, egui::Id::new(("swatch", id)), Sense::click());
    let rad = r.width().min(r.height()) * 0.5 - 1.0;
    let ring = if resp.hovered() { Color32::WHITE } else { Color32::from_white_alpha(90) };
    match c {
        Some(c) => {
            // cuadros bajo los colores translúcidos
            if c.a() < 255 {
                p.circle_filled(r.center(), rad, Color32::from_gray(90));
                p.circle_filled(r.center() + vec2(-rad * 0.35, -rad * 0.35), rad * 0.45, Color32::from_gray(150));
            }
            p.circle_filled(r.center(), rad, c);
        }
        None => {
            p.circle_filled(r.center(), rad, Color32::from_gray(40));
            p.line_segment([r.center() + vec2(-rad * 0.6, rad * 0.6), r.center() + vec2(rad * 0.6, -rad * 0.6)], Stroke::new(1.5, Color32::from_rgb(220, 80, 70)));
        }
    }
    p.circle_stroke(r.center(), rad, Stroke::new(if resp.hovered() { 2.0 } else { 1.5 }, ring));
    resp
}

fn st(col: Color32) -> Stroke {
    Stroke::new(1.6, col)
}

fn glyph_menu(p: &egui::Painter, c: Pos2, col: Color32) {
    for y in [-5.0, 0.0, 5.0] {
        p.line_segment([c + vec2(-8.0, y), c + vec2(8.0, y)], st(col));
    }
}

fn glyph_select(p: &egui::Painter, c: Pos2, col: Color32) {
    let pts = [vec2(-5.0, -8.0), vec2(6.0, 3.0), vec2(1.0, 3.5), vec2(4.0, 9.0), vec2(1.5, 10.0), vec2(-1.5, 4.5), vec2(-5.0, 8.0)].map(|v| c + v + vec2(0.0, -1.0));
    p.add(egui::Shape::closed_line(pts.to_vec(), Stroke::new(1.5, col)));
}

fn glyph_text(p: &egui::Painter, c: Pos2, col: Color32) {
    p.line_segment([c + vec2(-7.0, -7.0), c + vec2(7.0, -7.0)], Stroke::new(2.0, col));
    p.line_segment([c + vec2(0.0, -7.0), c + vec2(0.0, 8.0)], Stroke::new(2.0, col));
    p.line_segment([c + vec2(-3.0, 8.0), c + vec2(3.0, 8.0)], st(col));
}

fn glyph_pen(p: &egui::Painter, c: Pos2, col: Color32) {
    let (a, b) = (c + vec2(-6.0, 6.0), c + vec2(6.0, -6.0));
    let n = vec2(2.2, 2.2);
    let body = vec![a + vec2(-n.x, 0.0) + vec2(1.0, -1.0), b + vec2(-2.0, -1.0), b + vec2(1.0, 2.0), a + vec2(0.0, n.y) + vec2(1.0, -1.0)];
    p.add(egui::Shape::closed_line(body, Stroke::new(1.5, col)));
    p.add(egui::Shape::convex_polygon(vec![a + vec2(-2.0, 2.0), a + vec2(-1.2, -1.5), a + vec2(1.5, 1.2)], col, Stroke::NONE));
    p.line_segment([b + vec2(-3.5, -0.5), b + vec2(0.5, 3.5)], st(col));
}

/// Marco: rectángulo con una pestaña de título arriba a la izquierda.
fn glyph_frame(p: &egui::Painter, c: Pos2, col: Color32) {
    let r = Rect::from_min_max(c + vec2(-8.5, -4.5), c + vec2(8.5, 8.0));
    p.rect_stroke(r, CornerRadius::same(2), st(col), StrokeKind::Inside);
    p.rect_filled(Rect::from_min_max(c + vec2(-8.5, -9.0), c + vec2(-1.0, -5.5)), CornerRadius::same(1), col);
}

/// Recorte: dos escuadras cruzadas.
fn glyph_crop(p: &egui::Painter, c: Pos2, col: Color32) {
    let s = Stroke::new(1.6, col);
    p.add(egui::Shape::line(vec![c + vec2(-5.0, -9.0), c + vec2(-5.0, 5.0), c + vec2(9.0, 5.0)], s));
    p.add(egui::Shape::line(vec![c + vec2(-9.0, -5.0), c + vec2(5.0, -5.0), c + vec2(5.0, 9.0)], s));
}

/// Cuadrícula (el antiguo icono de marco).
fn glyph_grid(p: &egui::Painter, c: Pos2, col: Color32) {
    for y in [-4.5, 4.5] {
        p.line_segment([c + vec2(-9.0, y), c + vec2(9.0, y)], st(col));
    }
    for x in [-4.5, 4.5] {
        p.line_segment([c + vec2(x, -9.0), c + vec2(x, 9.0)], st(col));
    }
}

fn glyph_align(p: &egui::Painter, c: Pos2, a: u8, col: Color32) {
    for (k, w) in [16.0, 10.0, 16.0, 8.0].iter().enumerate() {
        let y = -6.0 + k as f32 * 4.0;
        let x0 = match a {
            1 => -w / 2.0,
            2 => 8.0 - w,
            _ => -8.0,
        };
        p.line_segment([c + vec2(x0, y), c + vec2(x0 + w, y)], Stroke::new(1.5, col));
    }
}

fn glyph_min(p: &egui::Painter, c: Pos2, col: Color32) {
    p.line_segment([c + vec2(-6.0, 0.0), c + vec2(6.0, 0.0)], st(col));
}

fn glyph_max(p: &egui::Painter, c: Pos2, col: Color32) {
    p.rect_stroke(Rect::from_center_size(c, Vec2::splat(11.0)), CornerRadius::same(2), st(col), StrokeKind::Inside);
}

fn glyph_restore(p: &egui::Painter, c: Pos2, col: Color32) {
    p.rect_stroke(Rect::from_min_size(c + vec2(-6.0, -2.5), Vec2::splat(8.5)), CornerRadius::same(2), st(col), StrokeKind::Inside);
    p.add(egui::Shape::line(vec![c + vec2(-2.5, -2.5), c + vec2(-2.5, -6.0), c + vec2(6.0, -6.0), c + vec2(6.0, 2.5), c + vec2(2.5, 2.5)], st(col)));
}

fn glyph_close(p: &egui::Painter, c: Pos2, col: Color32) {
    p.line_segment([c + vec2(-5.5, -5.5), c + vec2(5.5, 5.5)], st(col));
    p.line_segment([c + vec2(-5.5, 5.5), c + vec2(5.5, -5.5)], st(col));
}
