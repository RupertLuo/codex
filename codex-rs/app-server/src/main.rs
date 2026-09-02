use clap::Parser;
use codex_app_server::AppServerCli;
use codex_app_server::AppServerProcessOverrides;
use codex_app_server::run_app_server_serve;
use codex_arg0::Arg0DispatchPaths;
use codex_arg0::arg0_dispatch_or_else;

fn main() -> anyhow::Result<()> {
    let remote_control_disabled = codex_app_server::take_remote_control_disabled_env();
    arg0_dispatch_or_else(move |arg0_paths: Arg0DispatchPaths| async move {
        run_app_server_serve(
            AppServerCli::parse().serve,
            arg0_paths,
            remote_control_disabled,
            AppServerProcessOverrides::default(),
        )
        .await
    })
}

#[cfg(test)]
#[path = "main_tests.rs"]
mod tests;
