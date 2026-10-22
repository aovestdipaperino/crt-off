// Rust guideline compliant October 17th 2025

//! CRT screen-off animation for the terminal.
//!
//! Renders a retro CRT power-off effect inspired by Android's `ElectronBeam`:
//! the image collapses vertically with RGB chromatic aberration, shrinks to a
//! bright horizontal scanline, then fades to a single phosphor dot.
//!
//! The terminal animation is behind the default `terminal` feature. Without it,
//! [`render_frame`] still renders any moment of the effect as pixels, for a
//! window or a video; played backwards, it is a CRT warming up.
//!
//! # Animate terminal content
//!
//! ```no_run
//! # #[cfg(feature = "terminal")] {
//! use crt_off::{ScreenBuffer, Config};
//! use crossterm::terminal;
//!
//! let (w, h) = terminal::size().unwrap();
//! let mut screen = ScreenBuffer::new(w as u32, h as u32);
//! screen.fg(0, 255, 100).putln("Hello, retro world!");
//! screen.animate(&Config::default()).unwrap();
//! # }
//! ```
//!
//! # Animate an image
//!
//! ```no_run
//! # #[cfg(feature = "terminal")] {
//! use crt_off::Config;
//!
//! let img = image::open("logo.png").unwrap();
//! let width = 60u32;
//! let (ow, oh) = image::GenericImageView::dimensions(&img);
//! let pixel_h = ((width as f64 / ow as f64 * oh as f64).round() as u32 + 1) & !1;
//! let resized = img.resize_exact(
//!     width,
//!     pixel_h,
//!     image::imageops::FilterType::Lanczos3,
//! );
//! crt_off::animate(&resized.to_rgba8(), true, &Config::default()).unwrap();
//! # }
//! ```
//!
//! # Render frames as pixels
//!
//! ```
//! use crt_off::Config;
//!
//! let logo = image::RgbaImage::from_pixel(64, 48, image::Rgba([0, 255, 100, 255]));
//! let config = Config::default();
//! // A CRT warming up: the power-off effect, played backwards.
//! let fps = 60.0;
//! let frames = (config.duration() * fps) as u32;
//! for frame in (0..=frames).rev() {
//!     let image = crt_off::render_frame(&logo, &config, f64::from(frame) / fps);
//!     assert_eq!(image.dimensions(), (64, 48));
//! }
//! ```

#[cfg(feature = "terminal")]
use std::fmt::Write as _;
#[cfg(feature = "terminal")]
use std::io::{self, BufWriter, Write, stdout};
#[cfg(feature = "terminal")]
use std::thread;
#[cfg(feature = "terminal")]
use std::time::{Duration, Instant};

#[cfg(feature = "terminal")]
use crossterm::{cursor, event, execute, terminal};

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Animation timing configuration.
pub struct Config {
    /// Seconds to hold the static image before animating.
    pub hold_secs: f64,
    /// Duration of the vertical collapse phase.
    pub vstretch_secs: f64,
    /// Duration of the horizontal scanline phase.
    pub hstretch_secs: f64,
    /// Duration of the phosphor dot fade phase.
    pub dot_fade_secs: f64,
    /// Duration of the final black screen.
    pub black_secs: f64,
    /// Frames per second.
    pub fps: f64,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            hold_secs: 1.5,
            vstretch_secs: 0.9,
            hstretch_secs: 0.7,
            dot_fade_secs: 0.5,
            black_secs: 0.4,
            fps: 30.0,
        }
    }
}

impl Config {
    /// Seconds from the start of the collapse to the end of the dot fade: the
    /// span [`render_frame`] covers, without the hold before it or the black
    /// screen after it.
    pub fn duration(&self) -> f64 {
        self.vstretch_secs + self.hstretch_secs + self.dot_fade_secs
    }
}

