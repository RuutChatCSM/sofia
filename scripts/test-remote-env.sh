#!/usr/bin/env bash

# Remote-env setup script for sofia-rs integration tests.
#
# Usage (source-only):
#   source scripts/test-remote-env.sh
#   cd sofia-rs
#   just test -p sofia-core --test all remote_test_env_can_connect_and_use_filesystem
#   sofia_remote_env_cleanup

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"

is_sourced() {
  [[ "${BASH_SOURCE[0]}" != "$0" ]]
}

setup_remote_env() {
  local container_name
  local sofia_binary_path
  local container_ip
  local remote_codex_path
  local remote_exec_server_pid
  local remote_exec_server_port
  local remote_exec_server_stdout_path

  container_name="${SOFIA_TEST_REMOTE_ENV_CONTAINER_NAME:-sofia-remote-test-env-local-$(date +%s)-${RANDOM}}"
  sofia_binary_path="${CARGO_TARGET_DIR:-${REPO_ROOT}/sofia-rs/target}/debug/sofia"

  if ! command -v docker >/dev/null 2>&1; then
    echo "docker is required (Colima or Docker Desktop)" >&2
    return 1
  fi

  if ! docker info >/dev/null 2>&1; then
    echo "docker daemon is not reachable; for Colima run: colima start" >&2
    return 1
  fi

  if ! command -v cargo >/dev/null 2>&1; then
    echo "cargo is required to build sofia" >&2
    return 1
  fi

  (
    cd "${REPO_ROOT}/sofia-rs"
    cargo build -p sofia-cli --bin sofia
  )

  if [[ ! -f "${sofia_binary_path}" ]]; then
    echo "sofia binary not found at ${sofia_binary_path}" >&2
    return 1
  fi

  docker rm -f "${container_name}" >/dev/null 2>&1 || true
  # bubblewrap needs mount propagation inside the remote test container.
  docker run -d \
    --name "${container_name}" \
    --privileged \
    --security-opt seccomp=unconfined \
    ubuntu:24.04 sleep infinity >/dev/null
  if ! docker exec "${container_name}" sh -lc "apt-get update && DEBIAN_FRONTEND=noninteractive apt-get install -y python3 zsh bubblewrap"; then
    docker rm -f "${container_name}" >/dev/null 2>&1 || true
    return 1
  fi

  if [[ -z "${SOFIA_TEST_REMOTE_EXEC_SERVER_URL:-}" ]]; then
    remote_codex_path="/tmp/sofia-remote-env/sofia"
    remote_exec_server_port="31987"
    remote_exec_server_stdout_path="/tmp/sofia-remote-env/exec-server.stdout"
    docker exec "${container_name}" sh -lc "mkdir -p /tmp/sofia-remote-env"
    docker cp "${sofia_binary_path}" "${container_name}:${remote_codex_path}"
    docker exec "${container_name}" chmod +x "${remote_codex_path}"
    remote_exec_server_pid="$(
      docker exec "${container_name}" sh -lc \
        "rm -f ${remote_exec_server_stdout_path}; nohup ${remote_codex_path} exec-server --listen ws://0.0.0.0:${remote_exec_server_port} > ${remote_exec_server_stdout_path} 2>&1 & echo \$!"
    )"
    wait_for_remote_exec_server_port "${container_name}" "${remote_exec_server_port}" "${remote_exec_server_stdout_path}"
    container_ip="$(
      docker inspect -f '{{range .NetworkSettings.Networks}}{{.IPAddress}}{{end}}' "${container_name}"
    )"
    if [[ -z "${container_ip}" ]]; then
      echo "container ${container_name} has no IP address" >&2
      docker rm -f "${container_name}" >/dev/null 2>&1 || true
      return 1
    fi
    export SOFIA_TEST_REMOTE_EXEC_SERVER_PID="${remote_exec_server_pid}"
    export SOFIA_TEST_REMOTE_EXEC_SERVER_URL="ws://${container_ip}:${remote_exec_server_port}"
  fi

  export SOFIA_TEST_REMOTE_ENV="${container_name}"
  export SOFIA_TEST_REMOTE_ENV_CONTAINER_NAME="${container_name}"
  export SOFIA_TEST_ENVIRONMENT="docker"
}

wait_for_remote_exec_server_port() {
  local container_name="$1"
  local port="$2"
  local stdout_path="$3"
  local deadline=$((SECONDS + 5))

  while (( SECONDS < deadline )); do
    if docker exec "${container_name}" python3 -c "import socket; socket.create_connection(('127.0.0.1', ${port}), timeout=0.2).close()" >/dev/null 2>&1; then
      return 0
    fi
    sleep 0.025
  done

  echo "timed out waiting for remote exec-server on ${container_name}:${port}" >&2
  docker exec "${container_name}" sh -lc "cat ${stdout_path} 2>/dev/null || true" >&2 || true
  return 1
}

sofia_remote_env_cleanup() {
  if [[ -n "${SOFIA_TEST_REMOTE_ENV:-}" ]]; then
    docker rm -f "${SOFIA_TEST_REMOTE_ENV}" >/dev/null 2>&1 || true
    unset SOFIA_TEST_REMOTE_ENV
  fi
  unset SOFIA_TEST_REMOTE_ENV_CONTAINER_NAME
  unset SOFIA_TEST_REMOTE_EXEC_SERVER_PID
  unset SOFIA_TEST_REMOTE_EXEC_SERVER_URL
  unset SOFIA_TEST_ENVIRONMENT
}

if ! is_sourced; then
  echo "source this script instead of executing it: source scripts/test-remote-env.sh" >&2
  exit 1
fi

old_shell_options="$(set +o)"
set -euo pipefail
if setup_remote_env; then
  status=0
  echo "SOFIA_TEST_REMOTE_ENV=${SOFIA_TEST_REMOTE_ENV}"
  echo "SOFIA_TEST_ENVIRONMENT=${SOFIA_TEST_ENVIRONMENT}"
  echo "SOFIA_TEST_REMOTE_EXEC_SERVER_URL=${SOFIA_TEST_REMOTE_EXEC_SERVER_URL}"
  echo "Remote env ready. Run your command, then call: sofia_remote_env_cleanup"
else
  status=$?
fi
eval "${old_shell_options}"
return "${status}"
