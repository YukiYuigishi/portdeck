# portdeck

English | [日本語](README.ja.md)

portdeck is a Linux TUI for managing OpenSSH connections and Local or SOCKS
forwards. Select a target from `~/.ssh/config`, manage a dedicated
ControlMaster, and save, activate, or cancel `ssh -L` and `ssh -D` forwards
from one screen.

OpenSSH remains responsible for the SSH protocol, authentication, encryption,
host key verification, ProxyJump, and TCP forwarding. portdeck is not a
terminal emulator, an SSH key manager, or a custom proxy implementation.

## Features

- Discover concrete `Host` aliases from `~/.ssh/config` and recursive
  `Include` files
- Start, check, and stop portdeck-owned OpenSSH ControlMaster sessions
- Save and manage Local (`ssh -L`) and SOCKS (`ssh -D`) forward rules
- Retry a bounded range of local ports when the preferred port is unavailable
- Search targets, edit inactive rules, and inspect OpenSSH errors in the TUI
- Record private, file-backed diagnostic logs with `--debug`

## Requirements

- Linux
- An OpenSSH client with `ssh` available on `PATH`
- Rust stable when installing from source
- A `~/.ssh/config` containing concrete `Host` aliases to display targets

The MVP has been tested with real traffic on OpenSSH 9.6p1. Its minimum
supported OpenSSH version has not been established. macOS and Windows are not
currently supported.

## Install and run

```console
cargo install --path .
portdeck --diagnose
portdeck
```

`--diagnose` prints the OpenSSH version selected from `PATH`. `--help`,
`--version`, and `--debug` are also available.

## Quick start

1. Run `portdeck`.
2. Select a target and press `c` to connect.
3. Press `a` to save a Local or SOCKS forward rule.
4. Move to the Forwards pane and press `Space` to activate the rule.
5. Press `Space` again to cancel it, or `q` to stop portdeck-owned sessions and
   quit.

Saved rules are not activated automatically after creation or restart.

## Essential keys

| Key | Action |
| --- | --- |
| `↑` / `↓` / `j` / `k` | Move the selection |
| `Tab` / `h` / `l` | Switch between Targets and Forwards |
| `/` | Search targets by `Host` alias |
| `c` | Connect to the selected target |
| `a` | Add a forward rule |
| `e` | Edit the selected inactive rule |
| `Space` | Activate or cancel the selected forward |
| `D` / `d` | Delete a rule / disconnect a session |
| `E` | Show the latest error details |
| `q` / `Ctrl-C` | Stop owned sessions and quit |

See the [usage guide](docs/en/usage.md) for the complete interaction model and
form controls.

## Security highlights

- Commands are passed to OpenSSH as individual arguments, without a shell.
- portdeck does not weaken host key verification or modify SSH configuration,
  `known_hosts`, or private keys.
- Passwords, key passphrases, and private keys are not stored or logged.
- Local listeners default to `127.0.0.1`; externally exposed listeners require
  confirmation and display a warning.
- SOCKS and TCP forwarding are delegated to OpenSSH, not implemented by
  portdeck.
- Normal shutdown explicitly stops every ControlMaster owned by portdeck.

## Documentation

- [Usage](docs/en/usage.md): targets, sessions, search, and forward operations
- [Runtime and configuration](docs/en/runtime-and-configuration.md): saved
  rules, XDG paths, ControlPath management, and recovery
- [Troubleshooting](docs/en/troubleshooting.md): diagnostics, DEBUG logs, and
  common failures
- [Contributing](CONTRIBUTING.md): hooks, linting, and tests

Implementation order and remaining platform-hardening work are tracked in
[PLAN.md](PLAN.md).
