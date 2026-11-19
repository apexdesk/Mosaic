//! (refs) Mini reproductor de las referencias de audio, el de File Browser leyendo de los bytes del
//! tablero en vez de una ruta.
//!
//! Reproductor de audio integrado: `symphonia` decodifica (MP3, WAV, OGG/Vorbis, FLAC, AAC/M4A, ALAC)
//! en un hilo y `cpal` saca el sonido por el dispositivo predeterminado. Sin dependencias externas.
//! En Android todavía no hay salida de sonido.
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use crate::decode::{lock, Bytes};
use std::time::Instant;

use symphonia::core::codecs::{CODEC_TYPE_NULL, DecoderOptions};
use symphonia::core::errors::Error as SymErr;
use symphonia::core::formats::{FormatOptions, FormatReader, SeekMode, SeekTo};
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::{MetadataOptions, StandardTagKey};
use symphonia::core::probe::Hint;
use symphonia::core::units::Time;

pub fn supported() -> bool {
    true
}

pub const AUDIO_EXTS: &[&str] = &["mp3", "wav", "ogg", "oga", "flac", "m4a", "aac", "alac", "aif", "aiff"];

pub fn is_audio_path(p: &std::path::Path) -> bool {
    p.extension().and_then(|e| e.to_str()).is_some_and(|e| AUDIO_EXTS.iter().any(|x| x.eq_ignore_ascii_case(e)))
}

fn spawn<F: FnOnce() + Send + 'static>(name: &str, f: F) {
    let _ = std::thread::Builder::new().name(name.to_string()).stack_size(16 << 20).spawn(f);
}

/// Duración y forma de onda (picos 0..255 en `n` tramos) al importar. Las pistas de más de 20
/// minutos no se recorren enteras: sólo la duración.
pub fn probe(bytes: &Bytes, ext: &str, n: usize) -> Result<(f64, Vec<u8>), String> {
    use symphonia::core::audio::SampleBuffer;
    let mut format = open_format(bytes, ext)?;
    let track = format.tracks().iter().find(|t| t.codec_params.codec != CODEC_TYPE_NULL).ok_or(crate::media::t("sin pista de audio"))?;
    let (id, params) = (track.id, track.codec_params.clone());
    let rate = params.sample_rate.unwrap_or(44100).max(1) as f64;
    let mut duration = params.n_frames.map(|f| f as f64 / rate).unwrap_or(0.0);
    if duration > 20.0 * 60.0 {
        return Ok((duration, Vec::new()));
    }
    let mut decoder = symphonia::default::get_codecs().make(&params, &DecoderOptions::default()).map_err(|e| e.to_string())?;
    // máximos absolutos por bloque de 1/100 s; luego se agrupan en `n` tramos
    let block = (rate / 100.0).max(1.0) as usize;
    let mut blocks: Vec<f32> = Vec::new();
    let (mut acc, mut cnt) = (0f32, 0usize);
    let mut sbuf: Option<SampleBuffer<f32>> = None;
    let mut frames = 0u64;
    loop {
        let packet = match format.next_packet() {
            Ok(p) => p,
            Err(_) => break,
        };
        if packet.track_id() != id {
            continue;
        }
        let Ok(dec) = decoder.decode(&packet) else { continue };
        let spec = *dec.spec();
        let ch = spec.channels.count().max(1);
        let buf = sbuf.get_or_insert_with(|| SampleBuffer::new(dec.capacity() as u64, spec));
        if buf.capacity() < dec.capacity() {
            *buf = SampleBuffer::new(dec.capacity() as u64, spec);
        }
        buf.copy_interleaved_ref(dec);
        for f in buf.samples().chunks(ch) {
            let v = f.iter().fold(0f32, |m, s| m.max(s.abs()));
            acc = acc.max(v);
            cnt += 1;
            frames += 1;
            if cnt == block {
                blocks.push(acc);
                acc = 0.0;
                cnt = 0;
            }
        }
    }
    if cnt > 0 {
        blocks.push(acc);
    }
    if duration <= 0.0 {
        duration = frames as f64 / rate;
    }
    let mut peaks = Vec::with_capacity(n);
    if !blocks.is_empty() {
        for k in 0..n {
            let a = k * blocks.len() / n;
            let b = ((k + 1) * blocks.len() / n).max(a + 1).min(blocks.len());
            let m = blocks[a..b].iter().fold(0f32, |m, v| m.max(*v));
            peaks.push((m.clamp(0.0, 1.0).sqrt() * 255.0) as u8);
        }
    }
    Ok((duration, peaks))
}

