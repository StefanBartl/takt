// SPDX-License-Identifier: Apache-2.0 OR MIT
//! Audio-Puffer und Kanalbeschreibung.
//!
//! Alle Puffer hier werden **einmal** angelegt und danach nur noch gefuellt.
//! Es gibt bewusst keine Methode, die einen Puffer vergroessert: der
//! Audio-Callback darf nicht allozieren, und ein Typ, der das Gegenteil
//! anbietet, wird irgendwann falsch benutzt.

/// Abtastformat der Puffer.
///
/// Intern rechnet Takt durchgehend in `f32`. Die anderen Varianten existieren
/// nur fuer die Wandlung an den Raendern — Datei hinein, Geraet hinaus.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum SampleFormat {
    /// 16 Bit Ganzzahl mit Vorzeichen. Das haeufigste Dateiformat.
    I16,
    /// 24 Bit in 32 Bit ausgerichtet. Typisch fuer Audio-Interfaces.
    I24In32,
    /// 32 Bit Ganzzahl mit Vorzeichen.
    I32,
    /// 32 Bit Gleitkomma. Das interne Format von Takt.
    F32,
}

impl SampleFormat {
    /// Belegte Bytes pro Sample und Kanal.
    #[must_use]
    pub const fn bytes(self) -> usize {
        match self {
            Self::I16 => 2,
            Self::I24In32 | Self::I32 | Self::F32 => 4,
        }
    }

    /// Ob das Format ohne Wandlung verarbeitet werden kann.
    #[must_use]
    pub const fn is_native(self) -> bool {
        matches!(self, Self::F32)
    }
}

/// Kanalbelegung eines Puffers.
///
/// Bewusst klein gehalten: Surround kommt erst mit dem Studio-Modus, und eine
/// vollstaendige Kanalmaske jetzt zu entwerfen waere Spekulation.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum ChannelLayout {
    /// Ein Kanal.
    Mono,
    /// Links, rechts.
    #[default]
    Stereo,
    /// Vorn L/R, Center, LFE, hinten L/R.
    Surround51,
    /// Vorn L/R, Center, LFE, hinten L/R, seitlich L/R.
    Surround71,
}

impl ChannelLayout {
    /// Anzahl der Kanaele.
    #[must_use]
    pub const fn channels(self) -> usize {
        match self {
            Self::Mono => 1,
            Self::Stereo => 2,
            Self::Surround51 => 6,
            Self::Surround71 => 8,
        }
    }
}

/// Verschachtelter Audio-Puffer fester Kapazitaet.
///
/// Verschachtelt und nicht planar, weil alle Betriebssystem-Schnittstellen
/// (WASAPI, CoreAudio, PipeWire) verschachtelt arbeiten. Eine planare
/// Zwischenstufe kaeme erst mit der DSP-Kette hinzu.
#[derive(Debug, Clone)]
pub struct AudioBuffer {
    samples: Vec<f32>,
    layout: ChannelLayout,
    sample_rate: u32,
    /// Gueltige Frames im Puffer. Ein Frame ist ein Sample pro Kanal.
    frames: usize,
}

impl AudioBuffer {
    /// Legt einen Puffer fuer `capacity_frames` Frames an.
    ///
    /// Die Allokation geschieht hier und nur hier.
    #[must_use]
    pub fn with_capacity(layout: ChannelLayout, sample_rate: u32, capacity_frames: usize) -> Self {
        Self {
            samples: vec![0.0; capacity_frames * layout.channels()],
            layout,
            sample_rate,
            frames: 0,
        }
    }

    /// Kanalbelegung.
    #[must_use]
    pub const fn layout(&self) -> ChannelLayout {
        self.layout
    }

    /// Abtastrate in Hertz.
    #[must_use]
    pub const fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    /// Anzahl Kanaele.
    #[must_use]
    pub const fn channels(&self) -> usize {
        self.layout.channels()
    }

    /// Aufnahmefaehigkeit in Frames.
    #[must_use]
    pub fn capacity_frames(&self) -> usize {
        self.samples.len() / self.layout.channels()
    }

    /// Aktuell gueltige Frames.
    #[must_use]
    pub const fn frames(&self) -> usize {
        self.frames
    }

    /// Setzt die Anzahl gueltiger Frames.
    ///
    /// Wird auf die Kapazitaet begrenzt; der Puffer waechst nie.
    pub fn set_frames(&mut self, frames: usize) {
        self.frames = frames.min(self.capacity_frames());
    }

    /// Alle gueltigen Samples, verschachtelt.
    #[must_use]
    pub fn as_slice(&self) -> &[f32] {
        &self.samples[..self.frames * self.layout.channels()]
    }

    /// Schreibzugriff auf die volle Kapazitaet.
    ///
    /// Bewusst die volle Kapazitaet und nicht nur die gueltigen Frames: der
    /// Erzeuger fuellt und meldet danach ueber [`AudioBuffer::set_frames`],
    /// wie viel gueltig ist.
    #[must_use]
    pub fn as_mut_slice(&mut self) -> &mut [f32] {
        &mut self.samples
    }

    /// Setzt alle Samples auf Stille und die Frame-Zahl auf null.
    ///
    /// Alloziert nicht, ist also im Audio-Callback erlaubt.
    pub fn clear(&mut self) {
        self.samples.fill(0.0);
        self.frames = 0;
    }

    /// Dauer der gueltigen Frames als Sample-Anzahl.
    ///
    /// Zeit wird bei Audio in Samples gerechnet, nicht in Sekunden — bei
    /// 44,1 kHz ist jede Sekundenangabe eine Rundung.
    #[must_use]
    pub const fn duration_samples(&self) -> usize {
        self.frames
    }

    /// Groesster Absolutwert in den gueltigen Frames.
    ///
    /// Fuer Pegelanzeigen. Alloziert nicht.
    #[must_use]
    pub fn peak(&self) -> f32 {
        self.as_slice()
            .iter()
            .fold(0.0_f32, |acc, s| acc.max(s.abs()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kapazitaet_haengt_an_kanaelen() {
        let mut b = AudioBuffer::with_capacity(ChannelLayout::Stereo, 48_000, 128);
        assert_eq!(b.channels(), 2);
        assert_eq!(b.capacity_frames(), 128);
        assert_eq!(b.as_mut_slice().len(), 256);
    }

    #[test]
    fn puffer_waechst_nie() {
        let mut b = AudioBuffer::with_capacity(ChannelLayout::Stereo, 48_000, 128);
        b.set_frames(9_999);
        assert_eq!(b.frames(), 128);
    }

    #[test]
    fn slice_zeigt_nur_gueltige_frames() {
        let mut b = AudioBuffer::with_capacity(ChannelLayout::Stereo, 48_000, 128);
        assert_eq!(b.as_slice().len(), 0);
        b.set_frames(64);
        assert_eq!(b.as_slice().len(), 128);
    }

    #[test]
    fn peak_und_clear() {
        let mut b = AudioBuffer::with_capacity(ChannelLayout::Mono, 48_000, 4);
        b.as_mut_slice().copy_from_slice(&[0.25, -0.75, 0.5, 0.0]);
        b.set_frames(4);
        assert!((b.peak() - 0.75).abs() < f32::EPSILON);

        b.clear();
        assert_eq!(b.frames(), 0);
        assert!(b.peak().abs() < f32::EPSILON);
    }

    #[test]
    fn formatgroessen() {
        assert_eq!(SampleFormat::I16.bytes(), 2);
        assert_eq!(SampleFormat::I24In32.bytes(), 4);
        assert!(SampleFormat::F32.is_native());
        assert!(!SampleFormat::I16.is_native());
    }
}
