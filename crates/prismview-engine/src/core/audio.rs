//! Análise de áudio independente de SO (Fase 4): FFT radix-2 de 2048 pontos, 64 bandas em escala
//! logarítmica (30 Hz – 16 kHz), agregados legados (bass/mid/treble/level, mesmos nomes do
//! `ReactiveSignalDto` do Estúdio) e detecção de batida por fluxo espectral nos graves com limiar
//! adaptativo + estimativa de BPM.
//!
//! Produtores de plataforma (WASAPI loopback, CoreAudio, PipeWire) só empurram amostras mono com
//! `push` e chamam `analyze` quando `ready()`. Zero alocação depois de `new`: tabelas (janela de
//! Hann, twiddles, bit-reversal, faixas de bins) são pré-calculadas; buffers são reaproveitados.

use std::f32::consts::PI;

pub const FFT_SIZE: usize = 2048;
pub const BAND_COUNT: usize = 64;

const HALF: usize = FFT_SIZE / 2;
const BAND_MIN_HZ: f32 = 30.0;
const BAND_MAX_HZ: f32 = 16_000.0;
/// Faixa dinâmica mapeada para 0..=1 (dBFS após compensação de inclinação).
const DB_FLOOR: f32 = -72.0;
const DB_CEIL: f32 = -14.0;
/// Compensação espectral (dB por oitava, relativa a 1 kHz): música tem ~-3 dB/oitava.
const TILT_DB_PER_OCTAVE: f32 = 3.0;
/// Constantes de tempo do envelope das bandas (s): ataque rápido, release suave.
const BAND_ATTACK_S: f32 = 0.012;
const BAND_RELEASE_S: f32 = 0.140;
/// Análises por segundo (hop = sample_rate / ANALYSIS_HZ).
const ANALYSIS_HZ: u32 = 60;

/// Histórico do fluxo para o limiar adaptativo (~1 s a 60 Hz).
const FLUX_HISTORY: usize = 64;
const BEAT_LOW_HZ: f32 = 40.0;
const BEAT_HIGH_HZ: f32 = 180.0;
const BEAT_SENSITIVITY: f32 = 1.45;
const BEAT_MIN_FLUX: f32 = 0.035;
/// Período refratário (s) → limita a ~214 BPM.
const BEAT_REFRACTORY_S: f64 = 0.28;
const BPM_MIN: f32 = 70.0;
const BPM_MAX: f32 = 180.0;
/// Sem batidas por esse tempo → BPM volta a 0.
const BPM_TIMEOUT_S: f64 = 6.0;

const BASS_MAX_HZ: f32 = 250.0;
const MID_MAX_HZ: f32 = 2_200.0;
const TREBLE_MAX_HZ: f32 = 12_000.0;

/// Resultado de uma análise (reaproveitado; leia por referência).
#[derive(Debug, Clone, Copy)]
pub struct AudioFrame {
    /// Energia por banda 0..=1, envelope com ataque/release.
    pub bands: [f32; BAND_COUNT],
    pub bass: f32,
    pub mid: f32,
    pub treble: f32,
    /// RMS da janela, comprimido perceptualmente.
    pub level: f32,
    /// `Some(força 0..=1)` se esta análise detectou uma batida.
    pub beat: Option<f32>,
    /// BPM estimado (0 = desconhecido).
    pub bpm: f32,
    /// Tudo praticamente zero (permite ao produtor parar de publicar/acordar o loop).
    pub silent: bool,
}

impl Default for AudioFrame {
    fn default() -> Self {
        Self {
            bands: [0.0; BAND_COUNT],
            bass: 0.0,
            mid: 0.0,
            treble: 0.0,
            level: 0.0,
            beat: None,
            bpm: 0.0,
            silent: true,
        }
    }
}

#[derive(Debug, Clone, Copy, Default)]
struct BandMap {
    /// Faixa de bins [lo, hi) para bandas mais largas que 1 bin.
    lo: u16,
    hi: u16,
    /// Bin fracionário do centro (interpolação para bandas estreitas nos graves).
    center_bin: f32,
    narrow: bool,
    /// Ganho de compensação de inclinação (dB).
    tilt_db: f32,
}