/// Datos de cabecera: duración, etiquetas y carátula.
#[derive(Default)]
struct Meta {
    duration: f64,
    title: String,
    artist: String,
    album: String,
    cover: Option<egui::ColorImage>,
}

/// Lector de `symphonia` sobre los bytes del tablero (mapeados o en memoria).
struct BytesSource {
    b: Bytes,
    pos: u64,
}

impl std::io::Read for BytesSource {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let s = self.b.as_slice();
        let at = (self.pos as usize).min(s.len());
        let n = buf.len().min(s.len() - at);
        buf[..n].copy_from_slice(&s[at..at + n]);
        self.pos += n as u64;
        Ok(n)
    }
}

impl std::io::Seek for BytesSource {
    fn seek(&mut self, to: std::io::SeekFrom) -> std::io::Result<u64> {
        let len = self.b.len() as i64;
        let p = match to {
            std::io::SeekFrom::Start(v) => v as i64,
            std::io::SeekFrom::End(d) => len + d,
            std::io::SeekFrom::Current(d) => self.pos as i64 + d,
        };
        if p < 0 {
            return Err(std::io::Error::new(std::io::ErrorKind::InvalidInput, "posición negativa"));
        }
        self.pos = p as u64;
        Ok(self.pos)
    }
}

impl symphonia::core::io::MediaSource for BytesSource {
    fn is_seekable(&self) -> bool {
        true
    }
    fn byte_len(&self) -> Option<u64> {
        Some(self.b.len() as u64)
    }
}

fn open_format(bytes: &Bytes, ext: &str) -> Result<Box<dyn FormatReader>, String> {
    let mss = MediaSourceStream::new(Box::new(BytesSource { b: bytes.clone(), pos: 0 }), Default::default());
    let mut hint = Hint::new();
    if !ext.is_empty() {
        hint.with_extension(ext);
    }
    let probed = symphonia::default::get_probe()
        .format(&hint, mss, &FormatOptions::default(), &MetadataOptions::default())
        .map_err(|e| e.to_string())?;
    Ok(probed.format)
}

fn cover_image(data: &[u8]) -> Option<egui::ColorImage> {
    if data.len() > 16 * 1024 * 1024 {
        return None;
    }
    let img = if data.starts_with(&[0xFF, 0xD8]) {
        let mut d = jpeg_decoder::Decoder::new(std::io::Cursor::new(data));
        d.read_info().ok()?;
        let i = d.info()?;
        if i.width as u64 * i.height as u64 > 100_000_000 {
            return None;
        }
        let px = d.decode().ok()?;
        let rgb: Vec<u8> = match i.pixel_format {
            jpeg_decoder::PixelFormat::RGB24 => px,
            jpeg_decoder::PixelFormat::L8 => px.iter().flat_map(|&v| [v, v, v]).collect(),
            _ => return None,
        };
        image::DynamicImage::ImageRgb8(image::RgbImage::from_raw(i.width as u32, i.height as u32, rgb)?)
    } else {
        let r = image::ImageReader::new(std::io::Cursor::new(data)).with_guessed_format().ok()?;
        let (w, h) = r.into_dimensions().ok()?;
        if w as u64 * h as u64 > 100_000_000 {
            return None;
        }
        image::load_from_memory(data).ok()?
    };
    let img = if img.width() > 640 || img.height() > 640 { img.thumbnail(640, 640) } else { img };
    let rgba = img.into_rgba8();
    Some(egui::ColorImage::from_rgba_unmultiplied([rgba.width() as usize, rgba.height() as usize], rgba.as_raw()))
}