/// Renders the effect `t` seconds into the collapse, as an image the size of
/// `img`: `t = 0` is `img` itself, [`Config::duration`] and later is black.
///
/// The phases are those of the terminal animation, drawn in pixels: the image
/// squeezes to the centre row with each colour on its own curve and a rising
/// glow, the bright line it becomes shrinks to the centre, and a phosphor dot
/// fades out there.
pub fn render_frame(img: &image::RgbaImage, config: &Config, t: f64) -> image::RgbaImage {
    let (w, h) = img.dimensions();
    let mut out = image::RgbaImage::from_pixel(w, h, image::Rgba([0, 0, 0, 255]));
    if w == 0 || h == 0 {
        return out;
    }
    if t <= 0.0 {
        return img.clone();
    }
    if t < config.vstretch_secs {
        pixels_vstretch(&mut out, img, (t / config.vstretch_secs) as f32);
    } else if t < config.vstretch_secs + config.hstretch_secs {
        let s = (t - config.vstretch_secs) / config.hstretch_secs;
        pixels_hstretch(&mut out, s as f32);
    } else if t < config.duration() {
        let s = (t - config.vstretch_secs - config.hstretch_secs) / config.dot_fade_secs;
        pixels_dot(&mut out, s as f32);
    }
    out
}

/// Width, in pixels, of the beam's core: the scanline is a line one pixel thick
/// and the dot a point, however large the image.
const BEAM_CORE: f32 = 0.55;

/// Width, in pixels, and strength of the faint glow around the beam's core.
const BEAM_HALO: f32 = 2.0;
const BEAM_HALO_LEVEL: f32 = 0.18;

/// Colour of the glow, a little bluer than the white core.
const BEAM_HALO_TINT: [f32; 3] = [0.75, 0.85, 1.0];

/// How bright the beam is `d` pixels from its centre, as a white core and a
/// tinted glow, each between `0.0` and `1.0`.
fn beam_profile(d: f32) -> (f32, f32) {
    let core = (-(d / BEAM_CORE).powi(2)).exp();
    let halo = BEAM_HALO_LEVEL * (-(d / BEAM_HALO).powi(2)).exp();
    (core, halo)
}

/// Lights `(x, y)` with a beam whose core is `core` and whose glow is `halo`,
/// both already scaled by its brightness.
fn put_beam(out: &mut image::RgbaImage, x: u32, y: u32, core: f32, halo: f32) {
    let px = |c: usize| {
        ((core + halo * BEAM_HALO_TINT[c]) * 255.0)
            .round()
            .clamp(0.0, 255.0) as u8
    };
    out.put_pixel(x, y, image::Rgba([px(0), px(1), px(2), 255]));
}

/// The rows and columns the beam's glow reaches either side of its centre.
fn beam_reach() -> u32 {
    (BEAM_HALO * 3.0).ceil() as u32
}

fn pixels_vstretch(out: &mut image::RgbaImage, img: &image::RgbaImage, t: f32) {
    let ar = scurve(t, 7.5);
    let ag = scurve(t, 8.0);
    let ab = scurve(t, 8.5);
    let h = img.height() as f32;
    let comp = [h * (1.0 - ar), h * (1.0 - ag), h * (1.0 - ab)];
    let ys = [
        (h - comp[0]) / 2.0,
        (h - comp[1]) / 2.0,
        (h - comp[2]) / 2.0,
    ];
    let glow = ag * 180.0;
    for y in 0..img.height() {
        for x in 0..img.width() {
            let (r, g, b) = sample(img, x, y as f32, h, &comp, &ys, glow);
            out.put_pixel(x, y, image::Rgba([r, g, b, 255]));
        }
    }
}

fn pixels_hstretch(out: &mut image::RgbaImage, t: f32) {
    let (w, h) = out.dimensions();
    let a = scurve(t, 8.0);
    let half = (w as f32 * (1.0 - a)).min(w as f32 / 2.0);
    if half <= 0.0 {
        return;
    }
    let level = (1.0 - a * 0.75).max(0.0);
    // The line's ends fade over a stretch of it rather than stopping square.
    let taper = 0.15 * half + 1.0;
    let (cx, cy) = (w as f32 / 2.0, (h / 2) as f32 + 0.5);
    let reach = beam_reach();
    let x0 = (cx - half).floor().max(0.0) as u32;
    let x1 = ((cx + half).ceil() as u32).min(w);
    for y in (h / 2).saturating_sub(reach)..(h / 2 + reach + 1).min(h) {
        let (core, halo) = beam_profile(y as f32 + 0.5 - cy);
        for x in x0..x1 {
            let along = half - (x as f32 + 0.5 - cx).abs();
            let fade = smoothstep((along / taper).clamp(0.0, 1.0)) * level;
            put_beam(out, x, y, core * fade, halo * fade);
        }
    }
}

