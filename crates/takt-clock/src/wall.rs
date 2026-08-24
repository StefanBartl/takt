// SPDX-License-Identifier: Apache-2.0 OR MIT
//! Die Wall-Clock: monotone Systemzeit als Taktquelle. Studio-Modus.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use takt_core::{Rational, Timestamp};

use crate::anchor::MonotonicSource;
use crate::{Clock, ClockKind, delta_nanos};

/// Uhr, die stur der monotonen Systemzeit folgt.
///
/// Richtig fuer den Studio-Modus: dort gibt es keine Datei, deren Position
/// man folgen koennte. Es gibt Live-Eingaenge, die im Jetzt liefern, und einen
/// Stream, der nicht stocken darf. Wer nicht rechtzeitig fertig wird, wird
/// verworfen — die Uhr wartet auf niemanden.
#[derive(Debug)]
pub struct WallClock {
    source: Arc<dyn MonotonicSource>,
    /// Systemzeit, die als Position null gilt.
    origin_nanos: AtomicU64,
}

impl WallClock {
    /// Setzt den Nullpunkt auf jetzt.
    #[must_use]
    pub fn new(source: Arc<dyn MonotonicSource>) -> Self {
        let origin = source.now_nanos();
        Self {
            source,
            origin_nanos: AtomicU64::new(origin),
        }
    }

    /// Setzt den Nullpunkt neu auf jetzt.
    ///
    /// Entspricht dem Start einer Aufnahme oder eines Streams.
    pub fn restart(&self) {
        self.origin_nanos
            .store(self.source.now_nanos(), Ordering::Release);
    }
}

impl Clock for WallClock {
    fn kind(&self) -> ClockKind {
        ClockKind::Wall
    }

    fn now(&self) -> Timestamp {
        let elapsed = self
            .source
            .now_nanos()
            .saturating_sub(self.origin_nanos.load(Ordering::Acquire));
        Timestamp::from_nanos(i64::try_from(elapsed).unwrap_or(i64::MAX))
    }

    fn until_nanos(&self, pts: Timestamp) -> i64 {
        delta_nanos(self.now(), pts)
    }

    fn may_drop(&self) -> bool {
        true
    }
}

impl WallClock {
    /// Laufzeit seit dem Nullpunkt in Millisekunden. Fuer Anzeigen.
    #[must_use]
    pub fn elapsed_millis(&self) -> i64 {
        self.now()
            .rescale(Rational::MILLIS)
            .map_or(0, Timestamp::ticks)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::anchor::ManualMonotonic;

    #[test]
    fn beginnt_bei_null_und_laeuft_mit() {
        let src = Arc::new(ManualMonotonic::new());
        src.set(5_000_000_000); // Nullpunkt liegt irgendwo
        let clock = WallClock::new(Arc::clone(&src) as Arc<dyn MonotonicSource>);

        assert_eq!(clock.now().ticks(), 0);

        src.advance(250_000_000);
        assert_eq!(clock.elapsed_millis(), 250);
    }

    #[test]
    fn restart_setzt_den_nullpunkt() {
        let src = Arc::new(ManualMonotonic::new());
        let clock = WallClock::new(Arc::clone(&src) as Arc<dyn MonotonicSource>);

        src.advance(1_000_000_000);
        assert_eq!(clock.elapsed_millis(), 1_000);

        clock.restart();
        assert_eq!(clock.elapsed_millis(), 0);

        src.advance(500_000_000);
        assert_eq!(clock.elapsed_millis(), 500);
    }

    #[test]
    fn studio_darf_verwerfen() {
        let src = Arc::new(ManualMonotonic::new());
        let clock = WallClock::new(src as Arc<dyn MonotonicSource>);
        assert!(clock.may_drop());
        assert_eq!(clock.kind(), ClockKind::Wall);
    }
}
