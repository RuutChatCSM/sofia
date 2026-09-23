//! Cargo entry point for the minimal exec-server integration-test fixture.
//!
//! This mirrors `//sofia-rs/exec-server/testing:exec-server` so Cargo-backed
//! app-server integration tests can receive `CARGO_BIN_EXE_exec-server`. It
//! also handles the helper argv modes because exec-server re-execs
//! `sofia_self_exe` for sandboxed filesystem and process requests.

use sofia_exec_server::ExecServerRuntimePaths;
use sofia_http_client::HttpClientFactory;
use sofia_http_client::OutboundProxyPolicy;
use std::ffi::OsStr;

const SOFIA_LINUX_SANDBOX_EXE_ENV_VAR: &str = "SOFIA_TEST_LINUX_SANDBOX_EXE";

fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let mut args = std::env::args_os();
    let _ = args.next();
    let argv1 = args.next();
    #[cfg(unix)]
    if argv1.as_deref() == Some(OsStr::new(sofia_exec_server::SOFIA_ARG0_EXEC_HELPER_ARG1)) {
        sofia_exec_server::run_arg0_exec_helper_main();
    }
    if argv1.as_deref() == Some(OsStr::new(sofia_exec_server::SOFIA_FS_HELPER_ARG1)) {
        sofia_exec_server::run_fs_helper_main();
    }

    let current_exe = std::env::current_exe()?;
    let sofia_linux_sandbox_exe =
        std::env::var_os(SOFIA_LINUX_SANDBOX_EXE_ENV_VAR).map(std::path::PathBuf::from);
    let runtime_paths = ExecServerRuntimePaths::new(current_exe, sofia_linux_sandbox_exe)?;
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?
        .block_on(sofia_exec_server::run_main(
            "ws://127.0.0.1:0",
            runtime_paths,
            // This test-only fixture has no application configuration to resolve HTTP policy.
            HttpClientFactory::new(OutboundProxyPolicy::ReqwestDefault),
        ))
}
