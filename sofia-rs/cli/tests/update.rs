use anyhow::Result;
use predicates::str::contains;
use std::path::Path;
use tempfile::TempDir;

fn sofia_command(sofia_home: &Path) -> Result<assert_cmd::Command> {
    let mut cmd = assert_cmd::Command::new(sofia_utils_cargo_bin::cargo_bin("sofia")?);
    cmd.env("SOFIA_HOME", sofia_home);
    Ok(cmd)
}

#[cfg(debug_assertions)]
#[tokio::test]
async fn update_does_not_start_interactive_prompt() -> Result<()> {
    let sofia_home = TempDir::new()?;

    sofia_command(sofia_home.path())?
        .arg("update")
        .assert()
        .failure()
        .stderr(contains("`sofia update` is not available in debug builds"));

    Ok(())
}
