//! Entry-point for the `sofia-exec` binary.
//!
//! When this CLI is invoked normally, it parses the standard `sofia-exec` CLI
//! options and launches the non-interactive Sofia agent. However, if it is
//! invoked with arg0 as `sofia-linux-sandbox`, we instead treat the invocation
//! as a request to run the logic for the standalone `sofia-linux-sandbox`
//! executable (i.e., parse any -s args and then run a *sandboxed* command under
//! Landlock + seccomp.
//!
//! This allows us to ship a completely separate set of functionality as part
//! of the `sofia-exec` binary.
use clap::Parser;
use sofia_arg0::Arg0DispatchPaths;
use sofia_arg0::arg0_dispatch_or_else;
use sofia_exec::Cli;
use sofia_exec::run_main;
use sofia_utils_cli::CliConfigOverrides;

#[derive(Parser, Debug)]
struct TopCli {
    #[clap(flatten)]
    config_overrides: CliConfigOverrides,

    #[clap(flatten)]
    inner: Cli,
}

fn main() -> anyhow::Result<()> {
    arg0_dispatch_or_else(|arg0_paths: Arg0DispatchPaths| async move {
        let top_cli = TopCli::parse();
        // Merge root-level overrides into inner CLI struct so downstream logic remains unchanged.
        let mut inner = top_cli.inner;
        inner
            .config_overrides
            .prepend_root_overrides(top_cli.config_overrides);

        run_main(inner, arg0_paths).await?;
        Ok(())
    })
}

#[cfg(test)]
#[path = "main_tests.rs"]
mod tests;