fn read_meta(bytes: &Bytes, ext: &str) -> Meta {
    let mut m = Meta::default();
    let mss = MediaSourceStream::new(Box::new(BytesSource { b: bytes.clone(), pos: 0 }), Default::default());
    let mut hint = Hint::new();
    if !ext.is_empty() {
        hint.with_extension(ext);
    }
    let Ok(mut probed) = symphonia::default::get_probe().format(&hint, mss, &FormatOptions::default(), &MetadataOptions::default()) else {
        return m;
    };
    if let Some(t) = probed.format.tracks().iter().find(|t| t.codec_params.codec != CODEC_TYPE_NULL) {
        let p = &t.codec_params;
        if let (Some(n), Some(sr)) = (p.n_frames, p.sample_rate)
            && sr > 0 {
                m.duration = n as f64 / sr as f64;
            }
    }
    let mut take = |rev: &symphonia::core::meta::MetadataRevision| {
        for tag in rev.tags() {
            let v = tag.value.to_string();
            match tag.std_key {
                Some(StandardTagKey::TrackTitle) if m.title.is_empty() => m.title = v,
                Some(StandardTagKey::Artist) | Some(StandardTagKey::AlbumArtist) if m.artist.is_empty() => m.artist = v,
                Some(StandardTagKey::Album) if m.album.is_empty() => m.album = v,
                _ => {}
            }
        }
        if m.cover.is_none()
            && let Some(v) = rev.visuals().first() {
                m.cover = cover_image(&v.data);
            }
    };
    if let Some(md) = probed.metadata.get()
        && let Some(rev) = md.current() {
            take(rev);
        }
    if let Some(rev) = probed.format.metadata().current() {
        take(rev);
    }
    m
}

/// Estado de una sesión de reproducción (cada arranque o salto crea una nueva).
struct Session {
    /// fotogramas realmente entregados al dispositivo
    played: AtomicU64,
    rate: AtomicU32,
    ended: AtomicBool,
    error: Mutex<Option<String>>,
    /// segundo real donde empezó la sesión tras el salto (bits de f64; NaN = aún no se sabe)
    from: AtomicU64,
}

impl Session {
    fn new() -> Session {
        Session { played: AtomicU64::new(0), rate: AtomicU32::new(0), ended: AtomicBool::new(false), error: Mutex::new(None), from: AtomicU64::new(f64::NAN.to_bits()) }
    }

    /// Inicio real de la sesión, si ya se conoce.
    fn from(&self) -> Option<f64> {
        Some(f64::from_bits(self.from.load(Ordering::Relaxed))).filter(|v| v.is_finite())
    }
}

/// Segundo al que ha llegado de verdad un salto (`None` si falló: se empieza desde el principio).
fn seeked_secs(r: Result<symphonia::core::formats::SeekedTo, SymErr>, tb: Option<symphonia::core::units::TimeBase>, asked: f64) -> f64 {
    match (r, tb) {
        (Ok(s), Some(tb)) => {
            let t = tb.calc_time(s.actual_ts);
            let v = t.seconds as f64 + t.frac;
            if v.is_finite() { v } else { asked }
        }
        (Ok(_), None) => asked,
        (Err(_), _) => 0.0,
    }
}

pub struct AudioPlayer {
    bytes: Bytes,
    ext: String,
    ctx: egui::Context,
    pub duration: f64,
    pub title: String,
    pub artist: String,
    pub album: String,
    pub tex: Option<egui::TextureHandle>,
    pub error: Option<String>,
    meta: Arc<Mutex<Option<Meta>>>,
    epoch: Arc<AtomicU64>,
    sess: Arc<Session>,
    base: f64,
    started: Option<Instant>,
}

