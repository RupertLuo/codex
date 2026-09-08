use crate::AppServerCodeModeHostArgs;
use crate::AppServerProcessOverrides;
use crate::AppServerRuntimeOptions;
use crate::AppServerTransport;
use crate::AppServerWebsocketAuthArgs;
#[cfg(debug_assertions)]
use crate::PluginStartupTasks;
use crate::RemoteControlStartupMode;
use crate::run_main_with_transport_options_and_overrides;
use codex_arg0::Arg0DispatchPaths;
use codex_config::LoaderOverrides;
use codex_protocol::protocol::SessionSource;
use codex_utils_cli::CliConfigOverrides;
use std::collections::BTreeMap;
use std::io;
use std::path::PathBuf;
use toml::Value as TomlValue;

#[cfg(debug_assertions)]
const MANAGED_CONFIG_PATH_ENV_VAR: &str = "CODEX_APP_SERVER_MANAGED_CONFIG_PATH";
#[cfg(debug_assertions)]
const DISABLE_MANAGED_CONFIG_ENV_VAR: &str = "CODEX_APP_SERVER_DISABLE_MANAGED_CONFIG";

#[derive(Clone, Debug, clap::Args)]
pub struct AppServerServeArgs {
    #[command(flatten)]
    config_overrides: CliConfigOverrides,

    #[arg(
        long = "process-mcp-server",
        value_name = "name=configuration",
        action = clap::ArgAction::Append,
        hide = true
    )]
    process_mcp_servers: Vec<String>,

    #[command(flatten)]
    code_mode_host: AppServerCodeModeHostArgs,

    #[arg(
        long = "listen",
        value_name = "URL",
        default_value = AppServerTransport::DEFAULT_LISTEN_URL
    )]
    listen: AppServerTransport,

    #[arg(
        long = "session-source",
        value_name = "SOURCE",
        default_value = "vscode",
        value_parser = SessionSource::from_startup_arg
    )]
    session_source: SessionSource,

    #[command(flatten)]
    auth: AppServerWebsocketAuthArgs,

    #[arg(long = "strict-config", default_value_t = false)]
    strict_config: bool,

    #[cfg(debug_assertions)]
    #[arg(long = "disable-plugin-startup-tasks-for-tests", hide = true)]
    disable_plugin_startup_tasks_for_tests: bool,

    #[arg(long = "remote-control", hide = true)]
    remote_control: bool,
}

#[derive(Debug, clap::Parser)]
#[command(version)]
pub struct AppServerCli {
    #[command(flatten)]
    pub serve: AppServerServeArgs,
}

impl AppServerServeArgs {
    pub fn raw_config_overrides(&self) -> &[String] {
        &self.config_overrides.raw_overrides
    }

    pub fn prepend_config_overrides(&mut self, values: impl IntoIterator<Item = String>) {
        self.config_overrides
            .raw_overrides
            .splice(0..0, values.into_iter().collect::<Vec<_>>());
    }

    pub fn raw_process_mcp_servers(&self) -> &[String] {
        &self.process_mcp_servers
    }
}

pub async fn run_app_server_serve(
    args: AppServerServeArgs,
    arg0_paths: Arg0DispatchPaths,
    remote_control_disabled: bool,
    process_overrides: AppServerProcessOverrides,
) -> anyhow::Result<()> {
    let AppServerServeArgs {
        config_overrides,
        process_mcp_servers,
        code_mode_host,
        listen,
        session_source,
        auth,
        strict_config,
        #[cfg(debug_assertions)]
        disable_plugin_startup_tasks_for_tests,
        remote_control,
    } = args;
    let process_overrides = process_overrides
        .with_mcp_server_replacements(parse_process_mcp_server_replacements(process_mcp_servers)?);
    let loader_overrides = if disable_managed_config_from_debug_env() {
        LoaderOverrides::without_managed_config_for_tests()
    } else {
        managed_config_path_from_debug_env()
            .map(LoaderOverrides::with_managed_config_path_for_tests)
            .unwrap_or_default()
    };
    let auth = auth.try_into_settings()?;
    let mut runtime_options = AppServerRuntimeOptions {
        code_mode_host_transport: code_mode_host.into(),
        ..Default::default()
    };
    #[cfg(debug_assertions)]
    if disable_plugin_startup_tasks_for_tests {
        runtime_options.plugin_startup_tasks = PluginStartupTasks::Skip;
    }
    runtime_options.remote_control_startup_mode = match (remote_control, remote_control_disabled) {
        (true, _) => RemoteControlStartupMode::EnabledEphemeral,
        (false, true) => RemoteControlStartupMode::DisabledEphemeral,
        (false, false) => RemoteControlStartupMode::ResolvePersisted,
    };

    run_main_with_transport_options_and_overrides(
        arg0_paths,
        config_overrides,
        loader_overrides,
        strict_config,
        /*default_analytics_enabled*/ false,
        listen,
        session_source,
        auth,
        runtime_options,
        process_overrides,
    )
    .await?;
    Ok(())
}

fn parse_process_mcp_server_replacements(
    raw_replacements: Vec<String>,
) -> io::Result<BTreeMap<String, TomlValue>> {
    let replacements = CliConfigOverrides {
        raw_overrides: raw_replacements,
    }
    .parse_overrides()
    .map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "invalid --process-mcp-server replacement",
        )
    })?;
    let mut parsed = BTreeMap::new();
    for (name, value) in replacements {
        if name.contains('.') || !value.is_table() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "--process-mcp-server requires a server name and complete TOML table",
            ));
        }
        parsed.insert(name, value);
    }
    Ok(parsed)
}

fn disable_managed_config_from_debug_env() -> bool {
    #[cfg(debug_assertions)]
    if let Ok(value) = std::env::var(DISABLE_MANAGED_CONFIG_ENV_VAR) {
        return matches!(value.as_str(), "1" | "true" | "TRUE" | "yes" | "YES");
    }
    false
}

fn managed_config_path_from_debug_env() -> Option<PathBuf> {
    #[cfg(debug_assertions)]
    if let Ok(value) = std::env::var(MANAGED_CONFIG_PATH_ENV_VAR) {
        return (!value.is_empty()).then(|| PathBuf::from(value));
    }
    None
}

#[cfg(test)]
#[path = "cli_tests.rs"]
mod tests;
