// SPDX-License-Identifier: Apache-2.0 OR MIT
//! Pixelformate und Farbraum-Beschreibung.
//!
//! Diese Typen sind bewusst getrennt von den Frame-Daten: ein Frame kann in
//! jedem Format vorliegen, aber die Farbraum-Interpretation entscheidet, was
//! der Shader daraus macht. Der haeufigste Fehler in Video-Pipelines ist ein
//! Frame, dessen Farbmetadaten unterwegs verloren gehen — dann sieht HDR
//! aus wie flaues SDR, und niemand weiss, wo es passiert ist.

/// Speicheranordnung der Bilddaten.
///
/// Hardware-Decoder liefern fast immer `Nv12` oder `P010`; alles andere ist
/// Software-Pfad oder Zwischenergebnis.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum PixelFormat {
    /// 8 Bit, Luma-Ebene plus verschachtelte Chroma-Ebene, 4:2:0.
    /// Das Standardformat aller Hardware-Decoder.
    Nv12,
    /// 10 Bit in 16-Bit-Woertern, sonst wie [`PixelFormat::Nv12`].
    /// Der HDR-Pfad.
    P010,
    /// Drei getrennte Ebenen, 8 Bit, 4:2:0. Der Software-Klassiker.
    Yuv420P,
    /// Drei getrennte Ebenen, 10 Bit.
    Yuv420P10,
    /// Drei getrennte Ebenen ohne Chroma-Unterabtastung.
    Yuv444P,
    /// 8 Bit pro Kanal, RGBA verschachtelt. Ziel der Compositing-Stufe.
    Rgba8,
    /// 16 Bit Gleitkomma pro Kanal. Zwischenformat fuer HDR-Compositing.
    Rgba16F,
}

impl PixelFormat {
    /// Anzahl getrennter Speicherebenen.
    #[must_use]
    pub const fn planes(self) -> usize {
        match self {
            Self::Nv12 | Self::P010 => 2,
            Self::Yuv420P | Self::Yuv420P10 | Self::Yuv444P => 3,
            Self::Rgba8 | Self::Rgba16F => 1,
        }
    }

    /// Bits pro Komponente.
    #[must_use]
    pub const fn bit_depth(self) -> u8 {
        match self {
            Self::Nv12 | Self::Yuv420P | Self::Yuv444P | Self::Rgba8 => 8,
            Self::P010 | Self::Yuv420P10 => 10,
            Self::Rgba16F => 16,
        }
    }

    /// Ob das Format Chroma horizontal und vertikal halbiert (4:2:0).
    #[must_use]
    pub const fn is_subsampled_420(self) -> bool {
        matches!(
            self,
            Self::Nv12 | Self::P010 | Self::Yuv420P | Self::Yuv420P10
        )
    }

    /// Ob das Format bereits im RGB-Raum liegt.
    #[must_use]
    pub const fn is_rgb(self) -> bool {
        matches!(self, Self::Rgba8 | Self::Rgba16F)
    }
}

/// Farbprimaerwerte, also der Umfang des darstellbaren Farbraums.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum ColorPrimaries {
    /// BT.709 — HD-Video, sRGB-Primaerwerte. Der Alltagsfall.
    #[default]
    Bt709,
    /// BT.601 — Standard Definition, PAL und NTSC.
    Bt601,
    /// BT.2020 — UHD und HDR, deutlich weiterer Farbraum.
    Bt2020,
    /// DCI-P3 — Kino und Apple-Displays.
    DciP3,
    /// Aus der Datei nicht ermittelbar.
    Unknown,
}

/// Uebertragungsfunktion, umgangssprachlich Gamma.
///
/// Der entscheidende Unterschied zwischen SDR und HDR liegt hier, nicht bei
/// den Primaerwerten: PQ und HLG kodieren absolute beziehungsweise relative
/// Leuchtdichte statt eines Gamma-Verlaufs.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum TransferFunction {
    /// BT.709-Gamma. Der SDR-Standardfall.
    #[default]
    Bt709,
    /// sRGB-Gamma. Aehnlich BT.709, aber nicht identisch.
    Srgb,
    /// Perceptual Quantizer, SMPTE ST 2084 — HDR10, Dolby Vision.
    Pq,
    /// Hybrid Log-Gamma, ARIB STD-B67 — Rundfunk-HDR, SDR-abwaertskompatibel.
    Hlg,
    /// Linear. Nur als Zwischenzustand im Compositing.
    Linear,
    /// Aus der Datei nicht ermittelbar.
    Unknown,
}

impl TransferFunction {
    /// Ob diese Funktion einen HDR-Signalpfad beschreibt.
    #[must_use]
    pub const fn is_hdr(self) -> bool {
        matches!(self, Self::Pq | Self::Hlg)
    }
}

/// Wertebereich der Komponenten.
///
/// Die Verwechslung dieser beiden Faelle ist der Grund fuer ausgewaschene
/// oder zu kontrastreiche Wiedergabe — ein Klassiker bei HDMI-Ausgabe.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum ColorRange {
    /// Studio-Bereich: 16..235 bei 8 Bit. Der Normalfall bei Video.
    #[default]
    Limited,
    /// Voller Bereich: 0..255 bei 8 Bit. Der Normalfall bei Standbildern.
    Full,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ebenen_und_tiefe() {
        assert_eq!(PixelFormat::Nv12.planes(), 2);
        assert_eq!(PixelFormat::Yuv420P.planes(), 3);
        assert_eq!(PixelFormat::Rgba8.planes(), 1);
        assert_eq!(PixelFormat::P010.bit_depth(), 10);
    }

    #[test]
    fn hdr_haengt_an_der_uebertragungsfunktion() {
        assert!(TransferFunction::Pq.is_hdr());
        assert!(TransferFunction::Hlg.is_hdr());
        assert!(!TransferFunction::Bt709.is_hdr());
    }

    #[test]
    fn standardwerte_sind_der_sdr_alltagsfall() {
        assert_eq!(ColorPrimaries::default(), ColorPrimaries::Bt709);
        assert_eq!(TransferFunction::default(), TransferFunction::Bt709);
        assert_eq!(ColorRange::default(), ColorRange::Limited);
    }
}
