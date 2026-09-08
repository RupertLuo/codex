use super::SensitiveString;
use pretty_assertions::assert_eq;

#[test]
fn sensitive_string_debug_is_redacted() {
    let input = SensitiveString::new("provider-secret".to_string());

    // Keep the existing diagnostic contract while moving ownership out of TUI.
    assert_eq!(format!("{input:?}"), "SensitiveInput([REDACTED])");
    assert_eq!(input.expose_secret(), "provider-secret");
}
