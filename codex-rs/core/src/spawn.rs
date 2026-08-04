use codex_network_proxy::NetworkProxy;
use codex_utils_absolute_path::AbsolutePathBuf;
use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Stdio;
use tokio::process::Child;
use tokio::process::Command;
use tracing::trace;

use codex_protocol::permissions::NetworkSandboxPolicy;

/// Experimental environment variable that will be set to some non-empty value
/// if both of the following are true:
///
/// 1. The process was spawned by Codex as part of a shell tool call.
/// 2. NetworkSandboxPolicy is restricted for the tool call.
///
/// We may try to have just one environment variable for all sandboxing
/// attributes, so this may change in the future.
pub const CODEX_SANDBOX_NETWORK_DISABLED_ENV_VAR: &str = "CODEX_SANDBOX_NETWORK_DISABLED";

/// Should be set when the process is spawned under a sandbox. Currently, the
/// value is "seatbelt" for macOS, but it may change in the future to
/// accommodate sandboxing configuration and other sandboxing mechanisms.
pub const CODEX_SANDBOX_ENV_VAR: &str = "CODEX_SANDBOX";

#[derive(Debug, Clone, Copy)]
pub enum StdioPolicy {
    RedirectForShellTool,
    Inherit,
}

/// Spawns the appropriate child process for the exec params and sandbox settings,
/// ensuring the args and environment variables used to create the `Command`
/// (and `Child`) honor the configuration.
///
/// For now, we take `NetworkSandboxPolicy` as a parameter to spawn_child()
/// because we need to determine whether to set the
/// `CODEX_SANDBOX_NETWORK_DISABLED_ENV_VAR` environment variable.
pub(crate) struct SpawnChildRequest<'a> {
    pub program: PathBuf,
    pub args: Vec<String>,
    pub arg0: Option<&'a str>,
    pub cwd: AbsolutePathBuf,
    pub network_sandbox_policy: NetworkSandboxPolicy,
    pub network: Option<&'a NetworkProxy>,
    pub stdio_policy: StdioPolicy,
    pub env: HashMap<String, String>,
}

/// Windows PowerShell 5.1 defaults `$OutputEncoding` to US-ASCII, and that variable is what
/// PowerShell encodes with when it pipes into a native program. `echo '<json with 中文>' | tool`
/// therefore delivers `?` for every non-ASCII character — the data is destroyed at the pipe,
/// before the tool ever runs. It silently produced a deck of Chinese slides rendered entirely as
/// question marks, and it is invisible to anyone developing with `pwsh` on PATH, since PowerShell
/// 7 already defaults to UTF-8.
///
/// The target is the console's *input* code page, because that is what the receiving program
/// decodes redirected stdin with — writing UTF-8 into a code-page-936 console just trades `?` for
/// mojibake, which is worse for being harder to notice. (`[Console]::OutputEncoding` measures
/// identically today, since both console code pages default to the same value, but it is the
/// wrong end of the pipe to aim at.) Forcing the console to UTF-8 with `chcp 65001` does not work
/// either: PowerShell then leads the stream with a BOM the receiving parser reads as garbage.
/// Measured end-to-end against a real tool under PowerShell 5.1 with a hidden console, which is
/// what the desktop app spawns.
///
/// This aligns the two ends rather than making any encoding universally correct: text the console
/// code page cannot represent — Chinese on a Western-locale machine — is still lost. Alignment is
/// nonetheless strictly better than either default, both of which fail unconditionally.
///
/// This belongs at the spawn boundary rather than in `Shell::derive_exec_args`: that function's
/// output is also what safety classification, policy matching, and the command shown to the user
/// are computed from, and a leading assignment there stops known-safe commands from matching —
/// turning every read-only Windows command into an approval prompt.
///
/// Each command runs in a fresh `-Command` process, so the assignment has to ride along with the
/// command; there is no environment variable for it. Command-line arguments are unaffected either
/// way — Windows passes those as UTF-16 — so this only changes what crosses a pipe.
fn with_powershell_utf8_piping(program: &std::path::Path, args: Vec<String>) -> Vec<String> {
    const PROLOGUE: &str = "$OutputEncoding = [Console]::InputEncoding; ";

    let is_powershell = program
        .file_stem()
        .and_then(|stem| stem.to_str())
        .is_some_and(|stem| {
            stem.eq_ignore_ascii_case("powershell") || stem.eq_ignore_ascii_case("pwsh")
        });
    if !is_powershell {
        return args;
    }
    // Only the `-Command <string>` form runs an inline command that can contain a pipe. `-File`
    // and friends name a script whose own encoding we neither control nor should rewrite.
    let Some(index) = args
        .iter()
        .position(|arg| arg.eq_ignore_ascii_case("-Command"))
    else {
        return args;
    };
    let mut args = args;
    if let Some(command) = args.get_mut(index + 1) {
        command.insert_str(0, PROLOGUE);
    }
    args
}

