# crt-off

[![Crates.io](https://img.shields.io/crates/v/crt-off.svg)](https://crates.io/crates/crt-off)
[![Docs.rs](https://docs.rs/crt-off/badge.svg)](https://docs.rs/crt-off)
[![License](https://img.shields.io/crates/l/crt-off.svg)](LICENSE)

CRT screen-off animation for the terminal.

Renders a retro CRT power-off effect inspired by Android's
[ElectronBeam](https://android.googlesource.com/platform/frameworks/base/+/refs/heads/master/services/core/java/com/android/server/display/ElectronBeam.java):
the image collapses vertically with RGB chromatic aberration, shrinks to a
bright horizontal scanline, then fades to a single phosphor dot.

## Installation

### CLI

```sh
cargo install crt-off
```

### Library

```toml
[dependencies]
crt-off = { version = "0.1", default-features = false }
```

## CLI Usage

Animate a PNG image:

```sh
crt-off logo.png        # full terminal width
crt-off logo.png 60     # 60 columns wide
```

Run the built-in demo (prints colourful text then animates CRT-off over it):

```sh
crt-off
```

Press **q**, **Esc**, or **Ctrl+C** to quit early.

## Library Usage

### Animate terminal content

```rust
use crt_off::{ScreenBuffer, Config};
use crossterm::terminal;

let (w, h) = terminal::size().unwrap();
let mut screen = ScreenBuffer::new(w as u32, h as u32);
screen.fg(0, 255, 100).putln("Hello, retro world!");
screen.animate(&Config::default()).unwrap();
```

`ScreenBuffer` prints to stdout while simultaneously recording every
character as pixel data. Calling `.animate()` then plays the CRT-off
effect directly over the printed content.

### Animate an image

```rust
use crt_off::Config;

let img = image::open("logo.png").unwrap();
let width = 60u32;
let (ow, oh) = image::GenericImageView::dimensions(&img);
let pixel_h = ((width as f64 / ow as f64 * oh as f64).round() as u32 + 1) & !1;
let resized = img.resize_exact(width, pixel_h, image::imageops::FilterType::Lanczos3);
crt_off::animate(&resized.to_rgba8(), true, &Config::default()).unwrap();
```

### Custom timing

```rust
use crt_off::Config;

let config = Config {
    hold_secs: 2.0,       // pause before animation
    vstretch_secs: 1.2,   // vertical collapse duration
    hstretch_secs: 0.8,   // scanline shrink duration
    dot_fade_secs: 0.6,   // phosphor dot fade duration
    black_secs: 0.5,      // final black screen
    fps: 60.0,
    ..Config::default()
};
```

## How it works

The animation has three phases, ported from Android's `ElectronBeam.drawVStretch` / `drawHStretch`:

1. **Vertical collapse** -- The image compresses toward the horizontal centre. Each RGB channel collapses at a slightly different rate (`scurve` with steepness 7.5 / 8.0 / 8.5), producing chromatic aberration fringes. An additive white glow intensifies in the tightest channel region.

2. **Horizontal scanline** -- A bright white line at the vertical centre shrinks from full width to zero while dimming.

3. **Phosphor dot** -- A single bright pixel at the centre fades to black.

Rendering uses Unicode half-block characters (`U+2580`) with 24-bit true-colour ANSI escapes, giving two vertical pixels per terminal cell.

## License

Apache-2.0 -- see [LICENSE](LICENSE) for details.
