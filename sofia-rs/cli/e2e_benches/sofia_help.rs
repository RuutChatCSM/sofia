#![allow(clippy::expect_used)]

use std::process::Command;

use divan::Bencher;

fn main() {
    divan::main();
}

/// Exercises the Bazel-backed end-to-end benchmark path with a cheap,
/// deterministic Sofia invocation. Richer scenarios can add separate
/// benchmark binaries without making the shared harness depend on them.
#[divan::bench(sample_count = 20, sample_size = 1)]
fn sofia_help(bencher: Bencher) {
    let sofia = sofia_utils_cargo_bin::cargo_bin("sofia")
        .expect("sofia binary should be available through Bazel runfiles");

    bencher.bench_local(move || {
        let output = Command::new(&sofia)
            .arg("--help")
            .output()
            .expect("sofia --help should run");
        assert!(output.status.success(), "sofia --help should succeed");
    });
}
