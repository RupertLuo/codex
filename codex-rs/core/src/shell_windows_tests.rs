use pretty_assertions::assert_eq;

use super::PowerShellProfileMode;
use super::derive_powershell_exec_args;

#[test]
fn standard_powershell_invocation_preserves_existing_profile_behavior() {
    assert_eq!(
        derive_powershell_exec_args(
            "pwsh.exe",
            "echo hello",
            /*use_login_shell*/ false,
            PowerShellProfileMode::Standard,
        ),
        vec!["pwsh.exe", "-NoProfile", "-Command", "echo hello"]
    );
    assert_eq!(
        derive_powershell_exec_args(
            "pwsh.exe",
            "echo hello",
            /*use_login_shell*/ true,
            PowerShellProfileMode::Standard,
        ),
        vec!["pwsh.exe", "-Command", "echo hello"]
    );
}

#[test]
fn catalyst_powershell_invocation_loads_only_the_managed_profile() {
    let expected_script = concat!(
        "if ([string]::IsNullOrWhiteSpace($env:CATALYST_POWERSHELL_PROFILE) ",
        "-or -not (Test-Path -LiteralPath $env:CATALYST_POWERSHELL_PROFILE -PathType Leaf)) ",
        "{ throw 'Catalyst PowerShell profile is unavailable.' }; ",
        ". $env:CATALYST_POWERSHELL_PROFILE; echo hello",
    );

    for use_login_shell in [false, true] {
        assert_eq!(
            derive_powershell_exec_args(
                "pwsh.exe",
                "echo hello",
                use_login_shell,
                PowerShellProfileMode::Catalyst,
            ),
            vec!["pwsh.exe", "-NoProfile", "-Command", expected_script]
        );
    }
    assert!(expected_script.is_ascii());
}
