use clap::Parser;
use codex_app_server::AppServerCli;
use pretty_assertions::assert_eq;

#[test]
fn app_server_accepts_cli_config_overrides() {
    let args = AppServerCli::try_parse_from([
        "codex-app-server",
        "-c",
        "model=\"gpt-5-codex\"",
        "--config",
        "sandbox_mode=\"read-only\"",
        "--listen",
        "off",
    ])
    .expect("parse app-server args");

    assert_eq!(
        args.serve.raw_config_overrides(),
        ["model=\"gpt-5-codex\"", "sandbox_mode=\"read-only\"",]
    );
}

#[test]
fn app_server_accepts_process_mcp_server_replacement() {
    let replacement =
        r#"cata_browser={ command = "/app/node", args = ["/app/browser.mjs"], required = true }"#;
    let args = AppServerCli::try_parse_from([
        "codex-app-server",
        "--process-mcp-server",
        replacement,
        "--listen",
        "off",
    ])
    .expect("parse app-server args");

    assert_eq!(args.serve.raw_process_mcp_servers(), [replacement]);
}

#[test]
fn app_server_accepts_process_scoped_code_mode_host() {
    let args = AppServerCli::try_parse_from([
        "codex-app-server",
        "--code-mode-host",
        "wss://example.test/code-mode",
        "--listen",
        "off",
    ])
    .expect("parse app-server args");

    assert!(args.serve.raw_config_overrides().is_empty());
}

#[test]
fn app_server_accepts_process_scoped_grpc_code_mode_host() {
    let args = AppServerCli::try_parse_from([
        "codex-app-server",
        "--code-mode-host",
        "https://example.test",
        "--listen",
        "off",
    ])
    .expect("parse gRPC app-server args");

    assert!(args.serve.raw_config_overrides().is_empty());
}

#[test]
fn app_server_rejects_invalid_code_mode_host() {
    for endpoint in [
        "ftp://127.0.0.1:8765",
        "ws://",
        "wss://example.test/code-mode#fragment",
        "https://example.test/code-mode",
        "http://alice:secret@example.test",
        "https://alice:secret@example.test",
        "http://example.test/?token=secret",
    ] {
        let error =
            AppServerCli::try_parse_from(["codex-app-server", "--code-mode-host", endpoint])
                .expect_err("invalid code-mode host endpoint should fail startup argument parsing");

        assert_eq!(error.kind(), clap::error::ErrorKind::ValueValidation);
        let rendered_error = error.to_string();
        assert!(!rendered_error.contains("alice"));
        assert!(!rendered_error.contains("secret"));
    }
}