pub(crate) async fn spawn_child_async(request: SpawnChildRequest<'_>) -> std::io::Result<Child> {
    let SpawnChildRequest {
        program,
        args,
        arg0,
        cwd,
        network_sandbox_policy,
        network,
        stdio_policy,
        mut env,
    } = request;

    trace!(
        "spawn_child_async: {program:?} {args:?} {arg0:?} {cwd:?} {network_sandbox_policy:?} {stdio_policy:?} {env:?}"
    );

    let args = with_powershell_utf8_piping(&program, args);

    let mut cmd = Command::new(&program);
    #[cfg(unix)]
    cmd.arg0(arg0.map_or_else(|| program.to_string_lossy().to_string(), String::from));
    cmd.args(args);
    cmd.current_dir(cwd);
    if let Some(network) = network {
        network.apply_to_env(&mut env);
    }
    cmd.env_clear();
    cmd.envs(env);

    if !network_sandbox_policy.is_enabled() {
        cmd.env(CODEX_SANDBOX_NETWORK_DISABLED_ENV_VAR, "1");
    }

    // If this Codex process dies (including being killed via SIGKILL), we want
    // any child processes that were spawned as part of a `"shell"` tool call
    // to also be terminated.

    #[cfg(unix)]
    unsafe {
        let detach_from_tty = matches!(stdio_policy, StdioPolicy::RedirectForShellTool);
        #[cfg(target_os = "linux")]
        let parent_pid = libc::getpid();
        cmd.pre_exec(move || {
            if detach_from_tty {
                codex_utils_pty::process_group::detach_from_tty()?;
            }

            // This relies on prctl(2), so it only works on Linux.
            #[cfg(target_os = "linux")]
            {
                // This prctl call effectively requests, "deliver SIGTERM when my
                // current parent dies."
                codex_utils_pty::process_group::set_parent_death_signal(parent_pid)?;
            }
            Ok(())
        });
    }

    match stdio_policy {
        StdioPolicy::RedirectForShellTool => {
            // Do not create a file descriptor for stdin because otherwise some
            // commands may hang forever waiting for input. For example, ripgrep has
            // a heuristic where it may try to read from stdin as explained here:
            // https://github.com/BurntSushi/ripgrep/blob/e2362d4d5185d02fa857bf381e7bd52e66fafc73/crates/core/flags/hiargs.rs#L1101-L1103
            cmd.stdin(Stdio::null());

            cmd.stdout(Stdio::piped()).stderr(Stdio::piped());
        }
        StdioPolicy::Inherit => {
            // Inherit stdin, stdout, and stderr from the parent process.
            cmd.stdin(Stdio::inherit())
                .stdout(Stdio::inherit())
                .stderr(Stdio::inherit());
        }
    }

    cmd.kill_on_drop(true).spawn()
}

#[cfg(test)]
mod powershell_utf8_tests {
    use super::with_powershell_utf8_piping;
    use std::path::Path;

    fn args(items: &[&str]) -> Vec<String> {
        items.iter().map(|item| (*item).to_string()).collect()
    }

    #[test]
    fn prefixes_inline_powershell_commands() {
        // The command a user actually loses data on: a pipe carrying non-ASCII text.
        let command = "echo '[{\"text\":\"中文\"}]' | officecli batch deck.pptx --json";
        let out = with_powershell_utf8_piping(
            Path::new(r"C:\Windows\System32\WindowsPowerShell\v1.0\powershell.exe"),
            args(&["-NoProfile", "-Command", command]),
        );

        assert_eq!(out[..2], args(&["-NoProfile", "-Command"])[..]);
        let assignment = out[2]
            .find("$OutputEncoding")
            .expect("the assignment must be present");
        let user_command = out[2].find(command).expect("the command must survive");
        // Order is the whole point: PowerShell pipes as US-ASCII until the variable is assigned.
        assert!(
            assignment < user_command,
            "assignment must come first: {}",
            out[2]
        );
        // Specifically the console's *input* encoding — the one the receiving program decodes
        // stdin with. Hard-coding UTF-8 here reintroduces the bug in a quieter form: UTF-8 bytes
        // arriving at a code-page-936 console become mojibake rather than `?`.
        assert!(
            out[2].contains("[Console]::InputEncoding"),
            "must target the encoding the receiving end decodes with: {}",
            out[2],
        );
    }

    #[test]
    fn leaves_everything_else_alone() {
        // pwsh already defaults to UTF-8, but prefixing is harmless and keeps one code path.
        let pwsh = with_powershell_utf8_piping(
            Path::new("pwsh.exe"),
            args(&["-NoProfile", "-Command", "ls"]),
        );
        assert!(pwsh[2].contains("$OutputEncoding"));

        // A script file carries its own encoding; rewriting its path would corrupt the invocation.
        let file = with_powershell_utf8_piping(
            Path::new("powershell.exe"),
            args(&["-NoProfile", "-File", "run.ps1"]),
        );
        assert_eq!(file, args(&["-NoProfile", "-File", "run.ps1"]));

        // Non-PowerShell programs must be untouched — this is PowerShell syntax.
        let bash = with_powershell_utf8_piping(
            Path::new("/bin/bash"),
            args(&["-lc", "echo '中文' | tool"]),
        );
        assert_eq!(bash, args(&["-lc", "echo '中文' | tool"]));

        // A bare `-Command` with nothing after it must not panic.
        let dangling =
            with_powershell_utf8_piping(Path::new("powershell.exe"), args(&["-Command"]));
        assert_eq!(dangling, args(&["-Command"]));
    }
}
