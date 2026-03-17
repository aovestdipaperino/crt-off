// Rust guideline compliant October 17th 2025

//! CRT screen-off animation for the terminal.
//!
//! Renders a retro CRT power-off effect inspired by Android's `ElectronBeam`:
//! the image collapses vertically with RGB chromatic aberration, shrinks to a
//! bright horizontal scanline, then fades to a single phosphor dot.
//!
//! # Animate terminal content
//!
//! ```no_run
//! use crt_off::{ScreenBuffer, Config};
//! use crossterm::terminal;
//!
//! let (w, h) = terminal::size().unwrap();
//! let mut screen = ScreenBuffer::new(w as u32, h as u32);
//! screen.fg(0, 255, 100).putln("Hello, retro world!");
//! screen.animate(&Config::default()).unwrap();
//! ```
//!
//! # Animate an image
//!
//! ```no_run
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
//! ```

use std::fmt::Write as _;
use std::io::{self, BufWriter, Write, stdout};
use std::thread;
use std::time::{Duration, Instant};

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

struct TermGuard {
    alternate_screen: bool,
}

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
                        "\x1b[38;2;{b};{b};{b}m\x1b[48;2;0;0;0m\u{2580}",
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

fn frame_dot(buf: &mut String, w: u32, rows: u32, t: f32) {
    let b = ((1.0 - t) * 255.0).max(0.0) as u8;
    let cy = rows / 2;
    let cx = w / 2;

    for row in 0..rows {
        if row == cy && b > 0 {
            for x in 0..w {
                if x == cx {
                    write!(buf, "\x1b[38;2;{b};{b};{b}m\x1b[48;2;0;0;0m\u{2588}",).unwrap();
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
