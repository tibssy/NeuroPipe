pub mod endpoint;
pub mod parakeet;
pub mod smart_turn;

use anyhow::Result;

pub trait SttEngine {
    fn load(&mut self) -> Result<()>;
    fn unload(&mut self);
    fn is_loaded(&self) -> bool;
    fn transcribe(&mut self, samples: &[f32]) -> Result<String>;
}

/// Scores how likely the current pause marks the end of the user's turn.
/// `HeuristicTurnEnd` (heuristic prosody) is the Phase 0 implementation; a
/// trained ONNX classifier can implement the same trait and be swapped in.
pub trait TurnEndDetector: Send {
    fn score(&mut self, ctx: &endpoint::TurnContext) -> f32;
    fn reset(&mut self);
}