fn pixels_dot(out: &mut image::RgbaImage, t: f32) {
    let (w, h) = out.dimensions();
    let level = f32::from(phosphor_level(t)) / 255.0;
    let (cx, cy) = ((w / 2) as f32 + 0.5, (h / 2) as f32 + 0.5);
    let reach = beam_reach();
    for y in (h / 2).saturating_sub(reach)..(h / 2 + reach + 1).min(h) {
        for x in (w / 2).saturating_sub(reach)..(w / 2 + reach + 1).min(w) {
            let d = (x as f32 + 0.5 - cx).hypot(y as f32 + 0.5 - cy);
            let (core, halo) = beam_profile(d);
            // The dot is a little larger than the line's core, so it reads as a point.
            let core = core.max((-(d / (BEAM_CORE * 1.6)).powi(2)).exp());
            put_beam(out, x, y, core * level, halo * level);
        }
    }
}

fn smoothstep(x: f32) -> f32 {
    x * x * (3.0 - 2.0 * x)
}

#[cfg(feature = "terminal")]
/// Run the CRT-off animation on an RGBA image.
///
/// The image height must be even (each terminal row encodes two pixel rows
/// via half-block characters). Width and row count are derived from the image
/// dimensions.
///
/// When `use_alt_screen` is `true` the animation runs in an alternate terminal
/// buffer and displays the static image for [`Config::hold_secs`] before the
/// collapse begins. When `false` the caller is expected to have already printed
/// content to the terminal; the function sleeps for the hold duration (so the
/// content remains visible) and then animates directly over it.
pub fn animate(rgba: &image::RgbaImage, use_alt_screen: bool, config: &Config) -> io::Result<()> {
    let width = rgba.width();
    let pixel_h = rgba.height();
    let term_rows = pixel_h / 2;
    debug_assert!(pixel_h.is_multiple_of(2), "image height must be even");

    let mut out = BufWriter::new(stdout());

    if use_alt_screen {
        execute!(out, terminal::EnterAlternateScreen, cursor::Hide)?;
        terminal::enable_raw_mode()?;
    } else {
        thread::sleep(Duration::from_secs_f64(config.hold_secs));
        execute!(out, cursor::Hide)?;
        terminal::enable_raw_mode()?;
        // Clear screen so stale text below content area doesn't leak through
        write!(out, "\x1b[2J")?;
    }
    let _guard = TermGuard {
        alternate_screen: use_alt_screen,
    };

    let hold_offset = if use_alt_screen {
        config.hold_secs
    } else {
        0.0
    };
    let total = hold_offset + config.vstretch_secs + config.hstretch_secs + config.dot_fade_secs;
    let start = Instant::now();
    let frame_dt = Duration::from_secs_f64(1.0 / config.fps);
    let mut next_frame = start;

    loop {
        if event::poll(Duration::ZERO).unwrap_or(false)
            && let Ok(event::Event::Key(k)) = event::read()
        {
            match k.code {
                event::KeyCode::Char('c') if k.modifiers.contains(event::KeyModifiers::CONTROL) => {
                    break;
                }
                event::KeyCode::Char('q') | event::KeyCode::Esc => break,
                _ => {}
            }
        }

        let t = start.elapsed().as_secs_f64();
        if t >= total {
            break;
        }

        let mut buf = String::with_capacity(width as usize * term_rows as usize * 30);
        buf.push_str("\x1b[H");

        let anim_t = t - hold_offset;
        if anim_t < 0.0 {
            frame_vstretch(&mut buf, rgba, width, pixel_h, term_rows, 0.0);
        } else if anim_t < config.vstretch_secs {
            let s = (anim_t / config.vstretch_secs) as f32;
            frame_vstretch(&mut buf, rgba, width, pixel_h, term_rows, s);
        } else if anim_t < config.vstretch_secs + config.hstretch_secs {
            let s = ((anim_t - config.vstretch_secs) / config.hstretch_secs) as f32;
            frame_hstretch(&mut buf, width, term_rows, s);
        } else {
            let s = ((anim_t - config.vstretch_secs - config.hstretch_secs) / config.dot_fade_secs)
                as f32;
            frame_dot(&mut buf, width, term_rows, s);
        }

        out.write_all(buf.as_bytes())?;
        out.flush()?;

        next_frame += frame_dt;
        if let Some(wait) = next_frame.checked_duration_since(Instant::now()) {
            thread::sleep(wait);
        }
    }

    write_black_screen(&mut out, width, term_rows)?;
    thread::sleep(Duration::from_secs_f64(config.black_secs));

    Ok(())
}

