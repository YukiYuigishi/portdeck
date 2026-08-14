#!/usr/bin/env sh
set -eu

repository_root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
revision=$(git -C "$repository_root" rev-parse --verify HEAD)

smoke_root=
cleanup() {
    if [ -n "$smoke_root" ] && [ -d "$smoke_root" ]; then
        rm -rf -- "$smoke_root"
    fi
}
trap cleanup 0
trap 'exit 1' HUP INT TERM

smoke_root=$(mktemp -d "${TMPDIR:-/tmp}/portdeck-install-smoke.XXXXXX")
cargo_home="$smoke_root/cargo-home"
install_root="$smoke_root/install"
test_home="$smoke_root/home"
config_home="$smoke_root/config"
state_home="$smoke_root/state"
runtime_directory="$smoke_root/runtime"
fake_bin="$smoke_root/bin"

mkdir -p \
    "$cargo_home" \
    "$install_root" \
    "$test_home" \
    "$config_home" \
    "$state_home" \
    "$runtime_directory" \
    "$fake_bin"
chmod 700 "$test_home" "$config_home" "$state_home" "$runtime_directory"

cat >"$fake_bin/ssh" <<'EOF'
#!/bin/sh
if [ "$#" -ne 1 ] || [ "$1" != "-V" ]; then
    echo "install smoke: unexpected ssh arguments" >&2
    exit 2
fi
echo "OpenSSH_9.6p1 install-smoke" >&2
EOF
chmod 700 "$fake_bin/ssh"

CARGO_HOME="$cargo_home" cargo install \
    --git "file://$repository_root" \
    --rev "$revision" \
    --locked \
    --root "$install_root"

installed_binary="$install_root/bin/portdeck"
HOME="$test_home" \
XDG_CONFIG_HOME="$config_home" \
XDG_STATE_HOME="$state_home" \
XDG_RUNTIME_DIR="$runtime_directory" \
PATH="$fake_bin:/usr/bin:/bin" \
    "$installed_binary" --version

HOME="$test_home" \
XDG_CONFIG_HOME="$config_home" \
XDG_STATE_HOME="$state_home" \
XDG_RUNTIME_DIR="$runtime_directory" \
PATH="$fake_bin:/usr/bin:/bin" \
    "$installed_binary" --diagnose
