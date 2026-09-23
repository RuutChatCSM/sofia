//! TUI-owned Windows sandbox helpers retained while setup still runs in the local client process.
//!
//! TODO: These helpers inspect and modify the TUI host, so they do not support
//! cross-platform remote app servers. Move readiness and setup to the existing
//! `windowsSandbox/*` RPCs while preserving the pending permission profile,
//! use the server platform reported during initialization, and add a remote
//! equivalent for read-root grants.

use crate::legacy_core::config::Config;
use sofia_config::types::WindowsSandboxModeToml;
use sofia_features::Feature;
#[cfg(target_os = "windows")]
use sofia_otel::SessionTelemetry;
use sofia_protocol::config_types::WindowsSandboxLevel;
#[cfg(target_os = "windows")]
use sofia_protocol::models::PermissionProfile;
#[cfg(target_os = "windows")]
use sofia_utils_absolute_path::AbsolutePathBuf;
#[cfg(target_os = "windows")]
use std::collections::HashMap;
use std::path::Path;
#[cfg(target_os = "windows")]
use std::path::PathBuf;

#[cfg(target_os = "windows")]
pub(crate) fn record_world_writable_scan_result(
    session_telemetry: &SessionTelemetry,
    result: &anyhow::Result<usize>,
) {
    let (flagged_count, result) = match result {
        Ok(flagged_count) => (*flagged_count as i64, "success"),
        Err(_) => (0, "error"),
    };
    session_telemetry.histogram(
        "sofia.windows_sandbox.world_writable_scan_flagged_directories",
        flagged_count,
        &[("result", result)],
    );
}

pub(crate) fn level_from_config(config: &Config) -> WindowsSandboxLevel {
    match config.permissions.windows_sandbox_mode {
        Some(WindowsSandboxModeToml::Elevated) => WindowsSandboxLevel::Elevated,
        Some(WindowsSandboxModeToml::Unelevated) => WindowsSandboxLevel::RestrictedToken,
        None if config.features.enabled(Feature::WindowsSandboxElevated) => {
            WindowsSandboxLevel::Elevated
        }
        None if config.features.enabled(Feature::WindowsSandbox) => {
            WindowsSandboxLevel::RestrictedToken
        }
        None => WindowsSandboxLevel::Disabled,
    }
}

#[cfg(target_os = "windows")]
pub(crate) use sofia_windows_sandbox::sandbox_setup_is_complete;

#[cfg(not(target_os = "windows"))]
pub(crate) fn sandbox_setup_is_complete(_codex_home: &Path) -> bool {
    false
}

#[cfg(target_os = "windows")]
pub(crate) fn prepare_elevated_sandbox(
    permission_profile: &PermissionProfile,
    workspace_roots: &[AbsolutePathBuf],
    command_cwd: &Path,
    env_map: &HashMap<String, String>,
    sofia_home: &Path,
) -> anyhow::Result<()> {
    if !sandbox_setup_is_complete(sofia_home) {
        let permissions = sofia_windows_sandbox::ResolvedWindowsSandboxPermissions::try_from_permission_profile_for_workspace_roots(
            permission_profile,
            workspace_roots,
        )?;
        sofia_windows_sandbox::run_elevated_setup(sofia_windows_sandbox::SandboxSetupRequest {
            permissions: &permissions,
            command_cwd,
            env_map,
            sofia_home,
            proxy_enforced: false,
        })?;
    }
    sofia_windows_sandbox::run_setup_refresh(
        permission_profile,
        workspace_roots,
        command_cwd,
        env_map,
        sofia_home,
        /*proxy_enforced*/ false,
    )
}

#[cfg(target_os = "windows")]
pub(crate) fn elevated_setup_failure_details(err: &anyhow::Error) -> Option<(String, String)> {
    let failure = sofia_windows_sandbox::extract_setup_failure(err)?;
    Some((
        failure.code.as_str().to_string(),
        sofia_windows_sandbox::sanitize_setup_metric_tag_value(&failure.message),
    ))
}

#[cfg(target_os = "windows")]
pub(crate) fn elevated_setup_failure_metric_name(err: &anyhow::Error) -> &'static str {
    if sofia_windows_sandbox::extract_setup_failure(err).is_some_and(|failure| {
        matches!(
            failure.code,
            sofia_windows_sandbox::SetupErrorCode::OrchestratorHelperLaunchCanceled
        )
    }) {
        "sofia.windows_sandbox.elevated_setup_canceled"
    } else {
        "sofia.windows_sandbox.elevated_setup_failure"
    }
}

#[cfg(target_os = "windows")]
pub(crate) fn grant_read_root_non_elevated(
    permission_profile: &PermissionProfile,
    workspace_roots: &[AbsolutePathBuf],
    command_cwd: &Path,
    env_map: &HashMap<String, String>,
    sofia_home: &Path,
    read_root: &Path,
) -> anyhow::Result<PathBuf> {
    if !read_root.is_absolute() {
        anyhow::bail!("path must be absolute: {}", read_root.display());
    }
    if !read_root.exists() {
        anyhow::bail!("path does not exist: {}", read_root.display());
    }
    if !read_root.is_dir() {
        anyhow::bail!("path must be a directory: {}", read_root.display());
    }

    let canonical_root = dunce::canonicalize(read_root)?;
    sofia_windows_sandbox::run_setup_refresh_with_extra_read_roots(
        permission_profile,
        workspace_roots,
        command_cwd,
        env_map,
        sofia_home,
        vec![canonical_root.clone()],
        /*proxy_enforced*/ false,
    )?;
    Ok(canonical_root)
}
