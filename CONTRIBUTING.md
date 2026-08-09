# Contributing to portdeck

Thank you for contributing to portdeck. Keep changes focused, and update tests and documentation whenever behavior changes.

## Prerequisites

Development currently targets Linux. Install:

- Git
- the stable Rust toolchain, including `rustfmt` and Clippy
- OpenSSH Client (`ssh`)

The ignored integration tests also require these executables at their standard Linux paths:

- `/usr/bin/ssh`
- `/usr/bin/ssh-keygen`
- `/usr/sbin/sshd`

## Git hook

Enable the repository's pre-commit hook once after cloning:

```console
./scripts/install-git-hooks.sh
```

The hook runs the same lint script used by GitHub Actions. If you intentionally bypass the hook, run the checks below before sharing the change.

## Checks

Check formatting and Clippy warnings:

```console
./scripts/lint.sh
```

Run the regular test suite, including tests that use a fake `ssh` executable:

```console
cargo test --all-targets --all-features
```

GitHub Actions runs both commands for pushes and pull requests.

## OpenSSH integration tests

The real-OpenSSH tests are ignored by default. Run them explicitly on a compatible Linux system:

```console
cargo test --test openssh_integration -- --ignored --test-threads=1 --nocapture
```

These tests start a local `sshd` on an unprivileged port and generate host and client keys, a client configuration, a dedicated `known_hosts`, and ControlMaster runtime files inside a temporary directory. They verify ControlMaster lifecycle, Local forwarding, direct SOCKS5 forwarding, and SOCKS5 forwarding through ProxyJump.

They do not read or modify the user's SSH configuration, `known_hosts`, private keys, or existing ControlMaster sessions.

## Change discipline

- Split work into small commits with one clear purpose.
- Add or update tests when behavior changes.
- Update user and developer documentation in the same change when commands, controls, configuration, or supported behavior changes.
- Keep external command execution argv-based; do not introduce shell command construction.

See [PLAN.md](PLAN.md) for planned work and environment-specific hardening that has not yet been completed.
