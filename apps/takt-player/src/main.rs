// SPDX-License-Identifier: GPL-3.0-or-later
//! Takt Player — Phase 0.
//!
//! Derzeit ohne Fenster: der Player fuehrt eine kopflose Wiedergabeschleife
//! ueber [`NullBackend`] aus und gibt die Messwerte des Schedulers aus. Das
//! ist absichtlich der erste Schritt — die Zeit- und Scheduling-Mechanik ist
//! damit auf allen drei Plattformen pruefbar, bevor eine Fensterbibliothek
//! oder libmpv im Spiel ist.
//!
//! Naechster Schritt: egui-Fenster, dann `MpvBackend`.

// Dieses Modul formatiert ausschliesslich Messwerte fuer die Ausgabe. Zeit
// wird hier nie gerechnet, nur angezeigt — deshalb ist Gleitkomma erlaubt.
#![allow(clippy::cast_precision_loss)]

use std::sync::Arc;
use std::time::Duration;

use takt_backend::{NullBackend, Pattern, Presentation, Scheduler, VideoBackend};
use takt_clock::{AudioMasterClock, Clock, MonotonicSource, SystemMonotonic};
use takt_core::{
    ColorPrimaries, ColorRange, FrameMeta, PixelFormat, Rational, Timestamp, TransferFunction,
};

/// Wie lange die Demo-Wiedergabe laeuft.
const DEMO_SECONDS: i64 = 3;
/// Bildwiederholrate der Testquelle: die NTSC-Rate, weil sie am ehesten
/// Rundungsfehler aufdeckt.
const FPS: (i64, i64) = (24_000, 1_001);

fn main() {
    let fps = Rational::lit(FPS.0, FPS.1);
    let duration = Timestamp::from_millis(DEMO_SECONDS * 1_000);

    let meta = FrameMeta {
        width: 1920,
        height: 1080,
        format: PixelFormat::Nv12,
        primaries: ColorPrimaries::Bt709,
        transfer: TransferFunction::Bt709,
        range: ColorRange::Limited,
        pts: Timestamp::from_millis(0),
    };

    let source = Arc::new(SystemMonotonic::new());
    // 20 ms Extrapolationsgrenze, passend zu einer 10-ms-Callback-Periode.
    let clock = AudioMasterClock::new(
        48_000,
        Arc::clone(&source) as Arc<dyn MonotonicSource>,
        20_000_000,
    );
    let writer = clock.writer();

    let mut backend = NullBackend::new(meta, fps, duration, Pattern::Motion, 6);
    let info = match backend.open("null://motion") {
        Ok(info) => info,
        Err(e) => {
            eprintln!("Quelle konnte nicht geoeffnet werden: {e}");
            std::process::exit(1);
        }
    };

    println!("Takt Player — Phase 0 (kopflos)");
    println!("  Backend      {}", backend.name());
    println!(
        "  Geometrie    {}x{} {:?}",
        meta.width, meta.height, meta.format
    );
    println!("  Rate         {fps} fps");
    println!(
        "  Framedauer   {:.3} ms",
        info.frame_duration_nanos().unwrap_or(0) as f64 / 1e6
    );
    println!("  Frames       {}", backend.total_frames());
    println!();

    // Der Audio-Thread wird hier simuliert: alle 10 ms meldet er seine
    // Position, genau wie ein echter WASAPI- oder PipeWire-Callback.
    let audio = std::thread::spawn(move || {
        let period = Duration::from_millis(10);
        let samples_per_period = 480_u64; // 10 ms bei 48 kHz
        let total_periods = u64::try_from(DEMO_SECONDS).unwrap_or(3) * 100;
        let start = SystemMonotonic::new();
        for period_index in 0..=total_periods {
            writer.report(period_index * samples_per_period, start.now_nanos());
            std::thread::sleep(period);
        }
    });

    let mut scheduler = Scheduler::new();
    let frame_ns = info.frame_duration_nanos().unwrap_or(16_666_667);

    // Anzeigedurchlauf bei 60 Hz, so wie ein echter Renderer aufgerufen wuerde.
    let vsync = Duration::from_nanos(16_666_667);
    loop {
        let frame = match backend.next_frame(&clock) {
            Ok(f) => f,
            Err(e) => {
                eprintln!("Dekodierfehler: {e}");
                break;
            }
        };
        match scheduler.accept(&clock, frame.as_ref(), backend.is_eof()) {
            Presentation::Eof => break,
            Presentation::Shown { .. } | Presentation::Repeated => {}
        }
        drop(frame); // Frame zurueck in den Pool
        std::thread::sleep(vsync);
    }

    let _ = audio.join();

    let s = scheduler.stats();
    let b = backend.stats();
    println!("Messwerte");
    println!("  dargestellt     {}", s.shown);
    println!(
        "  wiederholt      {} ({:.1} %)",
        s.repeated,
        s.repeat_ratio()
    );
    println!("  verworfen       {}", b.dropped);
    println!(
        "  Fehler Mittel   {:.3} ms",
        s.mean_abs_error_nanos() as f64 / 1e6
    );
    println!(
        "  Fehler max spaet {:.3} ms",
        s.worst_late_nanos as f64 / 1e6
    );
    println!(
        "  Fehler max frueh {:.3} ms",
        s.worst_early_nanos as f64 / 1e6
    );
    println!("  Endposition     {}", clock.now());
    println!("  Framedauer      {:.3} ms", frame_ns as f64 / 1e6);
    println!();
    println!("Histogramm der Fehlerbetraege");
    for (i, count) in s.histogram.iter().enumerate() {
        if *count == 0 {
            continue;
        }
        let lo = if i == 0 { 0 } else { 1_u32 << i };
        let hi = 1_u32 << (i + 1);
        println!(
            "  {lo:>6}..{hi:<6} us  {:>5}  {}",
            count,
            "#".repeat((*count as usize).min(40))
        );
    }
}