impl AudioPlayer {
    pub fn new(ctx: &egui::Context, bytes: Bytes, ext: &str) -> AudioPlayer {
        let meta = Arc::new(Mutex::new(None));
        {
            let (meta, b, e, ctx) = (meta.clone(), bytes.clone(), ext.to_string(), ctx.clone());
            spawn("etiquetas de audio", move || {
                let m = crate::decode::guarded(|| read_meta(&b, &e)).unwrap_or_default();
                *lock(&meta) = Some(m);
                ctx.request_repaint();
            });
        }
        AudioPlayer {
            bytes,
            ext: ext.to_string(),
            ctx: ctx.clone(),
            duration: 0.0,
            title: String::new(),
            artist: String::new(),
            album: String::new(),
            tex: None,
            error: None,
            meta,
            epoch: Arc::new(AtomicU64::new(0)),
            sess: Arc::new(Session::new()),
            base: 0.0,
            started: None,
        }
    }

    pub fn playing(&self) -> bool {
        self.started.is_some()
    }

    pub fn position(&self) -> f64 {
        let rate = self.sess.rate.load(Ordering::Relaxed);
        // si el salto no llegó donde se pidió, cuenta desde donde de verdad empezó
        let base = if self.started.is_some() { self.sess.from().unwrap_or(self.base) } else { self.base };
        let p = if self.started.is_some() && rate > 0 { base + self.sess.played.load(Ordering::Relaxed) as f64 / rate as f64 } else { base };
        if self.duration > 0.0 { p.min(self.duration) } else { p }
    }

    fn stop(&mut self) {
        self.epoch.fetch_add(1, Ordering::SeqCst);
    }

    fn start(&mut self, pos: f64) {
        self.stop();
        self.error = None;
        let sess = Arc::new(Session::new());
        self.sess = sess.clone();
        let my = self.epoch.load(Ordering::SeqCst);
        let (epoch, bytes, ext, ctx) = (self.epoch.clone(), self.bytes.clone(), self.ext.clone(), self.ctx.clone());
        spawn("audio", move || {
            // un pánico del decodificador (p. ej. un FLAC roto) se muestra como error, no cierra la app
            let r = crate::decode::guarded(|| run(&bytes, &ext, pos, &sess, &epoch, my))
                .unwrap_or_else(|| Err(crate::media::t("No se pudo reproducir el audio").to_string()));
            if let Err(e) = r {
                *lock(&sess.error) = Some(e);
            }
            sess.ended.store(true, Ordering::SeqCst);
            ctx.request_repaint();
        });
        self.base = pos;
        self.started = Some(Instant::now());
    }

    pub fn toggle(&mut self) {
        if !supported() {
            return;
        }
        if self.playing() {
            self.base = self.position();
            self.started = None;
            self.stop();
        } else {
            let at = if self.duration > 0.0 && self.base >= self.duration - 0.2 { 0.0 } else { self.base };
            self.start(at);
        }
    }

    pub fn seek_to(&mut self, secs: f64) {
        if !supported() {
            return;
        }
        let max = if self.duration > 0.5 { self.duration - 0.3 } else { f64::MAX };
        self.start(secs.clamp(0.0, max));
    }

    #[allow(dead_code)]
    pub fn seek_by(&mut self, d: f64) {
        let p = self.position();
        self.seek_to(p + d);
    }

    /// Recoge las etiquetas y detecta el final de la pista o un error.
    pub fn poll(&mut self) {
        let m = lock(&self.meta).take();
        if let Some(m) = m {
            self.duration = m.duration;
            self.title = m.title;
            self.artist = m.artist;
            self.album = m.album;
            if let Some(img) = m.cover {
                self.tex = Some(self.ctx.load_texture("cover", img, egui::TextureOptions::LINEAR));
            }
        }
        if self.started.is_some() && self.sess.ended.load(Ordering::SeqCst) {
            if let Some(e) = lock(&self.sess.error).take() {
                self.error = Some(e);
                self.base = self.position();
            } else {
                self.base = if self.duration > 0.0 { self.duration } else { self.position() };
            }
            self.started = None;
        }
    }
}