pub struct AudioAnalyzer {
    sample_rate: u32,
    hop: usize,

    ring: Box<[f32; FFT_SIZE]>,
    ring_pos: usize,
    filled: usize,
    since_analysis: usize,

    window: Box<[f32; FFT_SIZE]>,
    window_gain: f32,
    tw_re: Box<[f32; HALF]>,
    tw_im: Box<[f32; HALF]>,
    bitrev: Box<[u16; FFT_SIZE]>,
    re: Box<[f32; FFT_SIZE]>,
    im: Box<[f32; FFT_SIZE]>,
    mags: Box<[f32; HALF]>,

    bands_map: [BandMap; BAND_COUNT],
    bass_range: (usize, usize),
    mid_range: (usize, usize),
    treble_range: (usize, usize),
    beat_range: (usize, usize),

    raw_prev: [f32; BAND_COUNT],
    flux_hist: [f32; FLUX_HISTORY],
    flux_pos: usize,
    flux_filled: usize,

    clock_s: f64,
    last_beat_s: f64,
    interval_ema: f32,

    out: AudioFrame,
}

impl AudioAnalyzer {
    pub fn new(sample_rate: u32) -> Self {
        let mut a = Self {
            sample_rate: 0,
            hop: 0,
            ring: Box::new([0.0; FFT_SIZE]),
            ring_pos: 0,
            filled: 0,
            since_analysis: 0,
            window: Box::new([0.0; FFT_SIZE]),
            window_gain: 1.0,
            tw_re: Box::new([0.0; HALF]),
            tw_im: Box::new([0.0; HALF]),
            bitrev: Box::new([0; FFT_SIZE]),
            re: Box::new([0.0; FFT_SIZE]),
            im: Box::new([0.0; FFT_SIZE]),
            mags: Box::new([0.0; HALF]),
            bands_map: [BandMap::default(); BAND_COUNT],
            bass_range: (0, 0),
            mid_range: (0, 0),
            treble_range: (0, 0),
            beat_range: (0, 0),
            raw_prev: [0.0; BAND_COUNT],
            flux_hist: [0.0; FLUX_HISTORY],
            flux_pos: 0,
            flux_filled: 0,
            clock_s: 0.0,
            last_beat_s: f64::NEG_INFINITY,
            interval_ema: 0.0,
            out: AudioFrame::default(),
        };

        let mut sum = 0.0f32;
        for i in 0..FFT_SIZE {
            let w = 0.5 * (1.0 - (2.0 * PI * i as f32 / (FFT_SIZE as f32 - 1.0)).cos());
            a.window[i] = w;
            sum += w;
        }
        // Amplitude de pico de uma senoide em fundo de escala → 1.0.
        a.window_gain = 2.0 / sum.max(1e-6);

        for k in 0..HALF {
            let ang = -2.0 * PI * k as f32 / FFT_SIZE as f32;
            a.tw_re[k] = ang.cos();
            a.tw_im[k] = ang.sin();
        }
        let bits = FFT_SIZE.trailing_zeros();
        for i in 0..FFT_SIZE {
            a.bitrev[i] = ((i as u32).reverse_bits() >> (32 - bits)) as u16;
        }

        a.set_sample_rate(sample_rate);
        a
    }

    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    /// Reconfigura as faixas de bins (troca de dispositivo/formato). Não aloca.
    pub fn set_sample_rate(&mut self, sample_rate: u32) {
        let sr = sample_rate.clamp(8_000, 384_000);
        if sr == self.sample_rate {
            return;
        }
        self.sample_rate = sr;
        self.hop = (sr / ANALYSIS_HZ).max(128) as usize;
        let bin_hz = sr as f32 / FFT_SIZE as f32;
        let nyquist = sr as f32 * 0.5;
        let max_hz = BAND_MAX_HZ.min(nyquist * 0.95);
        let ratio = max_hz / BAND_MIN_HZ;

        let (mut bass, mut mid, mut treble, mut beat) =
            ((usize::MAX, 0), (usize::MAX, 0), (usize::MAX, 0), (usize::MAX, 0));
        let widen = |r: &mut (usize, usize), b: usize| {
            r.0 = r.0.min(b);
            r.1 = r.1.max(b + 1);
        };

        for b in 0..BAND_COUNT {
            let f_lo = BAND_MIN_HZ * ratio.powf(b as f32 / BAND_COUNT as f32);
            let f_hi = BAND_MIN_HZ * ratio.powf((b + 1) as f32 / BAND_COUNT as f32);
            let fc = (f_lo * f_hi).sqrt();
            let lo = ((f_lo / bin_hz).floor() as usize).clamp(1, HALF - 1);
            let hi = ((f_hi / bin_hz).ceil() as usize).clamp(lo + 1, HALF);
            self.bands_map[b] = BandMap {
                lo: lo as u16,
                hi: hi as u16,
                center_bin: (fc / bin_hz).clamp(1.0, (HALF - 2) as f32),
                narrow: (f_hi - f_lo) < bin_hz * 1.5,
                tilt_db: TILT_DB_PER_OCTAVE * (fc / 1000.0).log2(),
            };
            if fc < BASS_MAX_HZ {
                widen(&mut bass, b);
            } else if fc < MID_MAX_HZ {
                widen(&mut mid, b);
            } else if fc <= TREBLE_MAX_HZ {
                widen(&mut treble, b);
            }
            if (BEAT_LOW_HZ..=BEAT_HIGH_HZ).contains(&fc) {
                widen(&mut beat, b);
            }
        }
        let fix = |r: (usize, usize)| if r.0 == usize::MAX { (0, 0) } else { r };
        self.bass_range = fix(bass);
        self.mid_range = fix(mid);
        self.treble_range = fix(treble);
        self.beat_range = fix(beat);
        self.reset();
    }

