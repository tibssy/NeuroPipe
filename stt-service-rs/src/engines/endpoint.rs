//! Smart turn-end detection (endpointing).
//!
//! Replaces the fixed silence timeout with a small detector that scores, on a
//! cadence while the speaker pauses, how likely the turn has actually ended.
//! Phase 0 ships a heuristic prosodic scorer (`HeuristicTurnEnd`); a trained
//! classifier can later implement the same [`TurnEndDetector`] trait and be
//! dropped in without touching the service loop.

use crate::engines::TurnEndDetector;

/// Below this (Hz/s) a final-pitch slope is treated as terminal. Real terminal
/// falls run to hundreds of Hz/s; a slightly negative reading is F0-estimator
/// jitter on a flat contour and must not gate a turn end.
const TERMINAL_SLOPE_HZ_S: f32 = -20.0;

/// Context handed to the detector on each scoring pass.
#[derive(Debug, Clone)]
pub struct TurnContext {
    /// Recent audio (last ~1.8 s) including trailing speech + silence, 16 kHz.
    pub tail: Vec<f32>,
    /// Duration of the current silence run in milliseconds.
    pub silence_ms: u64,
    /// Total recorded audio so far in milliseconds.
    pub utterance_ms: u64,
    /// Latest Silero VAD speech probability (0..1). Reserved for classifiers;
    /// the heuristic scorer keys on prosody instead.
    #[allow(dead_code)]
    pub last_vad: f32,
}

/// Heuristic end-of-turn scorer based on prosodic cues.
///
/// Terminal turns typically end with a falling pitch contour and a
/// perceptible pause; mid-turn pauses tend to hold a flat/rising contour.
/// The score combines final-pitch slope, pause length and utterance length.
/// A falling contour is *required* to finalize a turn early: without one the
/// score is capped below the end threshold so mid-thought pauses never
/// truncate the utterance — only the hard ceiling (or a later falling
/// contour) ends the turn.
#[derive(Debug, Clone)]
pub struct HeuristicTurnEnd {
    /// Weight for the terminal-pitch-slope contribution.
    pub pitch_weight: f32,
    /// Weight for the pause-length contribution.
    pub pause_weight: f32,
    /// Weight for the utterance-length contribution.
    pub length_weight: f32,
}

impl Default for HeuristicTurnEnd {
    fn default() -> Self {
        Self {
            pitch_weight: 0.6,
            pause_weight: 0.3,
            length_weight: 0.1,
        }
    }
}

impl HeuristicTurnEnd {
    pub fn new() -> Self {
        Self::default()
    }
}

impl TurnEndDetector for HeuristicTurnEnd {
    fn score(&mut self, ctx: &TurnContext) -> f32 {
        // Pitch contribution in [0,1]: 1 when the voiced tail clearly falls.
        // An unvoiced/absent tail yields 0: it carries no terminal evidence.
        let pitch = match final_pitch_slope(&ctx.tail) {
            Some(slope) if slope < TERMINAL_SLOPE_HZ_S => {
                ((-slope - TERMINAL_SLOPE_HZ_S) / 30.0).min(1.0)
            }
            Some(_) => 0.0,
            None => 0.0,
        };
        // Pause contribution: reaches 1 after ~1.2 s of silence.
        let pause = (ctx.silence_ms as f32 / 1200.0).min(1.0);
        // Length contribution: short utterances are rarely complete turns.
        let length = (ctx.utterance_ms as f32 / 3000.0).min(1.0);
        let mut score =
            self.pitch_weight * pitch + self.pause_weight * pause + self.length_weight * length;
        // Mid-thought pauses must never finalize the turn: without a falling
        // contour, cap the score below the default end threshold (0.5) so only
        // the hard ceiling ends the turn.
        if pitch <= 0.0 {
            score = score.min(self.pause_weight + self.length_weight);
        }
        score
    }

    fn reset(&mut self) {}
}

/// Estimate the slope of the fundamental-frequency contour over the last
/// voiced stretch of `tail`, in Hz per second. `None` when not enough voiced
/// frames exist.
fn final_pitch_slope(tail: &[f32]) -> Option<f32> {
    const SR: usize = 16_000;
    const FRAME: usize = 512;
    const HOP: usize = 256;

    if tail.len() < FRAME * 2 {
        return None;
    }

    // Walk backwards over voiced frames, starting from the most recent energy
    // peak so the "last spoken syllable" is measured.
    let mut f0s: Vec<(f32, f32)> = Vec::new(); // (time_sec, f0_hz)
    let mut t = tail.len() as i64 - FRAME as i64;
    while t >= 0 {
        let frame = &tail[t as usize..t as usize + FRAME];
        let rms = (frame.iter().map(|v| v * v).sum::<f32>() / FRAME as f32).sqrt();
        if rms > 0.01 {
            if let Some(f0) = autocorr_f0(frame, SR) {
                f0s.push((t as f32 / SR as f32, f0));
            }
        }
        t -= HOP as i64;
    }
    f0s.reverse();
    if f0s.len() < 3 {
        return None;
    }
    // Linear-regression slope over the (up to) last 20 voiced frames.
    let n = f0s.len().min(20);
    let chunk = &f0s[f0s.len() - n..];
    let mean_t: f32 = chunk.iter().map(|p| p.0).sum::<f32>() / n as f32;
    let mean_f: f32 = chunk.iter().map(|p| p.1).sum::<f32>() / n as f32;
    let mut num = 0.0f32;
    let mut den = 0.0f32;
    for (t, f) in chunk {
        num += (t - mean_t) * (f - mean_f);
        den += (t - mean_t) * (t - mean_t);
    }
    if den < 1e-6 {
        return None;
    }
    Some(num / den)
}

