# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

**Always update this file and README.md when making changes to the project.**

## Project Overview

`fnf` (Fancified YUM) is a `dnf` wrapper that enhances `dnf upgrade` with yay-style colored output: version diffs highlighted by differing segment, aligned columns, download sizes, repository names, and a Y/n confirmation prompt before running the actual upgrade.

Binary name: `fnf`.

## Commands

```sh
cargo build                    # build debug binary → target/debug/fnf
cargo run -- upgrade           # run (aliases: up, update)
cargo clippy                   # lint
cargo test                     # run tests (none currently)
cargo install --path .         # install to ~/.cargo/bin/fnf

# Releasing (cargo-release)
cargo release patch --execute  # patch release; use minor or major as appropriate
# release.toml auto-updates pkg/fnf.spec Version: and prepends a %changelog entry via pkg/pre-release.sh

# RPM packaging
./pkg/make-sources.sh          # produces pkg/fnf-<version>.tar.gz + vendor tarball
rpmbuild -ba pkg/fnf.spec ...  # see README.md for full rpmbuild invocation
```

Manual testing requires `dnf` on the system:

```sh
target/debug/fnf upgrade       # runs dnf upgrade --assumeno, then prompts
```

## Architecture

Everything lives in `src/main.rs`. No modules, no workspace.

**Upgrade flow:**

1. `check_updates()` — spawns `dnf upgrade --assumeno --color=never`; reads stderr in a background thread (spinner + size parsing) while stdout is collected; returns `(Vec<PackageUpdate>, SizeInfo)`
2. `parse_update_lines()` — returns `Result<Vec<PackageUpdate>>`; parses the **entire** stdout transaction table with an explicit, fully strict state machine (`TableParser` + `enum State`). Every line must match the pattern the current state expects; there is no "ignore unrecognized lines" path. States: `Header` (column header / `Nothing to do.` / blanks) → `ExpectSection` → `Section { group, upgrading }` → `Replacing { name }` (only inside `Upgrading:`) → `SummaryHeader` → `Summary` → `End`. Line patterns:
   - Column header: `Package Arch Version Repository Size` (exact token set)
   - Section header: column-0 line ending in `:`; mapped to a summary bucket via `summary_group()` (Installing / Upgrading / Downgrading / Reinstalling / Removing, incl. their "dependencies"/"dependent"/"unused" variants). An unknown header is a hard error.
   - Package line: single-space indent, exactly 6 whitespace fields (name, arch, version, repo, size-number, size-unit). Counted per summary bucket; only `Upgrading:` rows become `PackageUpdate`s.
   - `replacing` sub-line: 3-space indent + `replacing `, exactly 6 fields, first field must equal the package just seen; required after every `Upgrading:` package line and forbidden elsewhere.
   - Transaction Summary: ` Label: N package(s)` lines after a blank line; parsed into per-label counts.
   - `finish()` validates the terminal state (output must reach the summary, or be empty) and **cross-checks** the summary: the parsed per-bucket section counts plus the total `replacing`-line count (the `Replacing` bucket) must exactly equal the Transaction Summary counts.
   - Any deviation (wrong field count, missing/orphan replacing line, name mismatch, unknown section header, truncated output, summary/section count disagreement) is a hard error — this surfaces dnf output format changes immediately rather than silently misbehaving. Covered by unit tests in `src/main.rs`, including a real captured transaction in `src/testdata/dnf_upgrade_stdout.txt`.
3. `display_updates()` prints an aligned table; `highlight_diff()` finds the common prefix and suffix between two strings and colors only the differing middle segment — used for both version and repo diffs
4. After Y/n confirmation, `do_upgrade()` runs `dnf upgrade -y` with explicit `name-[epoch:]version-release.arch` specs built from the displayed package list — only the packages shown, at the exact versions shown

**Stderr handling (`process_stderr`):**
- Runs in a background thread while stdout is being collected
- `"Updating and loading repositories:"` → shows an `indicatif` spinner; `"Repositories loaded."` → clears it
- `"Total size of inbound packages is ... Need to download X MiB."` → parsed by `parse_download_line()` into `SizeInfo.download`
- `"After this operation, X MiB extra will be used ..."` / `"... will be freed ..."` → parsed by `parse_disk_line()` into `SizeInfo.net_disk` (positive = used, negative = freed)
- `"Operation aborted by the user."` → silently hidden (expected from `--assumeno`)
- Any other line is forwarded to stderr (via `pb.println` if spinner is active, else `eprintln!`)

**Key details:**
- `normalize_version()` strips the `0:` epoch prefix that dnf includes
- `parse_dnf_size()` converts dnf's human-readable sizes (e.g. `3.6 MiB`) to bytes; `format_size()` re-formats for display; both are used for per-package sizes in the transaction table AND for stderr size line parsing
- `SizeInfo` drives the `(X download, Y disk)` summary in the header; falls back to summing per-package sizes if stderr lines weren't parsed
- Column widths are computed from max lengths before printing, so all rows align; colored strings are padded by appending plain spaces after the ANSI codes
- Repo column: shows repo name dimmed when unchanged; shows `old_repo -> new_repo` diff when the package's source repo changed
- `shorten_repo()` replaces hex transaction hashes (≥20 hex chars) with `first2..last2` (e.g. `19..8e`) since they carry no useful meaning
- Arch column is hidden by default; `fnf upgrade --show-arch` / `-a` shows it
- `fnf upgrade --group <repository|none>` / `-g` controls grouping; default `repository` sorts packages by destination repo (`new_repo`) under an underlined repo header, `none` prints a single flat list sorted by name. Column widths are computed globally so rows align across groups.
- `fnf upgrade --show-command` / `-c` prints the exact `dnf upgrade -y name-version.arch …` command above the Y/n prompt
- `DNF` constant points to `/usr/bin/dnf` (absolute path, avoids PATH shadowing)
- clap handles subcommand parsing; `upgrade` has aliases `up` and `update`
