use clap::Parser;
use codex_app_server::AppServerCli;

#[test]
fn app_server_accepts_cli_config_overrides() {
    AppServerCli::try_parse_from([
        "codex-app-server",
        "-c",
        "model=\"gpt-5-codex\"",
        "--config",
        "sandbox_mode=\"read-only\"",
        "--listen",
        "off",
    ])
    .expect("parse app-server args");
}

#[test]
fn app_server_accepts_websocket_auth_and_session_source_flags() {
    AppServerCli::try_parse_from([
        "codex-app-server",
        "--listen",
        "ws://127.0.0.1:4500",
        "--session-source",
        "catalyst",
        "--ws-auth",
        "capability-token",
        "--ws-token-sha256",
        "abababababababababababababababababababababababababababababababab",
        "--strict-config",
        "--remote-control",
    ])
    .expect("parse WebSocket auth and session source flags");
}