#[cfg(feature = "terminal")]
/// Buffer that prints to stdout while recording pixel data for animation.
///
/// Each printed character is simultaneously recorded into an internal
/// [`image::RgbaImage`]. Call [`ScreenBuffer::animate`] to run the CRT-off
/// effect on the captured content, or [`ScreenBuffer::into_image`] to extract
/// the raw image for custom use.
pub struct ScreenBuffer {
    width: u32,
    height: u32,
    img: image::RgbaImage,
    col: u32,
    row: u32,
    fg: [u8; 3],
}

#[cfg(feature = "terminal")]
impl ScreenBuffer {
    /// Create a new buffer for a terminal of the given `width` and `height`.
    pub fn new(width: u32, height: u32) -> Self {
        Self {
            width,
            height,
            img: image::RgbaImage::from_pixel(width, height * 2, image::Rgba([0, 0, 0, 255])),
            col: 0,
            row: 0,
            fg: [255, 255, 255],
        }
    }

    /// Set the foreground colour for subsequent text.
    pub fn fg(&mut self, r: u8, g: u8, b: u8) -> &mut Self {
        self.fg = [r, g, b];
        self
    }

    /// Print `s` to stdout and record its pixels.
    pub fn put(&mut self, s: &str) -> &mut Self {
        print!("\x1b[38;2;{};{};{}m", self.fg[0], self.fg[1], self.fg[2]);
        for ch in s.chars() {
            if ch == '\n' {
                print!("\x1b[0m\r\n");
                self.col = 0;
                self.row += 1;
                continue;
            }
            print!("{ch}");
            if ch != ' ' && self.col < self.width && self.row < self.height {
                let pixel = image::Rgba([self.fg[0], self.fg[1], self.fg[2], 255]);
                self.img.put_pixel(self.col, self.row * 2, pixel);
                self.img.put_pixel(self.col, self.row * 2 + 1, pixel);
            }
            self.col += 1;
        }
        self
    }

    /// Print `s` followed by a newline.
    pub fn putln(&mut self, s: &str) -> &mut Self {
        self.put(s);
        self.put("\n")
    }

    /// Number of terminal rows that contain printed content.
    pub fn content_rows(&self) -> u32 {
        (if self.col > 0 { self.row + 1 } else { self.row })
            .max(1)
            .min(self.height)
    }

    /// Terminal width of this buffer.
    pub fn width(&self) -> u32 {
        self.width
    }

    /// Extract the recorded image, cropped to the content rows.
    pub fn into_image(self) -> image::RgbaImage {
        let rows = self.content_rows();
        let pixel_h = rows * 2;
        let mut rgba = image::RgbaImage::new(self.width, pixel_h);
        for y in 0..pixel_h {
            for x in 0..self.width {
                *rgba.get_pixel_mut(x, y) = *self.img.get_pixel(x, y);
            }
        }
        rgba
    }

    /// Flush stdout, extract the image, and run the CRT-off animation.
    ///
    /// This is a convenience wrapper around [`into_image`](Self::into_image)
    /// and [`animate`].
    pub fn animate(self, config: &Config) -> io::Result<()> {
        print!("\x1b[0m");
        io::stdout().flush()?;
        let rgba = self.into_image();
        crate::animate(&rgba, false, config)
    }
}

// ---------------------------------------------------------------------------
// Terminal cleanup guard
// ---------------------------------------------------------------------------

#[cfg(feature = "terminal")]
struct TermGuard {
    alternate_screen: bool,
}

#[cfg(feature = "terminal")]
impl Drop for TermGuard {
    fn drop(&mut self) {
        let _ = terminal::disable_raw_mode();
        if self.alternate_screen {
            let _ = execute!(io::stdout(), cursor::Show, terminal::LeaveAlternateScreen);
        } else {
            let _ = execute!(io::stdout(), cursor::Show);
        }
    }
}

// ---------------------------------------------------------------------------
// Animation easing (ported from ElectronBeam.java)
// ---------------------------------------------------------------------------

fn sigmoid(x: f32, s: f32) -> f32 {
    1.0 / (1.0 + (-x * s).exp())
}

fn scurve(value: f32, s: f32) -> f32 {
    let x = value - 0.5;
    let y = sigmoid(x, s) - 0.5;
    let v = sigmoid(0.5, s) - 0.5;
    y / v * 0.5 + 0.5
}

