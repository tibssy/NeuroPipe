use std::fs;
use std::path::PathBuf;

/// TTS engines supported by the Rust TTS service, in a fixed display order.
pub const ENGINES: [&str; 3] = ["kokoro", "pocket-tts", "supertonic-3"];

/// Base directory where install.sh places TTS model files.
fn models_dir() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_default();
    PathBuf::from(home).join(".local/share/neuropipe/models")
}

fn engine_dir(engine: &str) -> PathBuf {
    models_dir().join(engine)
}

/// List voices an engine has on disk, mirroring the service's discovery:
/// kokoro from voices-v1.0.bin (npz entry names), pocket-tts from
/// voices/*.safetensors, supertonic-3 from voice_styles/*.json.
pub fn list_voices(engine: &str) -> Vec<String> {
    match engine {
        "kokoro" => kokoro_voices(),
        "pocket-tts" => dir_voices("voices", ".safetensors"),
        "supertonic-3" => dir_voices("voice_styles", ".json"),
        _ => Vec::new(),
    }
}

fn dir_voices(subdir: &str, extension: &str) -> Vec<String> {
    let dir = engine_dir(if subdir == "voices" { "pocket-tts" } else { "supertonic-3" }).join(subdir);
    let mut voices: Vec<String> = match fs::read_dir(&dir) {
        Ok(entries) => entries
            .filter_map(|entry| entry.ok())
            .filter_map(|entry| entry.file_name().to_str().map(str::to_owned))
            .filter(|name| name.ends_with(extension))
            .map(|name| name.trim_end_matches(extension).to_string())
            .collect(),
        Err(_) => Vec::new(),
    };
    voices.sort();
    voices
}

/// Voice names inside the Kokoro voices archive. The file is an npz (zip)
/// archive, one .npy entry per voice, so reading entry names is enough.
fn kokoro_voices() -> Vec<String> {
    let path = engine_dir("kokoro").join("voices-v1.0.bin");
    let file = match fs::File::open(&path) {
        Ok(file) => file,
        Err(_) => return Vec::new(),
    };
    let archive = match zip::ZipArchive::new(file) {
        Ok(archive) => archive,
        Err(_) => return Vec::new(),
    };
    let mut voices: Vec<String> = archive
        .file_names()
        .map(|name| name.trim_end_matches(".npy").to_string())
        .filter(|name| !name.is_empty())
        .collect();
    voices.sort();
    voices
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn engines_are_fixed_and_valid() {
        assert_eq!(ENGINES, ["kokoro", "pocket-tts", "supertonic-3"]);
    }

    #[test]
    fn unsupported_engine_yields_no_voices() {
        assert!(list_voices("bogus").is_empty());
    }
}
