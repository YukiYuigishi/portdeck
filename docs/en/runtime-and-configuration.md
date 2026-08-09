# Runtime and configuration

English | [日本語](../ja/runtime-and-configuration.md)

portdeck separates saved forward definitions from the state of running
OpenSSH processes. A saved rule may exist without a session or active forward,
and restarting portdeck does not automatically connect or activate rules.

## Saved configuration

Forward rules are stored in:

```text
$XDG_CONFIG_HOME/portdeck/config.toml
# When XDG_CONFIG_HOME is unset:
$HOME/.config/portdeck/config.toml
```

Each change is written through a temporary file, synchronized, atomically
renamed into place, and followed by a parent-directory synchronization. If an
existing file cannot be parsed, portdeck reports the error and does not
overwrite it.

The current format uses schema version 1. For example:

```toml
version = 1

[[targets]]
host_alias = "dev-server"

[[targets.forwards]]
id = "rule-00000001"
label = "web"
bind_address = "127.0.0.1"
requested_local_port = 8080
kind = "local"
remote_host = "127.0.0.1"
remote_port = 3000

[[targets.forwards]]
id = "rule-00000002"
label = "browser-proxy"
bind_address = "127.0.0.1"
requested_local_port = 1080
kind = "socks"
```

Local rules persist their remote destination. SOCKS rules only persist the
local listener. Configuration created before the explicit `kind` field was
introduced is read as Local under the same version-1 schema and is written with
an explicit kind after the next change.

portdeck does not persist active-session state, process IDs, the actual local
port selected after retries, authentication data, private keys, or key
passphrases.

## Runtime directory and ControlPath

ControlMaster sockets are stored in:

```text
$XDG_RUNTIME_DIR/portdeck/
# When XDG_RUNTIME_DIR is unset:
<OS temporary directory>/portdeck-<uid>/
```

The fallback uses the temporary directory selected by `std::env::temp_dir()`.
On Linux this is normally `/tmp`, but it may differ according to the
environment.

The runtime directory must be absolute, owned by the current user, and a real
directory rather than a symlink. portdeck restricts it to mode `0700`.

Each target receives a short, stable ControlPath name derived from its internal
target ID. The `Host` alias itself is not embedded in the socket name. portdeck
also checks the resulting Unix-domain socket path against a conservative path
length limit.

The dedicated master starts with `ClearAllForwardings=yes`, so forwards from
the user's SSH configuration are not silently mixed into portdeck's tracked
runtime state. Dynamic forward and cancellation requests use
`ClearAllForwardings=no` with the exact `-L` or `-D` specification managed by
portdeck.

## Startup recovery

At startup, portdeck inspects ControlPath entries in its own namespace:

1. A socket associated with a currently known target is checked using
   `ssh -O check`.
2. If the known master is alive, portdeck explicitly stops it.
3. If OpenSSH confirms that the known socket is stale, portdeck removes it.
4. Runtime entries that cannot be associated with a current target are kept
   and reported for diagnosis.

An unchecked or unrecognized path is not deleted merely because it exists.
portdeck never adopts or stops ControlMasters created by another tool or by the
user outside its dedicated runtime directory.

## Shutdown

On normal quit, portdeck checks and explicitly stops every ControlMaster it
owns. Stopping a session also makes its runtime forwards inactive or
unavailable. There is currently no detach mode that intentionally leaves an
owned ControlMaster running after the TUI exits.

## Security properties

- SSH commands are executed as argument arrays, without `/bin/sh -c` or other
  shell interpolation.
- Authentication, encryption, ProxyJump, host key checks, SOCKS handling, and
  TCP forwarding remain OpenSSH responsibilities.
- portdeck does not add `StrictHostKeyChecking=no` or
  `UserKnownHostsFile=/dev/null`.
- `~/.ssh/config`, `known_hosts`, private keys, and other SSH-owned files are
  not modified.
- Forward state changes are accepted only after the corresponding OpenSSH
  command succeeds; a preliminary local bind check alone does not mark a
  forward Active.
- External bind addresses require confirmation. portdeck does not add
  authentication to an OpenSSH SOCKS listener.

For diagnostic-log storage and redaction rules, see
[Troubleshooting](troubleshooting.md).