// ---------------------------------------------------------------------------
// Frame builders
// ---------------------------------------------------------------------------

#[cfg(feature = "terminal")]
fn frame_vstretch(
    buf: &mut String,
    img: &image::RgbaImage,
    w: u32,
    pixel_h: u32,
    rows: u32,
    t: f32,
) {
    let ar = scurve(t, 7.5);
    let ag = scurve(t, 8.0);
    let ab = scurve(t, 8.5);

    let h = pixel_h as f32;
    let comp = [h * (1.0 - ar), h * (1.0 - ag), h * (1.0 - ab)];
    let ys = [
        (h - comp[0]) / 2.0,
        (h - comp[1]) / 2.0,
        (h - comp[2]) / 2.0,
    ];
    let glow = ag * 180.0;

    for row in 0..rows {
        let yt = (row * 2) as f32;
        let yb = (row * 2 + 1) as f32;

        for x in 0..w {
            let top = sample(img, x, yt, h, &comp, &ys, glow);
            let bot = sample(img, x, yb, h, &comp, &ys, glow);
            write!(
                buf,
                "\x1b[38;2;{};{};{}m\x1b[48;2;{};{};{}m\u{2580}",
                top.0, top.1, top.2, bot.0, bot.1, bot.2,
            )
            .unwrap();
        }
        buf.push_str("\x1b[0m");
        if row < rows - 1 {
            buf.push_str("\r\n");
        }
    }
}

fn sample(
    img: &image::RgbaImage,
    x: u32,
    y: f32,
    h: f32,
    comp: &[f32; 3],
    ys: &[f32; 3],
    glow: f32,
) -> (u8, u8, u8) {
    let mut c = [0u8; 3];
    for ch in 0..3 {
        if comp[ch] >= 1.0 && y >= ys[ch] && y < ys[ch] + comp[ch] {
            let sy = ((y - ys[ch]) / comp[ch] * h).clamp(0.0, h - 1.0) as u32;
            c[ch] = img.get_pixel(x, sy.min(img.height() - 1))[ch];
        }
    }
    if comp[2] >= 1.0 && y >= ys[2] && y < ys[2] + comp[2] {
        (
            (c[0] as f32 + glow).min(255.0) as u8,
            (c[1] as f32 + glow).min(255.0) as u8,
            (c[2] as f32 + glow).min(255.0) as u8,
        )
    } else {
        (c[0], c[1], c[2])
    }
}

#[cfg(feature = "terminal")]
fn frame_hstretch(buf: &mut String, w: u32, rows: u32, t: f32) {
    let a = scurve(t, 8.0);
    let lw = ((2.0 * w as f32 * (1.0 - a)) as u32).min(w);
    let brightness = ((1.0 - a * 0.75) * 255.0).max(0.0) as u8;
    let cy = rows / 2;
    let x0 = w.saturating_sub(lw) / 2;

    for row in 0..rows {
        if row == cy && lw > 0 {
            for x in 0..w {
                if x >= x0 && x < x0 + lw {
                    write!(
                        buf,
                        "\x1b[38;2;{b};{b};{b}m\x1b[48;2;0;0;0m\u{2581}",
                        b = brightness,
                    )
                    .unwrap();
                } else {
                    buf.push_str("\x1b[48;2;0;0;0m ");
                }
            }
        } else {
            write!(buf, "\x1b[48;2;0;0;0m{}", " ".repeat(w as usize)).unwrap();
        }
        buf.push_str("\x1b[0m");
        if row < rows - 1 {
            buf.push_str("\r\n");
        }
    }
}

/// Display gamma. Phosphor *luminance* is encoded through it to get the code
/// value a terminal actually paints.
const GAMMA: f32 = 2.2;

/// Exponential decay constant of the phosphor across the fade window. A P22
/// phosphor decays exponentially; `e^-3` (~5% of peak) is the visible tail of
/// the afterglow, and the residue is subtracted off so the dot still lands on
/// true black.
const PHOSPHOR_DECAY: f32 = 3.0;

