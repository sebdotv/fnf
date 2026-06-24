# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

**Always update this file when making changes to the project.**

## Project Overview

`fnf` (Fancified YUM) is a `dnf` wrapper that enhances `dnf upgrade` with yay-style colored output: version diffs highlighted by differing segment, aligned columns, download sizes, repository names, and a Y/n confirmation prompt before running the actual upgrade.

Binary name: `fnf`. The repo directory is `dnf-wrapper` for historical reasons.

## Commands

```sh
cargo build                    # build debug binary → target/debug/fnf
cargo run -- upgrade           # run (aliases: up, update)
cargo clippy                   # lint
cargo test                     # run tests (none currently)
cargo install --path .         # install to ~/.cargo/bin/fnf
```

Manual testing requires `dnf` on the system:

```sh
target/debug/fnf upgrade       # runs dnf upgrade --assumeno, then prompts
```

## Architecture

Everything lives in `src/main.rs`. No modules, no workspace.

**Upgrade flow:**

1. `check_updates()` — runs `dnf upgrade --assumeno --color=never`; parses stdout
2. `parse_update_lines()` — walks the transaction table on stdout:
   - Lines starting with ` ` (1 space, inside `Upgrading:` section): new version — name, arch, version, repo, size
   - Lines starting with `   replacing `: old version for the preceding package
   - Any non-space-starting line changes the active section
3. `display_updates()` prints an aligned table; `highlight_version_diff()` finds the common prefix and suffix between old/new version strings and colors only the differing middle segment
4. After Y/n confirmation, `do_upgrade()` runs `dnf upgrade -y`

**Key details:**
- `normalize_version()` strips the `0:` epoch prefix that dnf includes
- `parse_dnf_size()` converts dnf's human-readable sizes (e.g. `3.6 MiB`) to bytes for summing; `format_size()` re-formats for display
- Column widths are computed from max lengths before printing, so all rows align; colored strings are padded by appending plain spaces after the ANSI codes
- `DNF` constant points to `/usr/bin/dnf` (absolute path, avoids PATH shadowing)
- clap handles subcommand parsing; `upgrade` has aliases `up` and `update`
