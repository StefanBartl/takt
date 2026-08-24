// SPDX-License-Identifier: Apache-2.0 OR MIT
//! Der Ankerpunkt zwischen Audio-Thread und Render-Thread.
//!
//! # Das Problem
//!
//! Der Audio-Callback laeuft in groben Schritten: bei 480 Samples und 48 kHz
//! alle 10 ms. Der Render-Thread braucht die Wiedergabeposition aber
//! feinkoerniger, naemlich zu jedem Frame — bei 60 fps alle 16,6 ms, und zwar
//! zu einem beliebigen Zeitpunkt zwischen zwei Audio-Callbacks.
//!
//! Die Loesung ist ein *Anker*: der Audio-Thread hinterlegt „bei Systemzeit T
//! waren N Samples abgespielt". Der Render-Thread liest den Anker und
//! interpoliert mit der monotonen Systemuhr auf jetzt.
//!
//! # Warum ein Seqlock und kein Mutex
//!
//! Der Audio-Thread darf nicht blockieren — kein Lock, keine Allokation, kein
//! Warten. Ein `Mutex` verletzt das, auch wenn er meist frei ist: „meist" ist
//! bei einer harten Deadline von 2,7 ms keine Zusage.
//!
//! Zwei Werte (Samples und Zeitstempel) muessen aber gemeinsam konsistent
//! gelesen werden. Zwei unabhaengige Atomics erlauben einen zerrissenen Lesevorgang:
//! neue Sample-Zahl mit altem Zeitstempel, was einen Sprung in der
//! interpolierten Position ergibt.
//!
//! Ein **Seqlock** loest genau das:
//!
//! * Der Schreiber erhoeht einen Zaehler auf ungerade, schreibt, erhoeht auf
//!   gerade. Er blockiert nie und wartet nie.
//! * Der Leser merkt sich den Zaehler, liest, prueft den Zaehler erneut. Bei
//!   Abweichung oder ungeradem Wert versucht er es erneut.
//!
//! Der Schreiber haelt den ungeraden Zustand nur fuer zwei Speicherzugriffe,
//! ein Wiederholversuch des Lesers ist damit extrem selten und immer kurz.
//!
//! Der Aufbau kommt hier ohne `unsafe` aus, weil beide Nutzdaten selbst
//! `AtomicU64` sind — das Seqlock schuetzt nicht den Zugriff, sondern nur die
//! *Zusammengehoerigkeit* der beiden Werte.

use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::time::Instant;

/// Quelle einer monotonen Zeit in Nanosekunden.
///
/// Als Trait, damit Tests deterministisch laufen koennen, statt zu schlafen.
/// `Debug` ist Teil der Anforderung, damit die Uhren selbst `Debug` ableiten
/// koennen — bei der Fehlersuche in einer Zeitpipeline will man den Zustand
/// jederzeit ausgeben koennen.
pub trait MonotonicSource: Send + Sync + std::fmt::Debug {
    /// Nanosekunden seit einem festen, aber beliebigen Nullpunkt.
    fn now_nanos(&self) -> u64;
}

/// Die monotone Uhr des Betriebssystems.
#[derive(Debug)]
pub struct SystemMonotonic {
    origin: Instant,
}

impl SystemMonotonic {
    /// Legt den Nullpunkt auf jetzt.
    #[must_use]
    pub fn new() -> Self {
        Self {
            origin: Instant::now(),
        }
    }
}

impl Default for SystemMonotonic {
    fn default() -> Self {
        Self::new()
    }
}

impl MonotonicSource for SystemMonotonic {
    fn now_nanos(&self) -> u64 {
        // Instant ist per Definition monoton; ein Ueberlauf von u64
        // Nanosekunden entspricht 584 Jahren Laufzeit.
        u64::try_from(self.origin.elapsed().as_nanos()).unwrap_or(u64::MAX)
    }
}

/// Eine von Hand gestellte Uhr. Nur fuer Tests und Simulation.
#[derive(Debug, Default)]
pub struct ManualMonotonic {
    nanos: AtomicU64,
}

impl ManualMonotonic {
    /// Neue Uhr, stehend auf null.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            nanos: AtomicU64::new(0),
        }
    }

    /// Stellt die Uhr vor.
    pub fn advance(&self, nanos: u64) {
        self.nanos.fetch_add(nanos, Ordering::Relaxed);
    }

    /// Setzt die Uhr auf einen festen Wert.
    pub fn set(&self, nanos: u64) {
        self.nanos.store(nanos, Ordering::Relaxed);
    }
}

impl MonotonicSource for ManualMonotonic {
    fn now_nanos(&self) -> u64 {
        self.nanos.load(Ordering::Relaxed)
    }
}

/// Ein konsistent gelesener Ankerpunkt.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Anchor {
    /// Abgespielte Samples zum Zeitpunkt des Ankers.
    pub samples: u64,
    /// Monotone Systemzeit des Ankers in Nanosekunden.
    pub nanos: u64,
}

