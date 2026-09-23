use std::sync::Arc;

use pretty_assertions::assert_eq;
use sofia_config::McpServerTransportConfig;
use sofia_core::McpManager;
use sofia_core::config::Config;
use sofia_core::config::ConfigBuilder;
use sofia_core::plugins_manager_for_config;
use sofia_extension_api::ExtensionRegistryBuilder;
use sofia_extension_api::McpServerContribution;
use sofia_extension_api::McpServerContributionContext;
use sofia_extension_api::McpServerContributor;
use sofia_login::AuthManager;
use sofia_login::CodexAuth;
use sofia_login::test_support::auth_manager_from_optional_auth;
use sofia_mcp::SOFIA_APPS_MCP_SERVER_NAME;

type TestResult = Result<(), Box<dyn std::error::Error>>;

#[tokio::test]
async fn contributes_hosted_plugin_runtime_without_an_executor() -> TestResult {
    let sofia_home = tempfile::tempdir()?;
    let config = ConfigBuilder::default()
        .sofia_home(sofia_home.path().to_path_buf())
        .fallback_cwd(Some(sofia_home.path().to_path_buf()))
        .cli_overrides(vec![
            ("features.apps".to_string(), true.into()),
            ("chatgpt_base_url".to_string(), "https://chatgpt.com".into()),
        ])
        .build()
        .await?;
    let auth = CodexAuth::create_dummy_chatgpt_auth_for_testing();
    let manager = installed_manager(&config, Some(auth.clone()));

    let servers = manager.effective_servers(&config, Some(&auth)).await;
    let server = servers
        .get(SOFIA_APPS_MCP_SERVER_NAME)
        .ok_or("hosted plugin runtime should be contributed as a configured server")?
        .config();
    let McpServerTransportConfig::StreamableHttp { url, .. } = &server.transport else {
        panic!("hosted plugin runtime should use streamable HTTP");
    };
    assert_eq!(url, "https://chatgpt.com/backend-api/ps/mcp");

    Ok(())
}

#[tokio::test]
async fn runtime_overlay_preserves_disabled_server() -> TestResult {
    let sofia_home = tempfile::tempdir()?;
    let config = ConfigBuilder::default()
        .sofia_home(sofia_home.path().to_path_buf())
        .fallback_cwd(Some(sofia_home.path().to_path_buf()))
        .cli_overrides(vec![
            ("features.apps".to_string(), true.into()),
            (
                "mcp_servers.sofia_apps.url".to_string(),
                "https://example.com/mcp".into(),
            ),
            ("mcp_servers.sofia_apps.enabled".to_string(), false.into()),
        ])
        .build()
        .await?;
    let auth = CodexAuth::create_dummy_chatgpt_auth_for_testing();
    let manager = installed_manager(&config, Some(auth.clone()));

    let servers = manager.effective_servers(&config, Some(&auth)).await;
    let server = servers
        .get(SOFIA_APPS_MCP_SERVER_NAME)
        .ok_or("hosted plugin runtime should remain configured")?;

    assert!(!server.enabled());
    Ok(())
}

#[tokio::test]
async fn default_fallback_overwrites_reserved_config_without_an_extension() -> TestResult {
    let sofia_home = tempfile::tempdir()?;
    let config = ConfigBuilder::default()
        .sofia_home(sofia_home.path().to_path_buf())
        .fallback_cwd(Some(sofia_home.path().to_path_buf()))
        .cli_overrides(vec![
            ("features.apps".to_string(), true.into()),
            (
                "mcp_servers.sofia_apps.url".to_string(),
                "https://example.com/mcp".into(),
            ),
        ])
        .build()
        .await?;
    let auth = CodexAuth::create_dummy_chatgpt_auth_for_testing();
    let manager = McpManager::new(Arc::new(plugins_manager_for_config(
        &config,
        AuthManager::from_auth_for_testing(auth.clone()),
    )));

    let servers = manager.effective_servers(&config, Some(&auth)).await;
    let server = servers
        .get(SOFIA_APPS_MCP_SERVER_NAME)
        .ok_or("default Apps MCP should be present")?
        .config();
    let McpServerTransportConfig::StreamableHttp { url, .. } = &server.transport else {
        panic!("default Apps MCP should use streamable HTTP");
    };
    assert_eq!(url, "https://chatgpt.com/backend-api/ps/mcp");

    Ok(())
}

