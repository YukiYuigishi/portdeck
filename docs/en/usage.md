# Usage

English | [日本語](../ja/usage.md)

portdeck has a Targets pane for SSH sessions and a Forwards pane for the saved
rules belonging to the selected target. The status line shows available keys,
the latest result, and short error summaries.

## Target discovery

Targets are collected from concrete `Host` aliases in `~/.ssh/config` and its
recursive `Include` files. `Host *`, wildcard patterns, and negated patterns are
not displayed as targets. Their settings still apply normally when OpenSSH
resolves `ssh -G <alias>` or connects to the selected alias.

portdeck does not modify SSH configuration files. HostName, User, Port,
ProxyJump, authentication, and host key checking remain OpenSSH settings.

## Keyboard controls

The normal screen accepts these keys:

| Key | Action |
| --- | --- |
| `Tab` | Switch between the Targets and Forwards panes |
| `h` / `←` | Focus the Targets pane |
| `l` / `→` | Focus the Forwards pane |
| `↑` / `↓` / `j` / `k` | Move the selection |
| `/` | Start a case-insensitive `Host` alias search |
| `c` | Connect to the selected target |
| `r` | Recheck the session with `ssh -O check` |
| `a` | Add and save a forward rule |
| `e` | Edit the selected inactive forward rule |
| `Space` | Activate or cancel the selected forward |
| `D` | Delete a forward rule after confirmation |
| `d` | Disconnect a session after confirmation |
| `E` | Show the latest OpenSSH error or startup diagnostic |
| `q` / `Ctrl-C` | Stop portdeck-owned sessions and quit |

Confirmation dialogs accept `y` or `Enter`; use `n` or `Esc` to cancel. Close
error details with `Esc`, `E`, or `Enter`.

## Search targets

Press `/` and type part of a `Host` alias. Matching is case-insensitive and only
changes the displayed target list; it does not edit SSH or portdeck
configuration.

- `Backspace` removes the last character.
- `Enter` applies the filter.
- `Esc` restores the filter that was active before editing.
- Applying an empty search displays every target again.

Actions such as connect, check, and disconnect always apply to the target
selected in the filtered list.

## Session lifecycle

Press `c` on a target to start its dedicated ControlMaster. portdeck suspends
the TUI while OpenSSH uses the terminal for authentication, key passphrases, or
first-time host key confirmation. The TUI resumes after OpenSSH returns.

Press `r` to ask the ControlMaster for its current status. If a disconnection is
detected, forwards under that session become unavailable. portdeck does not
automatically reconnect.

Press `d` and confirm to stop the selected session with `ssh -O exit`. Pressing
`q` or `Ctrl-C` prompts when necessary, then stops every ControlMaster owned by
portdeck before exiting. There is no detach mode that leaves owned sessions
running after a normal exit.

## Add a forward

Press `a` to open the forward form. Use `Tab` or `↑`/`↓` to move between
fields, `Enter` to save, and `Esc` to cancel. On the Forward kind field, use
`←`/`→`, `Space`, `l`, or `s` to choose Local or SOCKS.

The fields are:

| Field | Local (`ssh -L`) | SOCKS (`ssh -D`) |
| --- | --- | --- |
| Forward kind | Local | SOCKS |
| Label | Optional display name | Optional display name |
| Local bind address | Defaults to `127.0.0.1` | Defaults to `127.0.0.1` |
| Preferred local port | Empty uses the remote port | Defaults to `1080` |
| Remote destination host | Defaults to `127.0.0.1`, as seen remotely | Not used |
| Remote destination port | Required, from 1 through 65535 | Not used |

Saving a rule does not activate it. Move to the Forwards pane and press
`Space`. If the preferred local port is unavailable, portdeck tries at most 20
consecutive candidates and displays the port OpenSSH actually accepted.

For Local rules, the listener forwards raw TCP traffic to the destination as
seen from the remote side. For SOCKS rules, SOCKS4/5 handling and TCP forwarding
are provided by OpenSSH through `ssh -D`; portdeck does not implement a proxy
protocol itself.

## Edit a forward

Select an inactive saved rule and press `e`. The same form opens with the
current values. `Enter` saves the update and `Esc` discards it. Editing preserves
the rule ID, target ownership, and selection position, and does not activate the
rule.

A rule cannot be edited while it is Active, Adding, Removing, Unavailable, or
Failed, or while it retains an actual local-port assignment. Cancel it with
`Space` and return it to Inactive first.

## Activate, cancel, and delete

`Space` asks the selected session's ControlMaster to add an inactive rule or
cancel an active one. A forward is shown as Active only after OpenSSH returns a
successful status. Cancellation uses the same normalized `-L` or `-D`
specification that was used to add the forward; a failed cancellation is not
hidden as a successful UI-only change.

`D` deletes the selected saved rule after confirmation. Cancel an active rule
before deleting it.

## Public listeners

Binding to `0.0.0.0`, `::`, or `*` can expose a listener beyond the local
machine, so saving such a rule requires an extra confirmation. The same check
applies when editing a rule.

A portdeck SOCKS listener has no additional authentication. If it is exposed,
anyone able to reach it may be able to connect to arbitrary destinations
through the SSH session. Review firewall and network access before confirming.
