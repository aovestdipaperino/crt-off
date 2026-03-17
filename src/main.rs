// Rust guideline compliant October 17th 2025

use std::env;
use std::fs;
use std::io;

use crossterm::terminal;
use crt_off::{Config, ScreenBuffer};
use image::imageops::FilterType;

fn main() -> io::Result<()> {
    let args: Vec<String> = env::args().collect();
    let (term_w, term_h) = terminal::size()?;
    let config = Config::default();

    if args.len() >= 2 && !args[1].starts_with('-') {
        // --- PNG mode ---
        let image_data = fs::read(&args[1]).expect("Failed to read image file");

        let width = args
            .get(2)
            .and_then(|s| s.parse::<u32>().ok())
            .unwrap_or(term_w as u32)
            .min(term_w as u32)
            .max(1);

        // Use logo-art to determine the output row count for a given width
        let term_rows = logo_art::image_to_ansi(&image_data, width).lines().count() as u32;
        if term_rows == 0 {
            eprintln!("Image produced no output. Try a larger width.");
            std::process::exit(1);
        }
        let pixel_h = term_rows * 2;

        let img = image::load_from_memory(&image_data).expect("Failed to decode image");
        let resized = img.resize_exact(width, pixel_h, FilterType::Lanczos3);

        crt_off::animate(&resized.to_rgba8(), true, &config)
    } else {
        // --- Screen mode: print demo content, then CRT-off ---
        let mut screen = ScreenBuffer::new(term_w as u32, term_h as u32);
        print_demo_content(&mut screen);
        screen.animate(&config)
    }
}

fn print_demo_content(s: &mut ScreenBuffer) {
    let w = s.width() as usize;
    let box_w = w.min(58);
    let pad = "  ";

    // Title box
    s.fg(0, 200, 200).put(pad).put("╔");
    s.put(&"═".repeat(box_w - 2));
    s.putln("╗");

    s.fg(0, 200, 200).put(pad).put("║");
    let title = "CRT TERMINAL EMULATOR v4.2.1";
    let lpad = (box_w - 2 - title.len()) / 2;
    s.fg(255, 255, 255)
        .put(&" ".repeat(lpad))
        .put(title)
        .put(&" ".repeat(box_w - 2 - lpad - title.len()));
    s.fg(0, 200, 200).putln("║");

    s.fg(0, 200, 200).put(pad).put("╚");
    s.put(&"═".repeat(box_w - 2));
    s.putln("╝");

    s.putln("");

    // Boot sequence
    let services = [
        ("Display subsystem", true),
        ("Font renderer", true),
        ("Color palette", true),
        ("Network daemon", true),
        ("Filesystem mounts", true),
        ("Diagnostics", false),
        ("Thermal sensors", true),
        ("User profile", true),
        ("Shell environment", true),
    ];
    for (name, ok) in &services {
        s.fg(100, 100, 100).put("  > ");
        s.fg(200, 200, 200).put(name);
        let dots = 38_usize.saturating_sub(name.len());
        s.fg(60, 60, 60).put(&".".repeat(dots));
        if *ok {
            s.fg(0, 255, 100).putln("  [OK]");
        } else {
            s.fg(255, 255, 0).putln("  [WARN]");
        }
    }

    s.putln("");

    // Section header
    s.fg(255, 180, 0)
        .putln("  ── System Status ──────────────────────────────");
    s.putln("");

    // Progress bars
    let bars: &[(&str, u32, [u8; 3])] = &[
        ("CPU ", 72, [0, 220, 120]),
        ("MEM ", 61, [0, 150, 255]),
        ("DISK", 38, [180, 100, 255]),
        ("NET ", 99, [255, 80, 80]),
    ];
    let bar_len = 20_u32;
    for (label, pct, color) in bars {
        s.fg(180, 180, 180).put("  ").put(label).put(":  ");
        let filled = (bar_len * pct / 100).min(bar_len);
        s.fg(color[0], color[1], color[2])
            .put(&"█".repeat(filled as usize));
        s.fg(50, 50, 50)
            .put(&"░".repeat((bar_len - filled) as usize));
        let pct_str = format!("  {pct:>3}%");
        s.fg(255, 255, 255).putln(&pct_str);
    }

    s.putln("");

    // Footer
    s.fg(100, 100, 100)
        .putln("  9 services active │ uptime: 47d 12h 33m");
    s.putln("");

    // Prompt
    s.fg(0, 255, 100).put("  user@crt-terminal ~ $ ");
    s.fg(200, 200, 200).putln("█");
}