    /// Zera estado (sem realocar). Usar após reconexão do dispositivo.
    pub fn reset(&mut self) {
        self.ring.fill(0.0);
        self.ring_pos = 0;
        self.filled = 0;
        self.since_analysis = 0;
        self.raw_prev = [0.0; BAND_COUNT];
        self.flux_hist = [0.0; FLUX_HISTORY];
        self.flux_pos = 0;
        self.flux_filled = 0;
        self.out = AudioFrame::default();
        self.interval_ema = 0.0;
        self.last_beat_s = f64::NEG_INFINITY;
    }

    /// Empurra uma amostra mono (-1..=1).
    #[inline]
    pub fn push(&mut self, sample: f32) {
        let s = if sample.is_finite() { sample.clamp(-1.0, 1.0) } else { 0.0 };
        self.ring[self.ring_pos] = s;
        self.ring_pos = (self.ring_pos + 1) & (FFT_SIZE - 1);
        if self.filled < FFT_SIZE {
            self.filled += 1;
        }
        self.since_analysis += 1;
    }

    /// Empurra `frames` amostras de silêncio (pacotes `AUDCLNT_BUFFERFLAGS_SILENT`).
    pub fn push_silence(&mut self, frames: usize) {
        for _ in 0..frames.min(FFT_SIZE) {
            self.push(0.0);
        }
        if frames > FFT_SIZE {
            self.since_analysis += frames - FFT_SIZE;
        }
    }

    /// Há amostras novas suficientes para mais uma análise?
    #[inline]
    pub fn ready(&self) -> bool {
        self.since_analysis >= self.hop
    }

    /// Último resultado.
    pub fn frame(&self) -> &AudioFrame {
        &self.out
    }

