#!/usr/bin/env bash
set -euo pipefail

NO_PROXY=0
HTTP_PROXY_VALUE="http://127.0.0.1:10809"
SOCKS_PROXY_VALUE="socks5://127.0.0.1:10808"

usage() {
  cat <<'EOF'
Build the Excalidraw Tauri desktop app for macOS.

Usage:
  ./build-desktop.command
  ./build-desktop.command --no-proxy
  ./build-desktop.command --http-proxy http://127.0.0.1:10809 --socks-proxy socks5://127.0.0.1:10808

Outputs:
  src-tauri/target/release/excalidraw-cloud-sync
  src-tauri/target/release/bundle/macos/Excalidraw.app
  src-tauri/target/release/bundle/dmg/Excalidraw_0.1.0_<arch>.dmg
EOF
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    -h|--help|-Help)
      usage
      exit 0
      ;;
    --no-proxy|-NoProxy)
      NO_PROXY=1
      shift
      ;;
    --http-proxy|-HttpProxy)
      HTTP_PROXY_VALUE="${2:-}"
      if [[ -z "$HTTP_PROXY_VALUE" ]]; then
        echo "Missing value for $1" >&2
        exit 1
      fi
      shift 2
      ;;
    --socks-proxy|-SocksProxy)
      SOCKS_PROXY_VALUE="${2:-}"
      if [[ -z "$SOCKS_PROXY_VALUE" ]]; then
        echo "Missing value for $1" >&2
        exit 1
      fi
      shift 2
      ;;
    *)
      echo "Unknown option: $1" >&2
      echo ""
      usage
      exit 1
      ;;
  esac
done

write_step() {
  echo ""
  printf '\033[36m==> %s\033[0m\n' "$1"
}

write_output_file() {
  local path="$1"

  if [[ -e "$path" ]]; then
    local size_mb modified
    size_mb="$(du -sm "$path" | awk '{print $1}')"
    modified="$(stat -f '%Sm' "$path")"
    echo "  $path (${size_mb} MB, $modified)"
  fi
}

remove_path_if_exists() {
  local path="$1"

  if [[ -e "$path" ]]; then
    rm -rf "$path"
    echo "  removed $path"
  fi
}

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo_root="$script_dir"
tauri_dir="$repo_root/src-tauri"
cargo_bin="$HOME/.cargo/bin"
arch="$(uname -m)"
if [[ "$arch" == "arm64" ]]; then
  bundle_arch="aarch64"
else
  bundle_arch="$arch"
fi

if [[ ! -d "$tauri_dir" ]]; then
  echo "Cannot find src-tauri directory: $tauri_dir" >&2
  exit 1
fi

if [[ -d "$cargo_bin" ]]; then
  export PATH="$cargo_bin:$PATH"
fi

if [[ "$NO_PROXY" -eq 0 ]]; then
  export HTTP_PROXY="$HTTP_PROXY_VALUE"
  export HTTPS_PROXY="$HTTP_PROXY_VALUE"
  export ALL_PROXY="$SOCKS_PROXY_VALUE"
  write_step "Proxy enabled"
  echo "  HTTP_PROXY=$HTTP_PROXY"
  echo "  HTTPS_PROXY=$HTTPS_PROXY"
  echo "  ALL_PROXY=$ALL_PROXY"
else
  write_step "Proxy disabled"
  unset HTTP_PROXY HTTPS_PROXY ALL_PROXY
fi

write_step "Checking toolchain"
cargo --version | sed 's/^/  /'
rustc --version | sed 's/^/  /'

if cargo tauri --version >/tmp/excalidraw-tauri-version.txt 2>&1; then
  sed 's/^/  /' /tmp/excalidraw-tauri-version.txt
else
  printf '\033[33m  Tauri CLI not found. Installing tauri-cli v2...\033[0m\n'
  cargo install tauri-cli --version "^2" --locked
fi
rm -f /tmp/excalidraw-tauri-version.txt

running_app_processes="$(
  ps -axo pid=,command= |
    awk -v target="$tauri_dir/target/release/bundle/macos/Excalidraw.app/Contents/MacOS/" \
      '{
        command = $0
        sub(/^[[:space:]]*[0-9]+[[:space:]]+/, "", command)
        if (index(command, target) == 1) {
          print
        }
      }'
)"

if [[ -n "$running_app_processes" ]]; then
  write_step "Detected running desktop app"
  echo "$running_app_processes" | sed 's/^/  /'
  echo "Close the running Excalidraw desktop app before building, then run the script again." >&2
  exit 1
fi

write_step "Cleaning previous desktop outputs"
remove_path_if_exists "$tauri_dir/target/release/bundle"
remove_path_if_exists "$tauri_dir/target/release/excalidraw-cloud-sync"
remove_path_if_exists "$tauri_dir/target/release/excalidraw-cloud-sync.d"

write_step "Building desktop package"
(
  cd "$tauri_dir"
  cargo tauri build
)

write_step "Build outputs"
write_output_file "$tauri_dir/target/release/excalidraw-cloud-sync"
write_output_file "$tauri_dir/target/release/bundle/macos/Excalidraw.app"
write_output_file "$tauri_dir/target/release/bundle/dmg/Excalidraw_0.1.0_${bundle_arch}.dmg"

echo ""
printf '\033[32mDone.\033[0m\n'
