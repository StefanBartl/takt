// SPDX-License-Identifier: Apache-2.0 OR MIT
//! Die Timeline-Uhr: explizit gesetzte Position als Taktquelle. Editor-Modus.

use std::sync::atomic::{AtomicI64, Ordering};

use takt_core::{Rational, Timestamp};

use crate::{Clock, ClockKind, delta_nanos};

/// Uhr, deren Position von aussen gesetzt wird.
///
/// Der grundlegende Unterschied zu den beiden Realtime-Uhren ist nicht die
/// Quelle, sondern die Richtung des Datenflusses:
///
/// * Player und Studio **schieben** Frames — die Uhr laeuft, und was nicht
///   rechtzeitig fertig ist, faellt weg.
/// * Der Editor **zieht** Frames — die Position steht, bis jemand sie
///   veraendert, und der Renderer darf sich Zeit nehmen.
///
/// Deshalb ist [`Clock::may_drop`] hier `false`. Beim Scrubben und beim
/// Export gilt Qualitaet vor Tempo. Ein uebersprungenes Frame im Export ist
/// ein Fehler, kein Kompromiss.
///
/// Die Position wird in Nanosekunden gehalten, weil eine Timeline Material
/// mit verschiedenen Zeitbasen mischt und keine davon privilegiert sein darf.
#[derive(Debug)]
pub struct TimelineClock {
    position_nanos: AtomicI64,
    /// Ende des Projekts, zum Begrenzen der Position.
    duration_nanos: AtomicI64,
}

impl TimelineClock {
    /// Neue Timeline der angegebenen Laenge, Position auf null.
    #[must_use]
    pub fn new(duration: Timestamp) -> Self {
        let dur = duration
            .rescale(Rational::NANOS)
            .map_or(0, Timestamp::ticks)
            .max(0);
        Self {
            position_nanos: AtomicI64::new(0),
            duration_nanos: AtomicI64::new(dur),
        }
    }

    /// Setzt die Position, begrenzt auf `0..=duration`.
    ///
    /// Genau das passiert beim Scrubben: der Nutzer zieht den Marker, die
    /// Position springt, und der Renderer holt das passende Frame.
    pub fn seek(&self, to: Timestamp) {
        let ns = to.rescale(Rational::NANOS).map_or(0, Timestamp::ticks);
        let clamped = ns.clamp(0, self.duration_nanos.load(Ordering::Acquire));
        self.position_nanos.store(clamped, Ordering::Release);
    }

    /// Verschiebt die Position relativ, etwa um eine Frame-Dauer.
    pub fn nudge_nanos(&self, delta: i64) {
        let current = self.position_nanos.load(Ordering::Acquire);
        let target = current.saturating_add(delta);
        let clamped = target.clamp(0, self.duration_nanos.load(Ordering::Acquire));
        self.position_nanos.store(clamped, Ordering::Release);
    }

    /// Setzt die Projektlaenge und begrenzt die Position mit.
    pub fn set_duration(&self, duration: Timestamp) {
        let dur = duration
            .rescale(Rational::NANOS)
            .map_or(0, Timestamp::ticks)
            .max(0);
        self.duration_nanos.store(dur, Ordering::Release);
        let pos = self.position_nanos.load(Ordering::Acquire);
        if pos > dur {
            self.position_nanos.store(dur, Ordering::Release);
        }
    }

    /// Projektlaenge.
    #[must_use]
    pub fn duration(&self) -> Timestamp {
        Timestamp::from_nanos(self.duration_nanos.load(Ordering::Acquire))
    }

    /// Fortschritt als Anteil, `0.0..=1.0`. **Nur zur Anzeige.**
    #[must_use]
    #[allow(clippy::cast_precision_loss)]
    pub fn progress(&self) -> f64 {
        let dur = self.duration_nanos.load(Ordering::Acquire);
        if dur <= 0 {
            return 0.0;
        }
        self.position_nanos.load(Ordering::Acquire) as f64 / dur as f64
    }
}

impl Clock for TimelineClock {
    fn kind(&self) -> ClockKind {
        ClockKind::Timeline
    }

    fn now(&self) -> Timestamp {
        Timestamp::from_nanos(self.position_nanos.load(Ordering::Acquire))
    }

    fn until_nanos(&self, pts: Timestamp) -> i64 {
        delta_nanos(self.now(), pts)
    }

    fn may_drop(&self) -> bool {
        // Der Editor wartet. Immer.
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ten_seconds() -> TimelineClock {
        TimelineClock::new(Timestamp::from_millis(10_000))
    }

    #[test]
    fn beginnt_bei_null() {
        let t = ten_seconds();
        assert_eq!(t.now().ticks(), 0);
        assert!((t.progress() - 0.0).abs() < f64::EPSILON);
    }

    #[test]
    fn seek_wird_begrenzt() {
        let t = ten_seconds();

        t.seek(Timestamp::from_millis(2_500));
        assert_eq!(t.now().rescale(Rational::MILLIS).unwrap().ticks(), 2_500);

        // Ueber das Ende hinaus: auf das Ende begrenzt.
        t.seek(Timestamp::from_millis(99_999));
        assert_eq!(t.now().rescale(Rational::MILLIS).unwrap().ticks(), 10_000);

        // Vor den Anfang: auf null begrenzt.
        t.seek(Timestamp::from_millis(-500));
        assert_eq!(t.now().ticks(), 0);
    }

    #[test]
    fn nudge_um_eine_framedauer() {
        let t = ten_seconds();
        // Eine Frame-Dauer bei 24000/1001 fps.
        let frame_ns = 1_001_000_000_000 / 24_000;

        t.nudge_nanos(frame_ns);
        assert_eq!(t.now().ticks(), frame_ns);

        t.nudge_nanos(-frame_ns * 5);
        assert_eq!(t.now().ticks(), 0, "vor den Anfang wird begrenzt");
    }

    #[test]
    fn kuerzere_dauer_zieht_die_position_nach() {
        let t = ten_seconds();
        t.seek(Timestamp::from_millis(9_000));

        t.set_duration(Timestamp::from_millis(5_000));
        assert_eq!(t.now().rescale(Rational::MILLIS).unwrap().ticks(), 5_000);
    }

    #[test]
    fn editor_verwirft_nie() {
        let t = ten_seconds();
        assert!(!t.may_drop(), "im Editor gilt Qualitaet vor Tempo");
        assert_eq!(t.kind(), ClockKind::Timeline);
    }

    #[test]
    fn until_nanos_relativ_zur_position() {
        let t = ten_seconds();
        t.seek(Timestamp::from_millis(1_000));
        assert_eq!(t.until_nanos(Timestamp::from_millis(1_040)), 40_000_000);
        assert_eq!(t.until_nanos(Timestamp::from_millis(960)), -40_000_000);
    }

    #[test]
    fn progress_nur_zur_anzeige() {
        let t = ten_seconds();
        t.seek(Timestamp::from_millis(5_000));
        assert!((t.progress() - 0.5).abs() < 1e-9);

        // Leeres Projekt darf nicht durch Null teilen.
        let empty = TimelineClock::new(Timestamp::from_millis(0));
        assert!((empty.progress() - 0.0).abs() < f64::EPSILON);
    }
}
