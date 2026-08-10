use codex_exec_server::ShellInfo;
use codex_shell_command::shell_detect::DetectedShell;
use serde::Deserialize;
use serde::Serialize;
use std::path::PathBuf;

pub use codex_shell_command::shell_detect::ShellType;

const CATALYST_POWERSHELL_PROFILE_ENV: &str = "CATALYST_POWERSHELL_PROFILE";
const CATALYST_POWERSHELL_PROFILE_LOADER: &str = concat!(
    "if ([string]::IsNullOrWhiteSpace($env:CATALYST_POWERSHELL_PROFILE) ",
    "-or -not (Test-Path -LiteralPath $env:CATALYST_POWERSHELL_PROFILE -PathType Leaf)) ",
    "{ throw 'Catalyst PowerShell profile is unavailable.' }; ",
    ". $env:CATALYST_POWERSHELL_PROFILE; ",
);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PowerShellProfileMode {
    Standard,
    Catalyst,
}

impl PowerShellProfileMode {
    fn from_environment() -> Self {
        let profile = std::env::var_os(CATALYST_POWERSHELL_PROFILE_ENV);
        Self::from_environment_value(profile.as_deref())
    }

    fn from_environment_value(profile: Option<&std::ffi::OsStr>) -> Self {
        #[cfg(windows)]
        if profile.is_some() {
            Self::Catalyst
        } else {
            Self::Standard
        }

        #[cfg(not(windows))]
        {
            let _ = profile;
            Self::Standard
        }
    }
}

fn derive_powershell_exec_args(
    shell_path: &str,
    command: &str,
    use_login_shell: bool,
    profile_mode: PowerShellProfileMode,
) -> Vec<String> {
    let mut args = vec![shell_path.to_string()];
    if !use_login_shell || profile_mode == PowerShellProfileMode::Catalyst {
        args.push("-NoProfile".to_string());
    }
    args.push("-Command".to_string());
    args.push(match profile_mode {
        PowerShellProfileMode::Standard => command.to_string(),
        PowerShellProfileMode::Catalyst => {
            format!("{CATALYST_POWERSHELL_PROFILE_LOADER}{command}")
        }
    });
    args
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Shell {
    pub(crate) shell_type: ShellType,
    pub(crate) shell_path: PathBuf,
}

impl Shell {
    pub fn name(&self) -> &'static str {
        self.shell_type.name()
    }

    /// Takes a string of shell and returns the full list of command args to
    /// use with `exec()` to run the shell command.
    pub fn derive_exec_args(&self, command: &str, use_login_shell: bool) -> Vec<String> {
        match self.shell_type {
            ShellType::Zsh | ShellType::Bash | ShellType::Sh => {
                let arg = if use_login_shell { "-lc" } else { "-c" };
                vec![
                    self.shell_path.to_string_lossy().to_string(),
                    arg.to_string(),
                    command.to_string(),
                ]
            }
            ShellType::PowerShell => derive_powershell_exec_args(
                &self.shell_path.to_string_lossy(),
                command,
                use_login_shell,
                PowerShellProfileMode::from_environment(),
            ),
            ShellType::Cmd => {
                let mut args = vec![self.shell_path.to_string_lossy().to_string()];
                args.push("/c".to_string());
                args.push(command.to_string());
                args
            }
        }
    }
}

impl From<DetectedShell> for Shell {
    fn from(detected: DetectedShell) -> Self {
        Self {
            shell_type: detected.shell_type,
            shell_path: detected.shell_path,
        }
    }
}

impl Shell {
    pub(crate) fn from_environment_shell_info(shell_info: ShellInfo) -> anyhow::Result<Self> {
        let shell_type = match shell_info.name.as_str() {
            "zsh" => ShellType::Zsh,
            "bash" => ShellType::Bash,
            "powershell" => ShellType::PowerShell,
            "sh" => ShellType::Sh,
            "cmd" => ShellType::Cmd,
            name => anyhow::bail!("unknown environment shell `{name}`"),
        };

        Ok(Self {
            shell_type,
            shell_path: PathBuf::from(shell_info.path),
        })
    }
}

#[cfg(all(test, unix))]
fn ultimate_fallback_shell() -> Shell {
    codex_shell_command::shell_detect::ultimate_fallback_shell().into()
}

pub fn get_shell_by_model_provided_path(shell_path: &PathBuf) -> Shell {
    codex_shell_command::shell_detect::get_shell_by_model_provided_path(shell_path).into()
}

pub fn get_shell(shell_type: ShellType, path: Option<&PathBuf>) -> Option<Shell> {
    codex_shell_command::shell_detect::get_shell(shell_type, path).map(Into::into)
}

pub fn default_user_shell() -> Shell {
    codex_shell_command::shell_detect::default_user_shell().into()
}

#[cfg(all(test, target_os = "macos"))]
fn default_user_shell_from_path(user_shell_path: Option<PathBuf>) -> Shell {
    codex_shell_command::shell_detect::default_user_shell_from_path(user_shell_path).into()
}

#[cfg(test)]
#[cfg(unix)]
#[path = "shell_tests.rs"]
mod tests;

#[cfg(all(test, windows))]
#[path = "shell_windows_tests.rs"]
mod windows_tests;
