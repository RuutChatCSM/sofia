use anyhow::Result;
use predicates::str::contains;
use pretty_assertions::assert_eq;
use serde_json::json;
use sofia_core_plugins::installed_marketplaces::marketplace_install_root;
use sofia_utils_absolute_path::AbsolutePathBuf;
use std::path::Path;
use tempfile::TempDir;

fn sofia_command(sofia_home: &Path) -> Result<assert_cmd::Command> {
    let mut cmd = assert_cmd::Command::new(sofia_utils_cargo_bin::cargo_bin("sofia")?);
    cmd.env("SOFIA_HOME", sofia_home);
    Ok(cmd)
}

#[tokio::test]
async fn marketplace_upgrade_runs_under_plugin() -> Result<()> {
    let sofia_home = TempDir::new()?;

    sofia_command(sofia_home.path())?
        .args(["plugin", "marketplace", "upgrade"])
        .assert()
        .success()
        .stdout(contains("No configured Git marketplaces to upgrade."));

    Ok(())
}

#[tokio::test]
async fn marketplace_upgrade_json_prints_upgrade_outcome() -> Result<()> {
    let sofia_home = TempDir::new()?;
    let source = TempDir::new()?;
    let manifest = r#"{"name":"debug","plugins":[]}"#;
    std::fs::create_dir_all(source.path().join(".agents/plugins"))?;
    std::fs::write(
        source.path().join(".agents/plugins/marketplace.json"),
        manifest,
    )?;
    for args in [
        vec!["init", "--quiet"],
        vec!["add", "."],
        vec![
            "-c",
            "user.name=Sofia Tests",
            "-c",
            "user.email=sofia@example.com",
            "commit",
            "-m",
            "marketplace",
        ],
    ] {
        let output = std::process::Command::new("git")
            .current_dir(source.path())
            .args(args)
            .output()?;
        anyhow::ensure!(
            output.status.success(),
            "git fixture failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    let source_arg = serde_json::to_string(&source.path().to_string_lossy())?;
    let marketplace_override =
        format!("marketplaces.debug={{source_type=\"git\",source={source_arg}}}");

    let assert = sofia_command(sofia_home.path())?
        .current_dir(sofia_home.path())
        .args(["--config", &marketplace_override])
        .args(["plugin", "marketplace", "upgrade", "--json"])
        .assert()
        .success();
    let stdout = assert.get_output().stdout.as_slice();
    let actual: serde_json::Value = serde_json::from_slice(stdout)?;
    let installed_root = AbsolutePathBuf::try_from(
        marketplace_install_root(&sofia_home.path().canonicalize()?).join("debug"),
    )?;

    assert_eq!(
        actual,
        json!({
            "selectedMarketplaces": ["debug"],
            "upgradedRoots": [installed_root],
            "errors": [],
        })
    );
    assert_eq!(
        std::fs::read_to_string(installed_root.join(".agents/plugins/marketplace.json"))?,
        manifest
    );
    assert!(!sofia_home.path().join("config.toml").exists());

    Ok(())
}

#[tokio::test]
async fn marketplace_upgrade_no_longer_runs_at_top_level() -> Result<()> {
    let sofia_home = TempDir::new()?;

    sofia_command(sofia_home.path())?
        .args(["marketplace", "upgrade"])
        .assert()
        .failure()
        .stderr(contains("unrecognized subcommand 'upgrade'"));

    Ok(())
}