    /// Roda FFT + bandas + batida. `dt_s` = tempo real desde a análise anterior (envelopes
    /// independentes da taxa de chamada).
    pub fn analyze(&mut self, dt_s: f32) -> &AudioFrame {
        let dt = dt_s.clamp(0.001, 0.25);
        self.since_analysis = 0;
        self.clock_s += dt as f64;

        // 1) Janela + permutação bit-reversa (ordem cronológica a partir de ring_pos).
        let mut sum_sq = 0.0f32;
        for i in 0..FFT_SIZE {
            let s = self.ring[(self.ring_pos + i) & (FFT_SIZE - 1)];
            sum_sq += s * s;
            let j = self.bitrev[i] as usize;
            self.re[j] = s * self.window[i];
            self.im[j] = 0.0;
        }

        // 2) Borboletas (twiddles da tabela: passo = FFT_SIZE / len).
        let mut len = 2usize;
        while len <= FFT_SIZE {
            let half = len >> 1;
            let step = FFT_SIZE / len;
            let mut i = 0;
            while i < FFT_SIZE {
                for k in 0..half {
                    let wr = self.tw_re[k * step];
                    let wi = self.tw_im[k * step];
                    let a = i + k;
                    let b = a + half;
                    let vr = self.re[b] * wr - self.im[b] * wi;
                    let vi = self.re[b] * wi + self.im[b] * wr;
                    let ur = self.re[a];
                    let ui = self.im[a];
                    self.re[a] = ur + vr;
                    self.im[a] = ui + vi;
                    self.re[b] = ur - vr;
                    self.im[b] = ui - vi;
                }
                i += len;
            }
            len <<= 1;
        }

        for k in 0..HALF {
            self.mags[k] = (self.re[k] * self.re[k] + self.im[k] * self.im[k]).sqrt() * self.window_gain;
        }

        // 3) Bandas log + envelope + fluxo espectral nos graves.
        let k_att = 1.0 - (-dt / BAND_ATTACK_S).exp();
        let k_rel = 1.0 - (-dt / BAND_RELEASE_S).exp();
        let mut flux = 0.0f32;
        let mut any = false;
        for b in 0..BAND_COUNT {
            let m = self.bands_map[b];
            let amp = if m.narrow {
                let c = m.center_bin;
                let i0 = c.floor() as usize;
                let t = c - i0 as f32;
                self.mags[i0] * (1.0 - t) + self.mags[(i0 + 1).min(HALF - 1)] * t
            } else {
                let mut peak = 0.0f32;
                for k in m.lo as usize..m.hi as usize {
                    peak = peak.max(self.mags[k]);
                }
                peak
            };
            let db = 20.0 * (amp + 1e-9).log10() + m.tilt_db;
            let raw = ((db - DB_FLOOR) / (DB_CEIL - DB_FLOOR)).clamp(0.0, 1.0);

            if b >= self.beat_range.0 && b < self.beat_range.1 {
                flux += (raw - self.raw_prev[b]).max(0.0);
            }
            self.raw_prev[b] = raw;

            let cur = self.out.bands[b];
            let k = if raw > cur { k_att } else { k_rel };
            let mut next = cur + (raw - cur) * k;
            if next < 0.002 {
                next = 0.0;
            }
            any |= next > 0.0;
            self.out.bands[b] = next;
        }
        let beat_bands = (self.beat_range.1 - self.beat_range.0).max(1) as f32;
        flux /= beat_bands;

        // 4) Agregados legados (mesma ponderação média/pico do Estúdio).
        self.out.bass = self.aggregate(self.bass_range);
        self.out.mid = self.aggregate(self.mid_range);
        self.out.treble = self.aggregate(self.treble_range);
        let filled = self.filled.max(1) as f32;
        let rms = (sum_sq / filled).sqrt();
        let v = rms * 4.2;
        let level_target = (v / (1.0 + v * 0.65)).clamp(0.0, 1.0);
        let k = if level_target > self.out.level { k_att } else { k_rel };
        self.out.level += (level_target - self.out.level) * k;
        if self.out.level < 0.002 {
            self.out.level = 0.0;
        }

        // 5) Batida: fluxo acima de média + σ·sensibilidade, com refratário.
        let (mean, std) = self.flux_stats();
        let threshold = mean + BEAT_SENSITIVITY * std + BEAT_MIN_FLUX;
        let since = self.clock_s - self.last_beat_s;
        self.out.beat = None;
        if self.flux_filled >= FLUX_HISTORY / 4
            && flux > threshold
            && since >= BEAT_REFRACTORY_S
            && self.out.level > 0.01
        {
            let strength = ((flux - mean) / (std * 3.0 + BEAT_MIN_FLUX)).clamp(0.15, 1.0);
            self.out.beat = Some(strength);
            self.update_bpm(since);
            self.last_beat_s = self.clock_s;
        }
        self.flux_hist[self.flux_pos] = flux;
        self.flux_pos = (self.flux_pos + 1) % FLUX_HISTORY;
        self.flux_filled = (self.flux_filled + 1).min(FLUX_HISTORY);

        if self.clock_s - self.last_beat_s > BPM_TIMEOUT_S {
            self.interval_ema = 0.0;
        }
        self.out.bpm = if self.interval_ema > 0.0 {
            (60.0 / self.interval_ema * 10.0).round() / 10.0
        } else {
            0.0
        };
        self.out.silent = !any && self.out.level == 0.0;
        &self.out
    }

