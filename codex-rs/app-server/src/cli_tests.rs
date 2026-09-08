use super::parse_process_mcp_server_replacements;
use pretty_assertions::assert_eq;
use std::collections::BTreeMap;

#[test]
fn process_mcp_replacements_parse_complete_tables_and_reject_malformed_inputs() {
    let replacements = parse_process_mcp_server_replacements(vec![
        "browser={ command = 'old', args = ['stale'] }".to_string(),
        "browser={ command = 'current' }".to_string(),
        "other={ url = 'https://example.invalid/mcp' }".to_string(),
    ])
    .unwrap();
    assert_eq!(
        replacements,
        BTreeMap::from([
            (
                "browser".to_string(),
                toml::toml! { command = "current" }.into()
            ),
            (
                "other".to_string(),
                toml::toml! { url = "https://example.invalid/mcp" }.into()
            ),
        ])
    );
    for raw in [
        "browser",
        "browser=42",
        "browser.command='secret-fixture'",
        "browser={",
    ] {
        let error = parse_process_mcp_server_replacements(vec![raw.to_string()]).unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::InvalidInput);
        assert!(!error.to_string().contains("secret-fixture"));
    }
}
