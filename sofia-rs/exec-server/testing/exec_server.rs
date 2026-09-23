//! Minimal exec-server fixture for Bazel-only integration tests.
//!
//! Linking only exec-server avoids depending on the full Sofia CLI binary
//! when a test only needs a WebSocket executor endpoint. It handles the arg0
//! helper mode because sandboxed process requests re-exec this binary.

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
            HttpClientFactory::new(OutboundProxyPolicy::ReqwestDefault),
        ))
}
