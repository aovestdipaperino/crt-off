//! Renders the effect as raw RGB24 frames on stdout, for previewing it as a video:
//! the picture switches off, stays black a moment, then warms up again.
//!
//! ```sh
//! cargo run --release --example video -- logo.png 640 480 60 |
//!     ffmpeg -f rawvideo -pix_fmt rgb24 -s 640x480 -r 60 -i - -pix_fmt yuv420p crt.mp4
//! ```

use std::io::{BufWriter, Write};

fn main() -> std::io::Result<()> {
    let mut args = std::env::args().skip(1);
    let path = args
        .next()
        .expect("usage: video PICTURE [WIDTH HEIGHT FPS]");
    let mut number = |default: u32| args.next().and_then(|a| a.parse().ok()).unwrap_or(default);
    let (width, height, fps) = (number(640), number(480), number(60));

    // The picture centred on black, as a window would show it.
    let logo = image::open(path).expect("readable picture").to_rgba8();
    let mut picture = image::RgbaImage::from_pixel(width, height, image::Rgba([0, 0, 0, 255]));
    let left = i64::from(width.saturating_sub(logo.width()) / 2);
    let top = i64::from(height.saturating_sub(logo.height()) / 2);
    image::imageops::overlay(&mut picture, &logo, left, top);
    for p in picture.pixels_mut() {
        let a = u16::from(p[3]);
        for c in 0..3 {
            p[c] = (u16::from(p[c]) * a / 255) as u8;
        }
        p[3] = 255;
    }

    let config = crt_off::Config::default();
    let fps = f64::from(fps);
    let frames = (config.duration() * fps).ceil() as u32;
    let pause = (0.5 * fps) as u32;
    let mut out = BufWriter::new(std::io::stdout().lock());
    let mut emit = |t: f64| -> std::io::Result<()> {
        let frame = crt_off::render_frame(&picture, &config, t);
        for p in frame.pixels() {
            out.write_all(&p.0[..3])?;
        }
        Ok(())
    };
    let ts = (0..pause)
        .map(|_| 0.0)
        .chain((0..=frames).map(|f| f64::from(f) / fps))
        .chain((0..pause).map(|_| config.duration()))
        .chain((0..=frames).rev().map(|f| f64::from(f) / fps))
        .chain((0..pause).map(|_| 0.0));
    for t in ts {
        emit(t)?;
    }
    Ok(())
}
