// SPDX-License-Identifier: Apache-2.0 OR MIT
//! Fehlertypen des Kerns.
//!
//! Bewusst ohne `thiserror` oder `anyhow`: Zone 1 soll so wenige
//! Abhaengigkeiten wie moeglich einziehen, und der Kern hat nur eine Handvoll
//! Fehlerfaelle. Die Anwendungsschicht in Zone 2 darf `anyhow` verwenden.

use std::fmt;

/// Ergebnis-Kurzform des Kerns.
pub type Result<T> = std::result::Result<T, Error>;

/// Fehler des Kerns.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum Error {
    /// Eine Zeitbasis war null oder nicht darstellbar.
    InvalidTimeBase,
    /// Eine Umrechnung passte nicht in den Zielbereich.
    TimeOverflow,
    /// Die Frame-Geometrie passt nicht zum Pixelformat.
    InvalidGeometry {
        /// Breite in Pixeln.
        width: u32,
        /// Hoehe in Pixeln.
        height: u32,
    },
    /// Der Frame-Pool ist erschoepft. Der Verbraucher kommt nicht nach.
    PoolExhausted {
        /// Obergrenze des Pools.
        capacity: usize,
    },
    /// Eine Funktion wurde fuer eine noch nicht gebaute Plattform aufgerufen.
    Unsupported(&'static str),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidTimeBase => f.write_str("ungueltige Zeitbasis"),
            Self::TimeOverflow => f.write_str("Zeitumrechnung ausserhalb des Wertebereichs"),
            Self::InvalidGeometry { width, height } => {
                write!(f, "ungueltige Frame-Geometrie {width}x{height}")
            }
            Self::PoolExhausted { capacity } => {
                write!(f, "Frame-Pool erschoepft (Kapazitaet {capacity})")
            }
            Self::Unsupported(what) => write!(f, "nicht unterstuetzt: {what}"),
        }
    }
}

impl std::error::Error for Error {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn meldungen_sind_lesbar() {
        assert_eq!(
            Error::InvalidGeometry {
                width: 0,
                height: 1080
            }
            .to_string(),
            "ungueltige Frame-Geometrie 0x1080"
        );
        assert_eq!(
            Error::PoolExhausted { capacity: 4 }.to_string(),
            "Frame-Pool erschoepft (Kapazitaet 4)"
        );
    }
}