#[tokio::test]
async fn later_extension_can_remove_same_name_registration() -> TestResult {
    let sofia_home = tempfile::tempdir()?;
    let config = ConfigBuilder::default()
        .sofia_home(sofia_home.path().to_path_buf())
        .fallback_cwd(Some(sofia_home.path().to_path_buf()))
        .cli_overrides(vec![("features.apps".to_string(), true.into())])
        .build()
        .await?;
    let auth = CodexAuth::create_dummy_chatgpt_auth_for_testing();
    let mut builder = ExtensionRegistryBuilder::new();
    sofia_mcp_extension::install(&mut builder);
    builder.mcp_server_contributor(Arc::new(RemoveCodexApps));
    let manager = McpManager::new_with_extensions(
        Arc::new(plugins_manager_for_config(
            &config,
            AuthManager::from_auth_for_testing(auth.clone()),
        )),
        Arc::new(builder.build()),
        sofia_core::CodexAppsToolsCache::default(),
    );

    let servers = manager.effective_servers(&config, Some(&auth)).await;

    assert!(!servers.contains_key(SOFIA_APPS_MCP_SERVER_NAME));
    Ok(())
}

#[tokio::test]
async fn hosted_apps_mcp_requires_chatgpt_auth() -> TestResult {
    let sofia_home = tempfile::tempdir()?;
    let config = ConfigBuilder::default()
        .sofia_home(sofia_home.path().to_path_buf())
        .fallback_cwd(Some(sofia_home.path().to_path_buf()))
        .cli_overrides(vec![("features.apps".to_string(), true.into())])
        .build()
        .await?;
    let auth = CodexAuth::from_api_key("test");
    let manager = installed_manager(&config, Some(auth.clone()));

    let servers = manager.effective_servers(&config, Some(&auth)).await;
    assert!(!servers.contains_key(SOFIA_APPS_MCP_SERVER_NAME));

    Ok(())
}

#[tokio::test]
async fn disabled_apps_remove_reserved_server_config_for_all_hosts() -> TestResult {
    let sofia_home = tempfile::tempdir()?;
    let config = ConfigBuilder::default()
        .sofia_home(sofia_home.path().to_path_buf())
        .fallback_cwd(Some(sofia_home.path().to_path_buf()))
        .cli_overrides(vec![
            ("features.apps".to_string(), false.into()),
            (
                "mcp_servers.sofia_apps.url".to_string(),
                "https://example.com/mcp".into(),
            ),
        ])
        .build()
        .await?;
    let managers = [
        installed_manager(&config, /*auth*/ None),
        McpManager::new(Arc::new(plugins_manager_for_config(
            &config,
            auth_manager_from_optional_auth(/*auth*/ None),
        ))),
    ];
    for manager in managers {
        let servers = manager.runtime_servers(&config).await;
        assert!(!servers.contains_key(SOFIA_APPS_MCP_SERVER_NAME));
    }
    Ok(())
}

fn installed_manager(config: &Config, auth: Option<CodexAuth>) -> McpManager {
    let mut builder = ExtensionRegistryBuilder::new();
    sofia_mcp_extension::install(&mut builder);
    McpManager::new_with_extensions(
        Arc::new(plugins_manager_for_config(
            config,
            auth_manager_from_optional_auth(auth),
        )),
        Arc::new(builder.build()),
        sofia_core::CodexAppsToolsCache::default(),
    )
}

struct RemoveCodexApps;

impl McpServerContributor<Config> for RemoveCodexApps {
    fn id(&self) -> &'static str {
        "remove_codex_apps"
    }

    fn contribute<'a>(
        &'a self,
        _context: McpServerContributionContext<'a, Config>,
    ) -> sofia_extension_api::ExtensionFuture<'a, Vec<McpServerContribution>> {
        Box::pin(async move {
            vec![McpServerContribution::Remove {
                name: SOFIA_APPS_MCP_SERVER_NAME.to_string(),
            }]
        })
    }
}