    fn aggregate(&self, range: (usize, usize)) -> f32 {
        let (lo, hi) = range;
        if hi <= lo {
            return 0.0;
        }
        let mut sum = 0.0f32;
        let mut peak = 0.0f32;
        for &v in &self.out.bands[lo..hi] {
            sum += v;
            peak = peak.max(v);
        }
        ((sum / (hi - lo) as f32) * 0.35 + peak * 0.65).clamp(0.0, 1.0)
    }

    fn flux_stats(&self) -> (f32, f32) {
        let n = self.flux_filled;
        if n == 0 {
            return (0.0, 0.0);
        }
        let mut sum = 0.0f32;
        for &f in &self.flux_hist[..n] {
            sum += f;
        }
        let mean = sum / n as f32;
        let mut var = 0.0f32;
        for &f in &self.flux_hist[..n] {
            let d = f - mean;
            var += d * d;
        }
        (mean, (var / n as f32).sqrt())
    }

    fn update_bpm(&mut self, since_last: f64) {
        if !since_last.is_finite() || since_last > 2.0 {
            return;
        }
        // Dobra o intervalo para a faixa [60/BPM_MAX, 60/BPM_MIN] (meio/dobro de tempo).
        let mut interval = since_last as f32;
        let min_i = 60.0 / BPM_MAX;
        let max_i = 60.0 / BPM_MIN;
        while interval < min_i {
            interval *= 2.0;
        }
        while interval > max_i {
            interval *= 0.5;
        }
        self.interval_ema = if self.interval_ema <= 0.0 {
            interval
        } else {
            self.interval_ema + (interval - self.interval_ema) * 0.15
        };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sine_lights_the_right_band() {
        let sr = 48_000;
        let mut a = AudioAnalyzer::new(sr);
        let f = 1_000.0f32;
        for i in 0..FFT_SIZE {
            a.push(0.05 * (2.0 * PI * f * i as f32 / sr as f32).sin());
        }
        let frame = *a.analyze(0.5);
        let (best, _) = frame
            .bands
            .iter()
            .enumerate()
            .fold((0, 0.0f32), |acc, (i, &v)| if v > acc.1 { (i, v) } else { acc });
        let ratio = BAND_MAX_HZ / BAND_MIN_HZ;
        let fc = BAND_MIN_HZ * ratio.powf((best as f32 + 0.5) / BAND_COUNT as f32);
        assert!((fc / f).log2().abs() < 0.25, "banda {best} centro {fc} Hz");
        assert!(frame.mid > frame.bass);
    }

    #[test]
    fn silence_is_silent() {
        let mut a = AudioAnalyzer::new(44_100);
        a.push_silence(FFT_SIZE);
        assert!(a.analyze(0.016).silent);
    }
}
