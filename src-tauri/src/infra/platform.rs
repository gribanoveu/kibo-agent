//! What the app runs on, as a product metric names it: the OS, its version
//! and the architecture (`docs/67-app-metrics.md`).
//!
//! Read once per process and kept: `os.version` does not change while the app
//! runs, and macOS answers only by running `sw_vers`. No dependency for it —
//! Windows reads the registry through `winreg`, which the microphone's
//! permission code already brings, and Linux reads `/etc/os-release`.

use std::sync::OnceLock;

/// The machine, as a row's `os.name`, `os.version` and `os.arch`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Facts {
    /// `macOS`, `Windows 11 Pro`, `Ubuntu`.
    pub os_name: String,
    /// `15.6.1`, `23H2`, `24.04`; empty when the system did not say.
    pub os_version: String,
    /// `aarch64`, `x86_64` — the build, not the machine it happens to run on.
    pub os_arch: String,
}

/// The facts of this process, read on the first call.
pub fn facts() -> &'static Facts {
    static FACTS: OnceLock<Facts> = OnceLock::new();
    FACTS.get_or_init(read)
}

fn read() -> Facts {
    let (os_name, os_version) = system();
    Facts { os_name, os_version, os_arch: std::env::consts::ARCH.to_string() }
}

#[cfg(target_os = "macos")]
fn system() -> (String, String) {
    let version = std::process::Command::new("sw_vers")
        .arg("-productVersion")
        .output()
        .ok()
        .and_then(|output| one_line(&output.stdout))
        .unwrap_or_default();
    ("macOS".to_string(), version)
}

#[cfg(windows)]
fn system() -> (String, String) {
    use winreg::enums::HKEY_LOCAL_MACHINE;
    use winreg::RegKey;
    let Ok(key) = RegKey::predef(HKEY_LOCAL_MACHINE).open_subkey(r"SOFTWARE\Microsoft\Windows NT\CurrentVersion") else {
        return ("Windows".to_string(), String::new());
    };
    let name = key.get_value::<String, _>("ProductName").unwrap_or_else(|_| "Windows".to_string());
    // `DisplayVersion` is the release a person knows (`23H2`); the build
    // number is there on releases that do not have one.
    let version = key
        .get_value::<String, _>("DisplayVersion")
        .or_else(|_| key.get_value::<String, _>("CurrentBuildNumber"))
        .unwrap_or_default();
    (name, version)
}

#[cfg(target_os = "linux")]
fn system() -> (String, String) {
    let text = std::fs::read_to_string("/etc/os-release").unwrap_or_default();
    let name = os_release_value(&text, "NAME").unwrap_or_else(|| "Linux".to_string());
    let version = os_release_value(&text, "VERSION_ID").unwrap_or_default();
    (name, version)
}

#[cfg(not(any(target_os = "macos", windows, target_os = "linux")))]
fn system() -> (String, String) {
    (std::env::consts::OS.to_string(), String::new())
}

/// The first line of a program's output, trimmed; `None` when there is none.
#[cfg(any(target_os = "macos", target_os = "linux", test))]
fn one_line(output: &[u8]) -> Option<String> {
    let text = String::from_utf8_lossy(output);
    let line = text.lines().next()?.trim();
    (!line.is_empty()).then(|| line.to_string())
}

/// `NAME="Ubuntu"` from `/etc/os-release`: the value without its quotes, and
/// nothing for a key the file does not have. A comment's key carries its `#`,
/// so it never answers for the real one, and the first line wins.
#[cfg(any(target_os = "linux", test))]
fn os_release_value(text: &str, key: &str) -> Option<String> {
    text.lines()
        .map(str::trim)
        .filter_map(|line| line.split_once('='))
        .find(|(name, _)| *name == key)
        .map(|(_, value)| value.trim().trim_matches(['"', '\'']).to_string())
        .filter(|value| !value.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_facts_are_named_and_read_once() {
        let read = facts();
        assert!(!read.os_name.is_empty(), "the app cannot say what it runs on");
        assert_eq!(read.os_arch, std::env::consts::ARCH);
        assert!(std::ptr::eq(read, facts()), "read twice: a per-row cost and a version that moves");
        #[cfg(target_os = "macos")]
        {
            assert_eq!(read.os_name, "macOS");
            assert!(read.os_version.split('.').count() >= 2, "sw_vers said {:?}", read.os_version);
        }
    }

    #[test]
    fn a_sw_vers_line_loses_its_newline_and_nothing_else() {
        assert_eq!(one_line(b"15.6.1\n").as_deref(), Some("15.6.1"));
        assert_eq!(one_line(b"  15.6.1  \n\n").as_deref(), Some("15.6.1"));
        // A version stamped on a second line is not a version.
        assert_eq!(one_line(b"\n15.6.1\n").as_deref(), None);
        assert_eq!(one_line(b""), None);
        assert_eq!(one_line(b" \n"), None);
    }

    #[test]
    fn os_release_values_come_back_without_their_quotes() {
        let text = "# a comment\nNAME=\"Ubuntu\"\nVERSION_ID=\"24.04\"\nID=ubuntu\n";
        assert_eq!(os_release_value(text, "NAME").as_deref(), Some("Ubuntu"));
        assert_eq!(os_release_value(text, "VERSION_ID").as_deref(), Some("24.04"));
        assert_eq!(os_release_value(text, "ID").as_deref(), Some("ubuntu"));
        assert_eq!(os_release_value(text, "MISSING"), None);
        assert_eq!(os_release_value("NAME=\n", "NAME"), None, "an empty name is no name");
        assert_eq!(os_release_value("NAME='Fedora'\n", "NAME").as_deref(), Some("Fedora"));
        // NAME=Ubuntu comes first, NAME=Debian does not replace it.
        assert_eq!(os_release_value("NAME=Ubuntu\nNAME=Debian\n", "NAME").as_deref(), Some("Ubuntu"));
        // A comment naming the key is not the key.
        assert_eq!(os_release_value("# NAME=Nope\n", "NAME"), None);
        assert_eq!(os_release_value("#NAME=Nope\nNAME=Real\n", "NAME").as_deref(), Some("Real"));
        assert_eq!(os_release_value("#NAME=Nope\n", "NAME"), None, "a commented key answered for a missing one");
    }
}
