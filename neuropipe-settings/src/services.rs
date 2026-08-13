use std::path::PathBuf;

/// One installable NeuroPipe service. The settings UI builds its tabs from the
/// detected subset of these, so a partial install only shows installed units.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Service {
    Stt,
    Assistant,
    Tts,
}

impl Service {
    pub const ALL: [Service; 3] = [Service::Stt, Service::Assistant, Service::Tts];

    pub fn unit_name(self) -> &'static str {
        match self {
            Service::Stt => "neuropipe-stt.service",
            Service::Assistant => "neuropipe-assistant.service",
            Service::Tts => "neuropipe-tts.service",
        }
    }

    /// Human-readable label used for the tab title.
    pub fn label(self) -> &'static str {
        match self {
            Service::Stt => "STT",
            Service::Assistant => "Assistant",
            Service::Tts => "TTS",
        }
    }

    /// Where install.sh writes the unit file (user-scoped systemd dir).
    fn unit_path(self) -> PathBuf {
        let home = std::env::var("HOME").unwrap_or_default();
        PathBuf::from(home)
            .join(".config/systemd/user")
            .join(self.unit_name())
    }

    /// A unit counts as installed when its unit file exists on disk. This is
    /// the same artifact `install.sh` drops for every component, so it stays
    /// accurate regardless of how the user chose the install selection.
    pub fn is_installed(self) -> bool {
        self.unit_path().is_file()
    }

    /// Detected set of installed services, in a fixed tab order.
    pub fn installed() -> Vec<Service> {
        Service::ALL.iter().copied().filter(|s| s.is_installed()).collect()
    }

    /// Runtime status via `systemctl --user is-active`.
    pub fn is_active(self) -> bool {
        let output = std::process::Command::new("systemctl")
            .args(["--user", "is-active", self.unit_name()])
            .output();
        match output {
            Ok(out) => out.status.success(),
            Err(_) => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Service;

    #[test]
    fn labels_and_units_match_installer() {
        assert_eq!(Service::Stt.unit_name(), "neuropipe-stt.service");
        assert_eq!(Service::Assistant.unit_name(), "neuropipe-assistant.service");
        assert_eq!(Service::Tts.unit_name(), "neuropipe-tts.service");
        assert_eq!(Service::Stt.label(), "STT");
        assert_eq!(Service::Assistant.label(), "Assistant");
        assert_eq!(Service::Tts.label(), "TTS");
    }

    #[test]
    fn installed_is_subset_of_all_and_ordered() {
        let installed = Service::installed();
        for s in &installed {
            assert!(Service::ALL.contains(s), "unexpected service {s:?}");
        }
        // Tab order is fixed regardless of install selection.
        assert!(installed.len() <= Service::ALL.len());
        for (i, s) in installed.iter().enumerate() {
            assert_eq!(*s, Service::ALL[i], "tab order diverged at index {i}");
        }
    }
}
