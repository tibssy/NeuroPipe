pub mod parakeet;

use anyhow::Result;

pub trait SttEngine {
    fn load(&mut self) -> Result<()>;
    fn unload(&mut self);
    fn is_loaded(&self) -> bool;
    fn transcribe(&mut self, samples: &[f32]) -> Result<String>;
}