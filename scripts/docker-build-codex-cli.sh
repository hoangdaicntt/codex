#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd -- "$SCRIPT_DIR/.." && pwd)"

IMAGE="${CODEX_DOCKER_IMAGE:-rust:1.95-bookworm}"
OUTPUT_DIR="${CODEX_DOCKER_OUTPUT_DIR:-$REPO_ROOT/docker-build-out}"
OUTPUT_BIN="$OUTPUT_DIR/codex"
RUN_AFTER_BUILD=0

usage() {
  cat <<'EOF'
Usage: scripts/docker-build-codex-cli.sh [--run] [--help]

Build Codex CLI inside Docker and copy the binary to:
  docker-build-out/codex

Options:
  --run     Run the built host binary with --version after build.
  --help    Show this help.

Environment:
  CODEX_DOCKER_IMAGE       Docker image to use. Default: rust:1.95-bookworm
  CODEX_DOCKER_OUTPUT_DIR  Output directory. Default: ./docker-build-out
EOF
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --run)
      RUN_AFTER_BUILD=1
      shift
      ;;
    --help|-h)
      usage
      exit 0
      ;;
    *)
      echo "Unknown option: $1" >&2
      usage >&2
      exit 2
      ;;
  esac
done

run_docker() {
  if docker info >/dev/null 2>&1; then
    docker "$@"
    return
  fi

  if command -v sg >/dev/null 2>&1 && id -nG | tr ' ' '\n' | grep -qx docker; then
    sg docker -c "$(printf '%q ' docker "$@")"
    return
  fi

  echo "Docker is not reachable. Try logging out/in after joining the docker group, or run with sudo." >&2
  exit 1
}

mkdir -p "$OUTPUT_DIR"

run_docker run --rm \
  -v "$REPO_ROOT:/workspace" \
  -v codex-cargo-registry:/usr/local/cargo/registry \
  -v codex-cargo-git:/usr/local/cargo/git \
  -v codex-rs-target:/target \
  -e CARGO_TARGET_DIR=/target \
  -w /workspace/codex-rs \
  "$IMAGE" \
  bash -lc 'export PATH=/usr/local/cargo/bin:$PATH; apt-get update && apt-get install -y --no-install-recommends pkg-config libssl-dev protobuf-compiler cmake git python3 ca-certificates && git config --global --add safe.directory /workspace && cargo build -p codex-cli'

run_docker run --rm \
  -v codex-rs-target:/target \
  -v "$OUTPUT_DIR:/out" \
  "$IMAGE" \
  bash -lc "cp /target/debug/codex /out/codex && chown $(id -u):$(id -g) /out/codex"

echo "Built $OUTPUT_BIN"
echo "Run: $OUTPUT_BIN --version"

if [[ "$RUN_AFTER_BUILD" -eq 1 ]]; then
  "$OUTPUT_BIN" --version
fi
