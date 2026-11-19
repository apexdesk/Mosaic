//! Opacidad de toda la ventana (para calcar encima de otra aplicación).
//!
//! Wayland no ofrece «opacidad de ventana» al cliente, y bajar la opacidad de cada cosa por separado
//! no da un resultado uniforme (una imagen al 50 % sobre un fondo al 50 % queda al 75 %). Así que se
//! pinta todo opaco y, como último paso del fotograma, una pasada de OpenGL multiplica el búfer entero
//! (color premultiplicado y alfa) por la opacidad: `destino × o`. El compositor ve la ventana
//! translúcida por igual.
//!
//! La misma pasada redondea las esquinas de la ventana sin decoración (cuando no está maximizada):
//! fuera del rectángulo redondeado multiplica por 0, con el borde suavizado.
use eframe::egui_glow::{self, ShaderVersion, glow, glow::HasContext};
use std::sync::{Arc, Mutex};

struct Gl {
    prog: glow::Program,
    vao: glow::VertexArray,
}

/// Programa compilado una vez (o `None` si el contexto no puede: entonces no hay transparencia).
#[derive(Clone, Default)]
pub struct Fade(Arc<Mutex<Option<Option<Gl>>>>);

const VS: &str = "
void main() {
    // triángulo que cubre la pantalla, sin búferes
    vec2 p = vec2(float((gl_VertexID << 1) & 2), float(gl_VertexID & 2));
    gl_Position = vec4(p * 2.0 - 1.0, 0.0, 1.0);
}
";

const FS: &str = "
uniform float u_o;
uniform vec2 u_size;
uniform float u_r;
out vec4 o_color;
void main() {
    // distancia con signo al rectángulo redondeado de la ventana (en píxeles)
    vec2 p = gl_FragCoord.xy - u_size * 0.5;
    vec2 q = abs(p) - (u_size * 0.5 - vec2(u_r));
    float d = length(max(q, 0.0)) + min(max(q.x, q.y), 0.0) - u_r;
    float cov = u_r > 0.0 ? clamp(0.5 - d, 0.0, 1.0) : 1.0;
    o_color = vec4(u_o * cov);
}
";

fn build(gl: &glow::Context) -> Option<Gl> {
    let head = match ShaderVersion::get(gl) {
        ShaderVersion::Gl140 => "#version 140\n",
        ShaderVersion::Es300 => "#version 300 es\nprecision mediump float;\n",
        _ => return None,
    };
    // SAFETY: dentro de la callback de pintado (contexto activo); objetos de este contexto
    unsafe {
        let prog = gl.create_program().ok()?;
        let mut shaders = Vec::new();
        for (kind, src) in [(glow::VERTEX_SHADER, VS), (glow::FRAGMENT_SHADER, FS)] {
            let sh = gl.create_shader(kind).ok()?;
            gl.shader_source(sh, &format!("{head}{src}"));
            gl.compile_shader(sh);
            if !gl.get_shader_compile_status(sh) {
                gl.delete_shader(sh);
                gl.delete_program(prog);
                return None;
            }
            gl.attach_shader(prog, sh);
            shaders.push(sh);
        }
        gl.link_program(prog);
        let ok = gl.get_program_link_status(prog);
        for s in shaders {
            gl.detach_shader(prog, s);
            gl.delete_shader(s);
        }
        if !ok {
            gl.delete_program(prog);
            return None;
        }
        let vao = gl.create_vertex_array().ok()?;
        Some(Gl { prog, vao })
    }
}

impl Fade {
    /// Añade la pasada final, encima de todo lo demás del fotograma.
    /// `radius`: esquinas redondeadas en puntos (0 = rectas, p. ej. maximizada).
    pub fn paint(&self, ctx: &egui::Context, opacity: f32, radius: f32) {
        if opacity >= 0.999 && radius <= 0.0 {
            return;
        }
        let r_px = radius * ctx.pixels_per_point();
        let me = self.clone();
        let o = opacity.clamp(0.05, 1.0);
        let screen = ctx.content_rect();
        let cb = egui_glow::CallbackFn::new(move |info, painter| {
            let [sw, sh] = info.screen_size_px;
            let gl = painter.gl();
            let mut st = me.0.lock().unwrap_or_else(|e| e.into_inner());
            let g = st.get_or_insert_with(|| build(gl));
            let Some(g) = g.as_ref() else { return };
            // SAFETY: contexto activo en la callback; egui_glow restaura su estado después
            unsafe {
                gl.disable(glow::SCISSOR_TEST);
                gl.enable(glow::BLEND);
                gl.blend_equation(glow::FUNC_ADD);
                // destino × color de la fuente (= o en los cuatro canales)
                gl.blend_func(glow::ZERO, glow::SRC_COLOR);
                gl.use_program(Some(g.prog));
                gl.uniform_1_f32(gl.get_uniform_location(g.prog, "u_o").as_ref(), o);
                gl.uniform_2_f32(gl.get_uniform_location(g.prog, "u_size").as_ref(), sw as f32, sh as f32);
                gl.uniform_1_f32(gl.get_uniform_location(g.prog, "u_r").as_ref(), r_px);
                gl.bind_vertex_array(Some(g.vao));
                gl.draw_arrays(glow::TRIANGLES, 0, 3);
                gl.bind_vertex_array(None);
                gl.use_program(None);
            }
        });
        let layer = egui::LayerId::new(egui::Order::Debug, egui::Id::new("window-fade"));
        ctx.layer_painter(layer).add(egui::PaintCallback { rect: screen, callback: Arc::new(cb) });
    }
}