/// Gray code value of the phosphor dot at normalized fade time `t` (`0.0` at
/// the start of the fade, `1.0` at its end).
///
/// Luminance decays exponentially, the way phosphor persistence actually
/// behaves, offset so it lands on exact black at `t == 1.0` instead of clipping
/// early. The result is then gamma-encoded, because a linear ramp in code value
/// (the previous behavior) is *not* a linear ramp in perceived brightness — it
/// collapses to near-black within the first few frames and reads as the dot
/// blinking out rather than fading.
fn phosphor_level(t: f32) -> u8 {
    let t = t.clamp(0.0, 1.0);
    let tail = (-PHOSPHOR_DECAY).exp();
    let lum = (((-PHOSPHOR_DECAY * t).exp() - tail) / (1.0 - tail)).max(0.0);
    (lum.powf(1.0 / GAMMA) * 255.0).round().clamp(0.0, 255.0) as u8
}

#[cfg(feature = "terminal")]
fn frame_dot(buf: &mut String, w: u32, rows: u32, t: f32) {
    let b = phosphor_level(t);
    let cy = rows / 2;
    let cx = w / 2;

    for row in 0..rows {
        if row == cy && b > 0 {
            for x in 0..w {
                if x == cx {
                    write!(buf, "\x1b[1m\x1b[38;2;{b};{b};{b}m\x1b[48;2;0;0;0m.",).unwrap();
                } else {
                    buf.push_str("\x1b[48;2;0;0;0m ");
                }
            }
        } else {
            write!(buf, "\x1b[48;2;0;0;0m{}", " ".repeat(w as usize)).unwrap();
        }
        buf.push_str("\x1b[0m");
        if row < rows - 1 {
            buf.push_str("\r\n");
        }
    }
}

#[cfg(feature = "terminal")]
fn write_black_screen(out: &mut impl Write, w: u32, rows: u32) -> io::Result<()> {
    let mut buf = String::new();
    buf.push_str("\x1b[H");
    let blank = format!("\x1b[48;2;0;0;0m{}\x1b[0m", " ".repeat(w as usize));
    for row in 0..rows {
        buf.push_str(&blank);
        if row < rows - 1 {
            buf.push_str("\r\n");
        }
    }
    out.write_all(buf.as_bytes())?;
    out.flush()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_frame_at_the_start_is_the_image_and_at_the_end_is_black() {
        let img = image::RgbaImage::from_pixel(40, 30, image::Rgba([10, 200, 30, 255]));
        let config = Config::default();
        assert_eq!(render_frame(&img, &config, 0.0), img);
        let end = render_frame(&img, &config, config.duration());
        assert!(end.pixels().all(|p| p.0 == [0, 0, 0, 255]));
    }

    #[test]
    fn the_scanline_is_a_bright_line_across_the_middle() {
        let img = image::RgbaImage::from_pixel(80, 60, image::Rgba([255; 4]));
        let config = Config::default();
        let line = render_frame(&img, &config, config.vstretch_secs + 0.01);
        // One pixel thick in a 60-pixel image: row 30, with only a faint glow beside it.
        assert!(line.get_pixel(40, 30).0[0] > 200, "the middle is lit");
        assert!(
            line.get_pixel(40, 29).0[0] < 60,
            "the row above is only glow"
        );
        assert!(
            line.get_pixel(40, 31).0[0] < 60,
            "the row below is only glow"
        );
        assert_eq!(
            line.get_pixel(40, 5).0,
            [0, 0, 0, 255],
            "far above it is black"
        );
    }

    #[test]
    fn phosphor_fade_spans_full_range() {
        assert_eq!(phosphor_level(0.0), 255, "starts at peak brightness");
        assert_eq!(phosphor_level(1.0), 0, "reaches exact black at the end");
        // Out-of-range inputs clamp rather than wrap or overflow.
        assert_eq!(phosphor_level(-1.0), 255);
        assert_eq!(phosphor_level(2.0), 0);
    }

    #[test]
    fn phosphor_fade_is_monotone() {
        let mut prev = 255u8;
        for i in 0..=100 {
            let b = phosphor_level(i as f32 / 100.0);
            assert!(b <= prev, "brightness rose at t={i}: {prev} -> {b}");
            prev = b;
        }
    }

    #[test]
    fn phosphor_fade_holds_the_glow_past_the_midpoint() {
        // The point of gamma-encoding: at the halfway mark the dot is still
        // clearly visible instead of the ~50% code value (and near-black
        // perceived brightness) a linear ramp would give.
        let mid = phosphor_level(0.5);
        assert!(mid > 100, "midpoint too dim to read as a fade: {mid}");
        // ...and it is still on its way down, not stuck near peak.
        assert!(mid < 220, "midpoint suspiciously close to peak: {mid}");
    }
}
