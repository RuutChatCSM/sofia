---
name: remote-tests
description: Testing against remote executors in integration tests.
---

Remote executor tests exercise the app-server/exec-server split to ensure that agent features work
in both local and remote execution environments.

Remote executor tests currently require an x86_64 Linux host machine. There are two flavors:

1. Docker (Linux exec-server)
2. Wine (Windows exec-server)

## Test Fixtures

Individual test cases must opt-in to being run against a remote executor.

### sofia_core

Use `TestCodexBuilder::build_with_auto_env()` to opt-in to remote execution in core integration
tests unless the test needs more precise control over its executor.

### app-server

Start the server with `TestAppServer::new_with_auto_env()` unless the test defines its own
`$SOFIA_HOME/environments.toml` or will define custom environments at runtime.

Start threads with `TestAppServer::send_thread_start_request_with_auto_env()` if you've created the
server with the `auto_env` approach. Omit `ThreadStartParams.environments` (leave it as `None`) when
doing so.

## Test Skips

If a test doesn't pass in a particular remote executor configuration you can skip it in just that
configuration. Include a string reason for future readers when the selected skip macro supports
one.

Choose the skip macro by what causes the test to fail:

- `skip_if_target_windows!`: Windows target behavior.
- `skip_if_wine_exec!`: Wine-exec runner constraints.
- `skip_if_host_windows!`: Windows host constraints.
- `skip_if_remote!`: Local-only test behavior.
- `skip_if_no_remote_env!`: Remote-only test behavior.

Prefer defining tests that run in all host/target configurations by default. See the `$path-types`
skill for the most common changes required to make tests compatible.

## Docker

Docker container is built and initialized via ./scripts/test-remote-env.sh. Sourcing this script
in bash also provides the `sofia_remote_env_cleanup` function to use after testing.

To run core integration tests against a Docker remote executor:

```bash
bash -c '
  set -euo pipefail
  unset SOFIA_TEST_REMOTE_EXEC_SERVER_URL
  source scripts/test-remote-env.sh
  trap sofia_remote_env_cleanup EXIT

  cd sofia-rs
  just test -p sofia-core --test all
'
```

To run app-server integration tests against a Docker remote executor:

```bash
bash -c '
  set -euo pipefail
  unset SOFIA_TEST_REMOTE_EXEC_SERVER_URL
  source scripts/test-remote-env.sh
  trap sofia_remote_env_cleanup EXIT

  cd sofia-rs
  just test -p sofia-app-server --test all
'
```

## Wine

These tests build an exec-server for Windows and run it under Wine, with the app-server staying on
the Linux host. The cross-platform build dependency means they only run in Bazel.

For core integration tests:

```sh
bazel test //sofia-rs/core:core-all-wine-exec-test
```

For app-server integration tests:

```sh
bazel test //sofia-rs/app-server:app-server-all-wine-exec-test
```

## Devboxes

You can use a devbox to run these tests if you are running on a macOS machine.

You can list devboxes via `applied_devbox ls`, pick the one with `sofia` in the name.
Connect to devbox via `ssh <devbox_name>`.
Reuse the same checkout of sofia in `~/code/sofia`. Reset files if needed. Multiple checkouts take longer to build and take up more space.
Check whether the SHA and modified files are in sync between remote and local.
