// SPDX-License-Identifier: Apache-2.0 OR MIT
//! Die Audio-Master-Uhr: Wiedergabeposition als Taktquelle.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use takt_core::{Rational, Timestamp};

use crate::anchor::{Anchor, AnchorCell, MonotonicSource};
use crate::{Clock, ClockKind, delta_nanos};

/// Wie oft der Leser ein zerrissenes Seqlock erneut versucht.
const ANCHOR_RETRIES: u32 = 64;

/// Uhr, deren Position aus der Audio-Wiedergabe folgt.
///
/// Das ist die richtige Wahl fuer den Player, weil das Ohr Zeitfehler
/// ungleich empfindlicher wahrnimmt als das Auge: ein gedoppeltes Videobild
/// bemerkt niemand, eine Luecke im Ton jeder. Deshalb laeuft das Video dem
/// Audio nach und nicht umgekehrt.
///
/// # Interpolation
///
/// Zwischen zwei Audio-Callbacks wird mit der monotonen Systemuhr
/// weitergerechnet. Die Interpolation wird auf [`AudioMasterClock::max_extrapolation_nanos`]
/// begrenzt: bleibt der Anker laenger aus, ist der Audio-Thread ins Stocken
/// geraten, und eine weiter extrapolierte Position waere eine Luege.
#[derive(Debug)]
pub struct AudioMasterClock {
    anchor: Arc<AnchorCell>,
    source: Arc<dyn MonotonicSource>,
    sample_rate: u32,
    paused: AtomicBool,
    max_extrapolation_nanos: u64,
    /// Ob der Schreibgriff schon herausgegeben wurde. Siehe
    /// [`AudioMasterClock::writer`].
    writer_taken: AtomicBool,
}

impl AudioMasterClock {
    /// Neue Uhr fuer die angegebene Abtastrate.
    ///
    /// `max_extrapolation_nanos` sollte etwa der doppelten erwarteten
    /// Callback-Periode entsprechen — bei 10 ms Periode also 20 ms.
    #[must_use]
    pub fn new(
        sample_rate: u32,
        source: Arc<dyn MonotonicSource>,
        max_extrapolation_nanos: u64,
    ) -> Self {
        Self {
            anchor: Arc::new(AnchorCell::new()),
            source,
            sample_rate,
            paused: AtomicBool::new(false),
            max_extrapolation_nanos,
            writer_taken: AtomicBool::new(false),
        }
    }

    /// Der Schreibgriff fuer den Audio-Thread.
    ///
    /// Absichtlich ein eigener Typ: der Audio-Thread soll die Uhr nicht lesen
    /// koennen, nur stellen. Das macht die Datenflussrichtung im Typsystem
    /// sichtbar.
    ///
    /// Gibt beim zweiten Aufruf `None` zurueck. Das Seqlock im Anker vertraegt
    /// **genau einen** Schreiber — es erhoeht seinen Zaehler nicht atomar,
    /// weil ein zweiter Schreiber ohnehin ein Entwurfsfehler waere. Anstatt
    /// das nur zu dokumentieren, gibt die Uhr den Schreibgriff einmalig
    /// heraus; [`AudioClockWriter`] ist entsprechend nicht `Clone`.
    #[must_use]
    pub fn writer(&self) -> Option<AudioClockWriter> {
        if self.writer_taken.swap(true, Ordering::AcqRel) {
            return None;
        }
        Some(AudioClockWriter {
            anchor: Arc::clone(&self.anchor),
        })
    }

    /// Haelt die Uhr an. Die Position bleibt stehen.
    pub fn set_paused(&self, paused: bool) {
        self.paused.store(paused, Ordering::Release);
    }

    /// Ob die Uhr angehalten ist.
    #[must_use]
    pub fn is_paused(&self) -> bool {
        self.paused.load(Ordering::Acquire)
    }

    /// Obergrenze der Extrapolation zwischen zwei Ankern.
    #[must_use]
    pub const fn max_extrapolation_nanos(&self) -> u64 {
        self.max_extrapolation_nanos
    }