/// Time-domain autocorrelation F0 estimator for a single 512-sample frame.
fn autocorr_f0(frame: &[f32], sr: usize) -> Option<f32> {
    const MIN_HZ: f32 = 70.0;
    const MAX_HZ: f32 = 350.0;
    let lo = (sr as f32 / MAX_HZ).ceil() as usize;
    let hi = (sr as f32 / MIN_HZ).floor() as usize;
    if hi <= lo || frame.len() < hi + 1 {
        return None;
    }
    let energy = frame.iter().map(|v| v * v).sum::<f32>();
    if energy < 1e-4 {
        return None;
    }
    let mut best_lag = lo;
    let mut best_norm = 0.0f32;
    for lag in lo..=hi {
        let mut num = 0.0f32;
        for i in 0..frame.len() - lag {
            num += frame[i] * frame[i + lag];
        }
        let norm = num / energy;
        if norm > best_norm {
            best_norm = norm;
            best_lag = lag;
        }
    }
    if best_norm < 0.3 {
        return None; // not periodic enough to be voiced
    }
    Some(sr as f32 / best_lag as f32)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tone(freq: f32, sr: usize, samples: usize, mut phase: f32) -> Vec<f32> {
        let mut out = Vec::with_capacity(samples);
        for _ in 0..samples {
            out.push((phase).sin());
            phase += 2.0 * std::f32::consts::PI * freq / sr as f32;
        }
        out
    }

    fn ctx(tail: Vec<f32>, silence_ms: u64, utterance_ms: u64) -> TurnContext {
        TurnContext {
            tail,
            silence_ms,
            utterance_ms,
            last_vad: 0.0,
        }
    }

    #[test]
    fn falling_tail_scores_high() {
        // A 440 Hz tone that glides down to ~160 Hz over the last 0.6 s.
        let sr = 16_000;
        let n = (0.6 * sr as f32) as usize;
        let mut tail = Vec::with_capacity(n);
        let mut phase = 0.0f32;
        let f0 = 440.0f32;
        let f1 = 160.0f32;
        for i in 0..n {
            let frac = i as f32 / n as f32;
            let freq = f0 + (f1 - f0) * frac;
            tail.push(phase.sin());
            phase += 2.0 * std::f32::consts::PI * freq / sr as f32;
        }
        let mut det = HeuristicTurnEnd::new();
        let score = det.score(&ctx(tail, 700, 8000));
        assert!(
            score > 0.55,
            "falling contour should read terminal, got {score}"
        );
    }

    #[test]
    fn flat_tail_scores_low() {
        let sr = 16_000;
        let n = (0.6 * sr as f32) as usize;
        let tail = tone(220.0, sr, n, 0.0);
        let mut det = HeuristicTurnEnd::new();
        let score = det.score(&ctx(tail, 700, 8000));
        assert!(
            score < 0.45,
            "flat contour should read continuation, got {score}"
        );
    }

    #[test]
    fn short_pause_scores_low() {
        let sr = 16_000;
        let n = (0.6 * sr as f32) as usize;
        let tail = tone(220.0, sr, n, 0.0);
        let mut det = HeuristicTurnEnd::new();
        let score = det.score(&ctx(tail, 150, 8000));
        assert!(
            score < 0.4,
            "micro-pause should not end the turn, got {score}"
        );
    }

    #[test]
    fn unvoiced_tail_never_scores_terminal() {
        // Regression: "…about …" mid-thought pause. The trailing speech has
        // fallen out of the 1.8s tail window, leaving mostly silence. The
        // unvoiced tail must NOT read as a falling contour and push the score
        // over the 0.5 threshold at ~1.6s.
        let sr = 16_000;
        let mut tail = vec![0.0f32; (1.8 * sr as f32) as usize]; // pure silence
        let mut det = HeuristicTurnEnd::new();
        let score = det.score(&ctx(tail, 1664, 4000));
        assert!(
            score < 0.5,
            "unvoiced tail with a long pause should stay below threshold, got {score}"
        );
        tail = vec![0.0f32; 100]; // near-empty tail
        det.reset();
        let score = det.score(&ctx(tail, 1664, 4000));
        assert!(
            score < 0.5,
            "near-empty tail should stay below threshold, got {score}"
        );
    }

    #[test]
    fn flat_tail_never_scores_terminal_at_long_pause() {
        // Mid-thought pause on a flat contour: even after ~1.7s of silence the
        // score must stay below the threshold — the trailing word must not be
        // cut off.
        let sr = 16_000;
        let n = (0.6 * sr as f32) as usize;
        let tail = tone(220.0, sr, n, 0.0);
        let mut det = HeuristicTurnEnd::new();
        let score = det.score(&ctx(tail, 1664, 4000));
        assert!(
            score < 0.5,
            "flat contour at 1.66s pause should stay below threshold, got {score}"
        );
    }

    #[test]
    fn pitch_slope_reports_falling() {
        let sr = 16_000;
        let n = (0.6 * sr as f32) as usize;
        let mut tail = Vec::with_capacity(n);
        let mut phase = 0.0f32;
        for i in 0..n {
            let frac = i as f32 / n as f32;
            let freq = 440.0 + (160.0 - 440.0) * frac;
            tail.push(phase.sin());
            phase += 2.0 * std::f32::consts::PI * freq / sr as f32;
        }
        let slope = final_pitch_slope(&tail);
        assert!(slope.is_some(), "voiced falling tail should yield a slope");
        assert!(
            slope.unwrap() < 0.0,
            "expected a negative slope, got {slope:?}"
        );
    }
}