impl Drop for AudioPlayer {
    fn drop(&mut self) {
        self.stop();
    }
}

fn run(bytes: &Bytes, ext: &str, start: f64, sess: &Session, epoch: &AtomicU64, my: u64) -> Result<(), String> {
    use std::collections::VecDeque;
    use symphonia::core::audio::SampleBuffer;

    let alive = || epoch.load(Ordering::SeqCst) == my;
    let mut format = open_format(bytes, ext)?;
    let track = format.tracks().iter().find(|t| t.codec_params.codec != CODEC_TYPE_NULL).ok_or(crate::media::t("sin pista de audio"))?;
    let (id, params) = (track.id, track.codec_params.clone());
    let mut decoder = symphonia::default::get_codecs().make(&params, &DecoderOptions::default()).map_err(|e| e.to_string())?;
    if start > 0.1 {
        let r = format.seek(SeekMode::Coarse, SeekTo::Time { time: Time::from(start), track_id: Some(id) });
        sess.from.store(seeked_secs(r, params.time_base, start).to_bits(), Ordering::Relaxed);
        decoder.reset();
    } else {
        sess.from.store(0f64.to_bits(), Ordering::Relaxed);
    }

    // dispositivo de salida
    let ring: Arc<Mutex<VecDeque<f32>>> = Arc::new(Mutex::new(VecDeque::new()));
    let played = Arc::new(AtomicU64::new(0));
    let (stream, rate_out, ch_out) = open_out(&ring, &played)?;
    sess.rate.store(rate_out, Ordering::Relaxed);
    let high = rate_out as usize * ch_out / 2; // ~0,5 s de audio en cola

    let mut sbuf: Option<SampleBuffer<f32>> = None;
    let (mut carry, mut phase): (Option<Vec<f32>>, f64) = (None, 0.0);
    let mut eof = false;
    while alive() {
        // mantiene `played` de la sesión al día con el contador del dispositivo
        sess.played.store(played.load(Ordering::Relaxed), Ordering::Relaxed);
        if ring.lock().map(|q| q.len()).unwrap_or(0) > high {
            std::thread::sleep(std::time::Duration::from_millis(15));
            continue;
        }
        let packet = match format.next_packet() {
            Ok(p) => p,
            Err(SymErr::ResetRequired) => break,
            Err(_) => {
                eof = true;
                break;
            }
        };
        if packet.track_id() != id {
            continue;
        }
        let buf = match decoder.decode(&packet) {
            Ok(b) => b,
            Err(SymErr::DecodeError(_)) => continue,
            Err(_) => break,
        };
        let spec = *buf.spec();
        let (rate_in, ch_in) = (spec.rate, spec.channels.count().max(1));
        if sbuf.is_none() {
            sbuf = Some(SampleBuffer::<f32>::new(buf.capacity() as u64, spec));
        }
        let sb = sbuf.as_mut().unwrap();
        sb.copy_interleaved_ref(buf);
        // canales de entrada -> canales de salida
        let src = sb.samples();
        let mut frames: Vec<f32> = Vec::with_capacity(src.len() / ch_in * ch_out + ch_out);
        if let Some(c) = carry.take() {
            frames.extend_from_slice(&c);
        }
        let had_carry = !frames.is_empty();
        for f in src.chunks_exact(ch_in) {
            for o in 0..ch_out {
                frames.push(map_channel(f, o, ch_out));
            }
        }
        let n_frames = frames.len() / ch_out;
        if n_frames == 0 {
            continue;
        }
        let mut out: Vec<f32> = Vec::new();
        if rate_in == rate_out {
            let skip = if had_carry { ch_out } else { 0 };
            out.extend_from_slice(&frames[skip..]);
            carry = None;
        } else {
            // remuestreo lineal; el último fotograma pasa al siguiente paquete
            if rate_in < 1000 {
                continue;
            }
            let step = rate_in as f64 / rate_out as f64;
            if !had_carry {
                phase = 0.0;
            }
            while (phase.floor() as usize) + 1 < n_frames {
                let i = phase.floor() as usize;
                let fr = (phase - i as f64) as f32;
                for c in 0..ch_out {
                    let a = frames[i * ch_out + c];
                    let b = frames[(i + 1) * ch_out + c];
                    out.push(a + (b - a) * fr);
                }
                phase += step;
            }
            carry = Some(frames[(n_frames - 1) * ch_out..].to_vec());
            phase -= (n_frames - 1) as f64;
        }
        if let Ok(mut q) = ring.lock() {
            q.extend(out);
        }
    }
    // vaciar la cola antes de dar la pista por terminada
    if eof {
        while alive() && ring.lock().map(|q| !q.is_empty()).unwrap_or(false) {
            sess.played.store(played.load(Ordering::Relaxed), Ordering::Relaxed);
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        if alive() {
            std::thread::sleep(std::time::Duration::from_millis(150));
        }
    }
    sess.played.store(played.load(Ordering::Relaxed), Ordering::Relaxed);
    drop(stream);
    Ok(())
}

/// Muestra del canal de salida `o` a partir de un fotograma de entrada.
fn map_channel(f: &[f32], o: usize, ch_out: usize) -> f32 {
    let ch_in = f.len();
    if ch_in == ch_out {
        f[o]
    } else if ch_in == 1 {
        f[0]
    } else if ch_out == 1 {
        f.iter().sum::<f32>() / ch_in as f32
    } else if o < ch_in {
        f[o]
    } else {
        0.0
    }
}

/// Abre la salida de sonido (cpal en escritorio, AAudio en Android). El valor devuelto mantiene
/// el flujo abierto mientras viva.
// (refs) sin la feature `audio` (p. ej. al comprobar otras plataformas sin su SDK) no hay salida
#[cfg(not(feature = "audio"))]
fn open_out(_ring: &Arc<Mutex<std::collections::VecDeque<f32>>>, _played: &Arc<AtomicU64>) -> Result<(Box<dyn std::any::Any>, u32, usize), String> {
    Err(crate::media::t("no hay dispositivo de audio").into())
}

#[cfg(feature = "audio")]
fn open_out(ring: &Arc<Mutex<std::collections::VecDeque<f32>>>, played: &Arc<AtomicU64>) -> Result<(Box<dyn std::any::Any>, u32, usize), String> {
    use cpal::traits::StreamTrait;
    let (dev, cfg) = output_device()?;
    let stream = build_stream(&dev, &cfg, ring.clone(), played.clone())?;
    stream.play().map_err(|e| e.to_string())?;
    Ok((Box::new(stream), cfg.sample_rate().0, cfg.channels() as usize))
}

#[cfg(feature = "audio")]
fn output_device() -> Result<(cpal::Device, cpal::SupportedStreamConfig), String> {
    use cpal::traits::{DeviceTrait, HostTrait};
    let dev = cpal::default_host().default_output_device().ok_or(crate::media::t("no hay dispositivo de audio"))?;
    let cfg = dev.default_output_config().map_err(|e| e.to_string())?;
    Ok((dev, cfg))
}

#[cfg(feature = "audio")]
fn build_stream(
    dev: &cpal::Device,
    cfg: &cpal::SupportedStreamConfig,
    ring: Arc<Mutex<std::collections::VecDeque<f32>>>,
    played: Arc<AtomicU64>,
) -> Result<cpal::Stream, String> {
    use cpal::SampleFormat as F;
    let conf = cfg.config();
    let ch = conf.channels as usize;
    match cfg.sample_format() {
        F::F32 => build::<f32>(dev, &conf, ring, played, ch),
        F::I16 => build::<i16>(dev, &conf, ring, played, ch),
        F::U16 => build::<u16>(dev, &conf, ring, played, ch),
        F::I32 => build::<i32>(dev, &conf, ring, played, ch),
        other => Err(crate::i18n::tf("formato de muestra no admitido: {}", &[&format!("{other:?}")])),
    }
}

#[cfg(feature = "audio")]
fn build<T>(
    dev: &cpal::Device,
    conf: &cpal::StreamConfig,
    ring: Arc<Mutex<std::collections::VecDeque<f32>>>,
    played: Arc<AtomicU64>,
    ch: usize,
) -> Result<cpal::Stream, String>
where
    T: cpal::SizedSample + cpal::FromSample<f32>,
{
    use cpal::traits::DeviceTrait;
    dev.build_output_stream(
        conf,
        move |data: &mut [T], _| {
            let mut n = 0;
            if let Ok(mut q) = ring.try_lock() {
                n = data.len().min(q.len());
                for (o, s) in data.iter_mut().zip(q.drain(..n)) {
                    *o = T::from_sample(s);
                }
            }
            for o in &mut data[n..] {
                *o = T::EQUILIBRIUM;
            }
            played.fetch_add((n / ch.max(1)) as u64, Ordering::Relaxed);
        },
        |e| eprintln!("audio: {e}"),
        None,
    )
    .map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use symphonia::core::formats::SeekedTo;
    use symphonia::core::units::TimeBase;

    #[test]
    fn failed_seek_reports_real_start() {
        let tb = Some(TimeBase::new(1, 44_100));
        assert_eq!(seeked_secs(Err(SymErr::SeekError(symphonia::core::errors::SeekErrorKind::Unseekable)), tb, 42.0), 0.0);
        let s = SeekedTo { track_id: 0, required_ts: 44_100 * 42, actual_ts: 44_100 * 40 };
        assert!((seeked_secs(Ok(s), tb, 42.0) - 40.0).abs() < 1e-9);
        let s = SeekedTo { track_id: 0, required_ts: 1, actual_ts: 1 };
        assert_eq!(seeked_secs(Ok(s), None, 42.0), 42.0);
        assert_eq!(Session::new().from(), None);
    }

    /// WAV PCM de 16 bits en memoria: medio segundo de silencio y medio de tono fuerte.
    fn wav(rate: u32, secs: f32) -> Vec<u8> {
        let n = (rate as f32 * secs) as u32;
        let mut v = Vec::new();
        v.extend_from_slice(b"RIFF");
        v.extend_from_slice(&(36 + n * 2).to_le_bytes());
        v.extend_from_slice(b"WAVEfmt ");
        v.extend_from_slice(&16u32.to_le_bytes());
        v.extend_from_slice(&1u16.to_le_bytes()); // PCM
        v.extend_from_slice(&1u16.to_le_bytes()); // mono
        v.extend_from_slice(&rate.to_le_bytes());
        v.extend_from_slice(&(rate * 2).to_le_bytes());
        v.extend_from_slice(&2u16.to_le_bytes());
        v.extend_from_slice(&16u16.to_le_bytes());
        v.extend_from_slice(b"data");
        v.extend_from_slice(&(n * 2).to_le_bytes());
        for i in 0..n {
            let s: i16 = if i < n / 2 { 0 } else if (i / 20) % 2 == 0 { 20000 } else { -20000 };
            v.extend_from_slice(&s.to_le_bytes());
        }
        v
    }

    #[test]
    fn probe_reads_duration_and_waveform_from_bytes() {
        let b = Bytes::Owned(Arc::from(wav(8000, 1.0)));
        let (d, peaks) = probe(&b, "wav", 10).unwrap();
        assert!((d - 1.0).abs() < 0.01, "{d}");
        assert_eq!(peaks.len(), 10);
        assert!(peaks[..4].iter().all(|p| *p < 10), "{peaks:?}");
        assert!(peaks[6..].iter().all(|p| *p > 150), "{peaks:?}");
        // basura: falla sin colgarse ni entrar en pánico
        let junk = Bytes::Owned(Arc::from(&b"no es audio"[..]));
        assert!(open_format(&junk, "flac").is_err());
        assert!(read_meta(&junk, "flac").title.is_empty());
    }
}
