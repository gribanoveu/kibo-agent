//! Which shells this machine has to pick from: on Windows, cmd and
//! PowerShell 5.1 always, PowerShell 7 and Git Bash when installed. Elsewhere
//! none — there is nothing to pick.
//!
//! Git Bash is looked for beside `git.exe` and in Git's install folders, never
//! as `bash.exe` on `PATH`: `System32\bash.exe` is WSL's, a Linux machine with
//! its own files, not a shell in this folder.

use crate::domain::command_exec::ShellOption;

/// The one picked in Settings, while installed — see [`ShellOption::pick`].
/// `None` off Windows, where commands run under `Shell::default` and the
/// terminal under the login shell; the settings are not read there at all.
pub fn chosen() -> Option<ShellOption> {
    let options = available();
    if options.is_empty() {
        return None;
    }
    let picked = super::settings_store::load().ok().and_then(|settings| settings.shell);
    ShellOption::pick(picked, &options).cloned()
}

#[cfg(not(windows))]
pub fn available() -> Vec<ShellOption> {
    Vec::new()
}

#[cfg(windows)]
pub fn available() -> Vec<ShellOption> {
    use crate::domain::command_exec::ShellKind;
    use std::path::{Path, PathBuf};

    let env = |name: &str| std::env::var_os(name).map(PathBuf::from);
    let on_path = |exe: &str| {
        std::env::var_os("PATH")
            .into_iter()
            .flat_map(|path| std::env::split_paths(&path).collect::<Vec<_>>())
            .map(|dir| dir.join(exe))
            .find(|p| p.is_file())
    };
    let first = |candidates: Vec<Option<PathBuf>>| candidates.into_iter().flatten().find(|p| p.is_file());
    let system = env("SystemRoot").unwrap_or_else(|| PathBuf::from(r"C:\Windows"));

    let cmd = first(vec![env("ComSpec"), Some(system.join(r"System32\cmd.exe"))]);
    let windows_powershell = first(vec![Some(system.join(r"System32\WindowsPowerShell\v1.0\powershell.exe"))]);
    let powershell = first(vec![on_path("pwsh.exe"), env("ProgramFiles").map(|p| p.join(r"PowerShell\7\pwsh.exe"))]);
    // `git.exe` is in `cmd\`, `bin\` or `mingw64\bin\` of the Git folder.
    let beside_git = on_path("git.exe").and_then(|git| {
        git.ancestors().skip(1).take(3).map(|dir| dir.join(r"bin\bash.exe")).find(|p| p.is_file())
    });
    let git_bash = first(vec![
        beside_git,
        env("ProgramFiles").map(|p| p.join(r"Git\bin\bash.exe")),
        env("LOCALAPPDATA").map(|p| p.join(r"Programs\Git\bin\bash.exe")),
    ]);

    [
        (ShellKind::Cmd, cmd),
        (ShellKind::WindowsPowershell, windows_powershell),
        (ShellKind::Powershell, powershell),
        (ShellKind::GitBash, git_bash),
    ]
    .into_iter()
    .filter_map(|(kind, program)| {
        let program: &Path = program.as_deref()?;
        Some(ShellOption { kind, program: program.display().to_string() })
    })
    .collect()
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;
    use crate::domain::command_exec::ShellKind;

    /// Windows itself has these two; the CI runner has Git Bash besides.
    #[test]
    fn windows_has_cmd_and_powershell() {
        let kinds: Vec<ShellKind> = available().iter().map(|o| o.kind).collect();
        assert!(kinds.contains(&ShellKind::Cmd), "{kinds:?}");
        assert!(kinds.contains(&ShellKind::WindowsPowershell), "{kinds:?}");
        assert!(available().iter().all(|o| !o.program.to_lowercase().contains(r"system32\bash")));
    }
}