    /// Abtastrate, die diese Uhr zugrunde legt.
    #[must_use]
    pub const fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    /// Wie lange der letzte Anker zurueckliegt, in Nanosekunden.
    ///
    /// `None`, solange nie ein Anker hinterlegt wurde. Ein Wert deutlich ueber
    /// [`AudioMasterClock::max_extrapolation_nanos`] bedeutet, dass der
    /// Audio-Thread haengt — das gehoert in die Diagnose-Anzeige.
    #[must_use]
    pub fn anchor_age_nanos(&self) -> Option<u64> {
        let a = self.anchor.load(ANCHOR_RETRIES)?;
        Some(self.source.now_nanos().saturating_sub(a.nanos))
    }

    /// Position ohne Extrapolation: genau der letzte gemeldete Anker.
    #[must_use]
    pub fn anchored_position(&self) -> Option<Timestamp> {
        let a = self.anchor.load(ANCHOR_RETRIES)?;
        Timestamp::from_samples(i64::try_from(a.samples).ok()?, self.sample_rate)
    }
}

impl Clock for AudioMasterClock {
    fn kind(&self) -> ClockKind {
        ClockKind::AudioMaster
    }

    fn now(&self) -> Timestamp {
        let base = Rational::new(1, i64::from(self.sample_rate)).unwrap_or(Rational::MILLIS);

        let Some(anchor) = self.anchor.load(ANCHOR_RETRIES) else {
            return Timestamp::zero(base);
        };

        let anchored = i64::try_from(anchor.samples).unwrap_or(i64::MAX);
        let at_anchor = Timestamp::new(anchored, base);

        if self.is_paused() {
            return at_anchor;
        }

        // Zwischen den Callbacks mit der Systemuhr weiterrechnen, aber nicht
        // unbegrenzt: ein alter Anker bedeutet ein Problem, keine Position.
        let elapsed = self
            .source
            .now_nanos()
            .saturating_sub(anchor.nanos)
            .min(self.max_extrapolation_nanos);

        let Ok(elapsed) = i64::try_from(elapsed) else {
            return at_anchor;
        };
        let Some(offset) = Timestamp::from_nanos(elapsed).rescale(base) else {
            return at_anchor;
        };
        at_anchor.offset(offset.ticks()).unwrap_or(at_anchor)
    }

    fn until_nanos(&self, pts: Timestamp) -> i64 {
        delta_nanos(self.now(), pts)
    }

    fn may_drop(&self) -> bool {
        true
    }
}

/// Schreibgriff auf die Audio-Master-Uhr, gedacht fuer den Audio-Thread.
///
/// # Realtime-Zusagen
///
/// [`AudioClockWriter::report`] alloziert nicht, nimmt kein Lock, blockiert
/// nicht und kann nicht in Panik geraten. Damit ist der Aufruf aus einem
/// Audio-Callback erlaubt.
///
/// Bewusst **nicht** `Clone`: das Seqlock im Anker vertraegt genau einen
/// Schreiber.
#[derive(Debug)]
pub struct AudioClockWriter {
    anchor: Arc<AnchorCell>,
}

