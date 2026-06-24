# fnf — Fancified YUM

A `dnf upgrade` wrapper with yay-style output: colored version diffs, aligned columns, download/disk sizes, and a confirmation prompt before anything is installed.

## What it looks like

```
⠹ Updating and loading repositories...

 :: 19 packages to upgrade  (53 MiB download, +11 MiB disk)

    firefox          112.0-1.fc44 -> 113.0-1.fc44  48.3 MiB  updates
    bash             5.2.15-1.fc44 -> 5.2.21-1.fc44  3.5 MiB  updates
    python3-requests 2.28.1-1.fc44 -> 2.28.2-1.fc44  121.4 KiB  updates

==> Proceed with upgrade? [Y/n]
```

Only the changed segment of each version is highlighted (red → green); common prefix and suffix are dimmed.

## Install

```sh
cargo install --path .
```

Installs `fnf` to `~/.cargo/bin/`. Make sure that's on your `PATH` before `/usr/bin`.

## Usage

```sh
fnf upgrade          # check for updates and prompt
fnf up               # alias
fnf update           # alias
```

### Flags

| Flag | Short | Description |
|------|-------|-------------|
| `--show-arch` | `-a` | Show the architecture column |
| `--show-command` | `-c` | Print the exact `dnf` command above the Y/n prompt |

### Example with flags

```sh
fnf upgrade --show-arch --show-command
```

## How it works

1. Runs `dnf upgrade --assumeno --color=never` — no root needed, no changes made
2. Shows a spinner while repositories load; suppresses dnf's repo-loading noise
3. Parses the transaction table and displays a compact diff table
4. Prompts for confirmation
5. On Y: runs `dnf upgrade -y pkg-version.arch …` with the exact package specs shown — no surprise upgrades if new versions appeared since the check

## Build

```sh
cargo build           # debug → target/debug/fnf
cargo clippy          # lint
```

## Requirements

- Fedora / RHEL-based system with `/usr/bin/dnf`
- Rust toolchain (edition 2024)
