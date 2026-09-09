use super::ConfigManager;
use codex_config::CloudConfigBundleLoader;
use codex_config::LoaderOverrides;
use codex_config::McpServerConfig;
use codex_core::config::ConfigOverrides;
use pretty_assertions::assert_eq;
use std::collections::BTreeMap;
use std::collections::HashMap;
use tempfile::TempDir;

#[tokio::test]
async fn process_mcp_replacement_survives_reload_and_respects_requirements() -> anyhow::Result<()> {
    let home = TempDir::new()?;
    let config_path = home.path().join("config.toml");
    let requirements_path = home.path().join("requirements.toml");
    let initial_file = "[mcp_servers.cata_browser]\nurl = 'https://stale.invalid/mcp'\nfuture_stale_field = true\n[mcp_servers.other]\ncommand = 'old-other'\n";
    std::fs::write(&config_path, initial_file)?;
    let replacement = toml::toml! { command = "process-browser" args = ["serve"] };
    let expected: McpServerConfig = toml::Value::Table(replacement.clone()).try_into()?;
    let mut loader = LoaderOverrides::without_managed_config_for_tests();
    loader.system_requirements_path = Some(requirements_path.clone());
    loader.ignore_project_config = true;
    let manager = ConfigManager::new_for_tests(
        home.path().to_path_buf(),
        Vec::new(),
        loader,
        CloudConfigBundleLoader::default(),
    )
    .with_process_mcp_server_replacements(BTreeMap::from([(
        "cata_browser".to_string(),
        replacement.into(),
    )]));
    let first = manager
        .load_latest_config(Some(home.path().to_path_buf()))
        .await?;
    assert_eq!(&first.mcp_servers.get()["cata_browser"], &expected);
    assert_eq!(std::fs::read_to_string(&config_path)?, initial_file);

    let requested = manager
        .load_with_cli_overrides(
            &[],
            Some(HashMap::from([(
                "mcp_servers.cata_browser.command".to_string(),
                serde_json::json!("request-browser"),
            )])),
            ConfigOverrides::default(),
            Some(home.path().to_path_buf()),
        )
        .await?;
    assert_eq!(&requested.mcp_servers.get()["cata_browser"], &expected);

    let refreshed_file = initial_file.replace("old-other", "new-other");
    std::fs::write(&config_path, &refreshed_file)?;
    let refreshed = manager.load_latest_config_for_thread(&first).await?;
    assert_eq!(&refreshed.mcp_servers.get()["cata_browser"], &expected);
    let other: McpServerConfig = toml::from_str("command = 'new-other'")?;
    assert_eq!(&refreshed.mcp_servers.get()["other"], &other);
    let defaults = manager.load_default_config().await?;
    assert_eq!(
        defaults.mcp_servers.get(),
        &HashMap::from([("cata_browser".to_string(), expected.clone())])
    );

    std::fs::write(
        &requirements_path,
        "[mcp_servers.cata_browser.identity]\ncommand = 'allowed-browser'\n",
    )?;
    let constrained = manager.load_latest_config_for_thread(&refreshed).await?;
    let browser = &constrained.mcp_servers.get()["cata_browser"];
    assert!(!browser.enabled);
    assert!(browser.disabled_reason.is_some());
    assert_eq!(browser.transport, expected.transport);
    assert_eq!(std::fs::read_to_string(&config_path)?, refreshed_file);
    Ok(())
}

#[tokio::test]
async fn process_mcp_replacement_cannot_bypass_typed_config_validation() -> anyhow::Result<()> {
    let home = TempDir::new()?;
    for replacement in [
        toml::Value::Integer(42),
        toml::toml! { command = "browser" url = "https://conflicting.invalid/mcp" }.into(),
    ] {
        let manager = ConfigManager::without_managed_config_for_tests(home.path().to_path_buf())
            .with_process_mcp_server_replacements(BTreeMap::from([(
                "browser".to_string(),
                replacement,
            )]));
        let error = manager
            .load_latest_config(Some(home.path().to_path_buf()))
            .await
            .unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::InvalidData);
    }
    Ok(())
}