impl AudioClockWriter {
    /// Meldet, dass zur Systemzeit `now_nanos` insgesamt `samples_played`
    /// Samples an das Geraet uebergeben wurden.
    ///
    /// **Aufrufbar aus dem Audio-Thread.**
    pub fn report(&self, samples_played: u64, now_nanos: u64) {
        self.anchor.publish(Anchor {
            samples: samples_played,
            nanos: now_nanos,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::anchor::ManualMonotonic;

    fn setup(rate: u32) -> (AudioMasterClock, Arc<ManualMonotonic>) {
        let src = Arc::new(ManualMonotonic::new());
        let clock = AudioMasterClock::new(
            rate,
            Arc::clone(&src) as Arc<dyn MonotonicSource>,
            20_000_000,
        );
        (clock, src)
    }

    #[test]
    fn schreibgriff_wird_nur_einmal_herausgegeben() {
        // Das Seqlock im Anker vertraegt genau einen Schreiber. Diese
        // Invariante steht im Typsystem, nicht nur in der Dokumentation.
        let (clock, _src) = setup(48_000);
        assert!(clock.writer().is_some());
        assert!(
            clock.writer().is_none(),
            "ein zweiter Schreibgriff wuerde das Seqlock brechen"
        );
    }

    #[test]
    fn steht_bei_null_ohne_anker() {
        let (clock, _src) = setup(48_000);
        assert_eq!(clock.now().ticks(), 0);
        assert_eq!(clock.anchor_age_nanos(), None);
        assert_eq!(clock.anchored_position(), None);
    }

    #[test]
    fn folgt_der_sampleposition_exakt() {
        let (clock, src) = setup(48_000);
        let w = clock.writer().unwrap();

        src.set(10_000_000); // 10 ms
        w.report(480, 10_000_000); // 480 Samples = 10 ms bei 48 kHz

        // Kein Zeitfortschritt seit dem Anker: exakt die Sample-Position.
        assert_eq!(clock.now().ticks(), 480);
        assert_eq!(clock.now().rescale(Rational::MILLIS).unwrap().ticks(), 10);
    }

    #[test]
    fn interpoliert_zwischen_callbacks() {
        let (clock, src) = setup(48_000);
        let w = clock.writer().unwrap();

        src.set(0);
        w.report(0, 0);

        // 5 ms nach dem Anker: 240 Samples weiter, ohne neuen Callback.
        src.set(5_000_000);
        assert_eq!(clock.now().ticks(), 240);

        // Der naechste Callback korrigiert auf den echten Wert.
        src.set(10_000_000);
        w.report(480, 10_000_000);
        assert_eq!(clock.now().ticks(), 480);
    }

    #[test]
    fn extrapolation_ist_begrenzt() {
        let (clock, src) = setup(48_000);
        let w = clock.writer().unwrap();

        src.set(0);
        w.report(0, 0);

        // Der Audio-Thread haengt: eine ganze Sekunde ohne neuen Anker.
        src.set(1_000_000_000);

        // Extrapoliert wird nur bis 20 ms = 960 Samples, nicht bis 48000.
        assert_eq!(clock.now().ticks(), 960);
        assert_eq!(clock.anchor_age_nanos(), Some(1_000_000_000));
    }

    #[test]
    fn pause_haelt_die_position() {
        let (clock, src) = setup(48_000);
        let w = clock.writer().unwrap();

        src.set(0);
        w.report(4_800, 0);
        clock.set_paused(true);

        src.set(15_000_000);
        assert_eq!(clock.now().ticks(), 4_800, "Pause muss die Uhr anhalten");

        clock.set_paused(false);
        assert_eq!(clock.now().ticks(), 4_800 + 720);
    }

    #[test]
    fn until_nanos_wird_negativ_bei_verzug() {
        let (clock, src) = setup(48_000);
        let w = clock.writer().unwrap();

        src.set(0);
        w.report(48_000, 0); // Position: 1 s

        // Ein Frame, das bei 1,020 s praesentiert werden soll: 20 ms Zeit.
        let future = Timestamp::from_millis(1_020);
        assert_eq!(clock.until_nanos(future), 20_000_000);

        // Ein Frame, das bei 0,980 s haette laufen sollen: 20 ms zu spaet.
        let past = Timestamp::from_millis(980);
        assert_eq!(clock.until_nanos(past), -20_000_000);

        assert!(clock.may_drop(), "der Player darf Frames verwerfen");
    }

    #[test]
    fn is_due_bei_halber_framedauer() {
        let (clock, src) = setup(48_000);
        let w = clock.writer().unwrap();
        src.set(0);
        w.report(48_000, 0); // 1 s

        // Halbe Frame-Dauer bei 60 fps.
        let tol = 8_333_333;
        assert!(clock.is_due(Timestamp::from_millis(1_005), tol));
        assert!(!clock.is_due(Timestamp::from_millis(1_020), tol));
    }

    #[test]
    fn funktioniert_auch_bei_44100() {
        // 44,1 kHz ist die Rate, bei der Sekundenrechnung immer rundet.
        let (clock, src) = setup(44_100);
        let w = clock.writer().unwrap();
        src.set(0);
        w.report(44_100 * 3_600, 0); // exakt eine Stunde

        let pos = clock.now();
        assert_eq!(pos.ticks(), 44_100 * 3_600);
        assert_eq!(pos.rescale(Rational::MILLIS).unwrap().ticks(), 3_600_000);
    }
}