/// Seqlock-geschuetzter Ankerpunkt.
///
/// Schreiben ist wartefrei und fuer den Audio-Thread gedacht. Lesen kann in
/// seltenen Faellen wiederholen und ist fuer Render- und UI-Thread gedacht.
#[derive(Debug)]
pub struct AnchorCell {
    /// Gerade = konsistent, ungerade = wird gerade geschrieben.
    seq: AtomicU32,
    samples: AtomicU64,
    nanos: AtomicU64,
}

impl AnchorCell {
    /// Neue Zelle, noch ohne gueltigen Anker.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            seq: AtomicU32::new(0),
            samples: AtomicU64::new(0),
            nanos: AtomicU64::new(0),
        }
    }

    /// Hinterlegt einen neuen Anker.
    ///
    /// **Aufrufbar aus dem Audio-Thread.** Alloziert nicht, blockiert nicht,
    /// wartet nicht.
    pub fn publish(&self, anchor: Anchor) {
        let start = self.seq.load(Ordering::Relaxed);
        // Ungerade: Schreibvorgang laeuft.
        self.seq.store(start.wrapping_add(1), Ordering::Release);
        self.samples.store(anchor.samples, Ordering::Relaxed);
        self.nanos.store(anchor.nanos, Ordering::Relaxed);
        // Gerade: wieder konsistent.
        self.seq.store(start.wrapping_add(2), Ordering::Release);
    }

    /// Liest den Anker konsistent.
    ///
    /// Gibt `None` zurueck, solange noch nie ein Anker hinterlegt wurde, oder
    /// wenn der Schreiber nach `max_retries` Versuchen immer noch mitten im
    /// Schreiben ist — Letzteres darf im Betrieb nicht vorkommen und ist ein
    /// Hinweis auf einen unterbrochenen Audio-Thread.
    #[must_use]
    pub fn load(&self, max_retries: u32) -> Option<Anchor> {
        for _ in 0..max_retries {
            let before = self.seq.load(Ordering::Acquire);
            if before == 0 {
                return None; // noch nie geschrieben
            }
            if !before.is_multiple_of(2) {
                std::hint::spin_loop();
                continue; // Schreibvorgang laeuft
            }
            let samples = self.samples.load(Ordering::Relaxed);
            let nanos = self.nanos.load(Ordering::Relaxed);
            let after = self.seq.load(Ordering::Acquire);
            if before == after {
                return Some(Anchor { samples, nanos });
            }
            std::hint::spin_loop();
        }
        None
    }
}

impl Default for AnchorCell {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[test]
    fn leer_bis_zum_ersten_publish() {
        let cell = AnchorCell::new();
        assert_eq!(cell.load(8), None);

        cell.publish(Anchor {
            samples: 480,
            nanos: 10_000_000,
        });
        assert_eq!(
            cell.load(8),
            Some(Anchor {
                samples: 480,
                nanos: 10_000_000
            })
        );
    }

    #[test]
    fn letzter_schreibvorgang_gewinnt() {
        let cell = AnchorCell::new();
        for i in 1..=100_u64 {
            cell.publish(Anchor {
                samples: i * 480,
                nanos: i * 10_000_000,
            });
        }
        let a = cell.load(8).unwrap();
        assert_eq!(a.samples, 48_000);
        assert_eq!(a.nanos, 1_000_000_000);
    }

    #[test]
    fn nebenlaeufig_niemals_zerrissen() {
        // Der Anker haelt die Invariante nanos == samples * 1000: ein
        // zerrissener Lesevorgang wuerde sie brechen.
        let cell = Arc::new(AnchorCell::new());
        let writer = {
            let cell = Arc::clone(&cell);
            std::thread::spawn(move || {
                for i in 1..200_000_u64 {
                    cell.publish(Anchor {
                        samples: i,
                        nanos: i * 1_000,
                    });
                }
            })
        };

        let mut reads = 0_u32;
        let mut torn = 0_u32;
        while !writer.is_finished() {
            if let Some(a) = cell.load(64) {
                reads += 1;
                if a.nanos != a.samples * 1_000 {
                    torn += 1;
                }
            }
        }
        writer.join().unwrap();

        assert_eq!(torn, 0, "zerrissener Lesevorgang trotz Seqlock");
        assert!(reads > 0, "kein einziger Lesevorgang zustande gekommen");
    }

    #[test]
    fn manuelle_uhr_ist_deterministisch() {
        let clock = ManualMonotonic::new();
        assert_eq!(clock.now_nanos(), 0);
        clock.advance(10_000_000);
        assert_eq!(clock.now_nanos(), 10_000_000);
        clock.set(42);
        assert_eq!(clock.now_nanos(), 42);
    }
}
