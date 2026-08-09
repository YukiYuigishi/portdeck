# Troubleshooting

English | [日本語](../ja/troubleshooting.md)

Start with portdeck's short error message, then press `E` in the TUI for the
latest OpenSSH diagnostic. portdeck keeps user-facing summaries separate from
the detailed OpenSSH error so failures are visible without hiding their cause.

## Quick diagnostics

```console
portdeck --diagnose
ssh -V
portdeck --debug --diagnose
```

`portdeck --diagnose` runs the `ssh` found on `PATH` and prints its detected
version. `--debug` may be combined with `--diagnose` or used while running the
TUI. It does not change OpenSSH configuration or connection arguments.

## DEBUG mode

Run `portdeck --debug` to write DEBUG-level structured events to a private file
for that invocation:

```text
$XDG_STATE_HOME/portdeck/debug-<timestamp>-<pid>.log
# When XDG_STATE_HOME is unset:
$HOME/.local/state/portdeck/debug-<timestamp>-<pid>.log
```

The log directory uses mode `0700` and each log file uses mode `0600`. The
actual path is printed before portdeck enters the alternate screen and is also
available from the TUI startup status and error details.

Log retention targets the 10 most recent logs and removes at most 64 excess
matching logs per startup. Only filenames that strictly match portdeck's owned
`debug-<number>-<number>.log` pattern are eligible for removal. Other files in
the directory are left unchanged.

Recorded events cover connection, session checks, disconnection, forward add
and cancellation, port candidates, state transitions, configuration saves and
rollbacks, runtime recovery, and shutdown. Related events carry an operation
ID. DEBUG events are written to the file rather than stdout or stderr while the
TUI is drawing.

The log deliberately omits:

- Passwords and key passphrases
- Private key contents
- The full process environment
- Raw key input
- Raw OpenSSH stdout and stderr

Before sharing a log, still review operational information such as host aliases
and port numbers.

## Common problems

### No targets are displayed

Check that `~/.ssh/config` or one of its readable `Include` files contains a
concrete `Host <alias>`. Wildcards, `Host *`, and negated patterns are applied
by OpenSSH but are not displayed as selectable targets.

### OpenSSH cannot be started

Run `portdeck --diagnose` and `ssh -V`, and confirm that the intended `ssh`
binary is available on `PATH`. portdeck reports a distinct startup failure when
OpenSSH cannot be executed.

### Connection or authentication fails

Press `E` to inspect OpenSSH stderr. Try `ssh <alias>` outside portdeck with the
same environment and SSH files. portdeck does not change authentication
methods, disable host key checks, or retry with weaker settings.

### A forward cannot be activated

Check for a local-port conflict and whether the server permits forwarding with
settings such as `AllowTcpForwarding`. For a Local rule, also verify that its
remote destination is reachable from the remote side. portdeck tries a bounded
set of 20 local-port candidates but relies on the final OpenSSH status.

For an exposed SOCKS listener, remember that portdeck adds no proxy
authentication. Binding safely does not fix an SSH server that rejects dynamic
forwarding.

### The displayed state may be stale

Press `r` to recheck the selected ControlMaster with `ssh -O check`. If the
master has disconnected, the session is updated and its forwards are shown as
Unavailable rather than usable.

### A forward cannot be edited or deleted

Only an Inactive rule without retained runtime-forward data can be edited or
deleted. Press `Space` to cancel an active rule first. If cancellation fails,
inspect `E`; portdeck keeps the failed runtime information visible instead of
pretending that cancellation succeeded.

### The configuration file is invalid

portdeck reports the parse or validation error and exits without overwriting
the file. Correct the file at `$XDG_CONFIG_HOME/portdeck/config.toml`, or at
`$HOME/.config/portdeck/config.toml` when `XDG_CONFIG_HOME` is unset, then
restart portdeck.

### More execution detail is needed

Reproduce the problem with `portdeck --debug`, then inspect the owner-only log
whose path is shown at startup. The log provides operation sequencing and error
categories; use `E` for the raw OpenSSH error because raw output is intentionally
excluded from DEBUG logs.
