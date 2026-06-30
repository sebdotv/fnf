use std::collections::BTreeMap;
use std::io::{self, BufRead, Write};
use std::process::{Command, Stdio};
use std::time::Duration;

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand, ValueEnum};
use colored::Colorize;
use indicatif::{ProgressBar, ProgressStyle};

#[derive(Debug)]
struct PackageUpdate {
    name: String,
    arch: String,
    old_version: String,
    new_version: String,
    old_repo: String,
    new_repo: String,
    download_size: u64,
}

#[derive(Default)]
struct SizeInfo {
    download: Option<u64>,
    net_disk: Option<i64>,
}

const DNF: &str = "/usr/bin/dnf";

#[derive(Parser)]
#[command(name = "fnf", about = "Fancified YUM — dnf wrapper with improved upgrade output")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    #[command(alias = "up", alias = "update", about = "Upgrade all packages")]
    Upgrade {
        #[arg(long, short = 'a', help = "Show architecture column")]
        show_arch: bool,
        #[arg(long, short = 'c', help = "Print the dnf command before running it")]
        show_command: bool,
        #[arg(long, short = 'g', value_enum, default_value_t = GroupBy::Repository, help = "Group packages")]
        group: GroupBy,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
enum GroupBy {
    /// Group packages by repository
    Repository,
    /// Do not group packages
    None,
}

fn main() {
    let cli = Cli::parse();

    let result = match cli.command {
        Commands::Upgrade { show_arch, show_command, group } => run_upgrade_wrapper(&Options{ show_arch, show_command, group }),
    };

    if let Err(e) = result {
        eprintln!("Error: {e:#}");
        std::process::exit(1);
    }
}

struct Options {
    show_arch: bool,
    show_command: bool,
    group: GroupBy,
}

fn run_upgrade_wrapper(options: &Options) -> Result<()> {
    let (updates, size_info) = check_updates().context("checking for updates")?;

    if updates.is_empty() {
        println!("{}", " :: System is up to date.".green().bold());
        return Ok(());
    }

    let Options { show_arch, show_command, group } = *options;

    display_updates(&updates, show_arch, group, &size_info);

    if show_command {
        let specs = upgrade_specs(&updates);
        let cmd = std::iter::once(DNF)
            .chain(["upgrade", "-y"])
            .chain(specs.iter().map(String::as_str))
            .collect::<Vec<_>>()
            .join(" ");
        println!("{}", format!("\n==> Command: {cmd}").dimmed());
    }

    print!("\n{} ", "==> Proceed with upgrade? [Y/n]".bold());
    io::stdout().flush()?;

    let mut input = String::new();
    io::stdin().read_line(&mut input)?;
    let answer = input.trim().to_lowercase();

    if answer.is_empty() || answer == "y" || answer == "yes" {
        do_upgrade(&updates);
    } else {
        println!("{}", "Operation cancelled.".yellow());
    }

    Ok(())
}

fn check_updates() -> Result<(Vec<PackageUpdate>, SizeInfo)> {
    let mut child = Command::new(DNF)
        .args(["upgrade", "--assumeno", "--color=never"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .context("running dnf upgrade --assumeno")?;

    let stderr = child.stderr.take().expect("stderr is piped");
    let stderr_thread = std::thread::spawn(move || process_stderr(stderr));

    let output = child
        .wait_with_output()
        .context("waiting for dnf upgrade --assumeno")?;

    let size_info = stderr_thread
        .join()
        .expect("stderr thread panicked")
        .context("processing dnf stderr")?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let updates = parse_update_lines(&stdout).context("parsing dnf output")?;

    Ok((updates, size_info))
}

fn process_stderr(stderr: impl std::io::Read) -> Result<SizeInfo> {
    let reader = std::io::BufReader::new(stderr);
    let mut size_info = SizeInfo::default();
    let mut spinner: Option<ProgressBar> = None;

    for line in reader.lines() {
        let line = line.context("reading dnf stderr")?;
        match line.as_str() {
            "Updating and loading repositories:" => {
                let pb = ProgressBar::new_spinner();
                pb.set_style(
                    ProgressStyle::default_spinner()
                        .tick_chars("⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏ ")
                        .template("{spinner:.cyan} {msg}")
                        .unwrap(),
                );
                pb.enable_steady_tick(Duration::from_millis(100));
                pb.set_message("Updating and loading repositories...");
                spinner = Some(pb);
            }
            "Repositories loaded." => {
                if let Some(pb) = spinner.take() {
                    pb.finish_and_clear();
                }
            }
            "Operation aborted by the user." => {}
            s if s.starts_with("Total size of inbound packages is") => {
                size_info.download = parse_download_line(s);
            }
            s if s.starts_with("After this operation,") => {
                size_info.net_disk = parse_disk_line(s);
            }
            other => match &spinner {
                Some(pb) => pb.println(other),
                None => eprintln!("{other}"),
            },
        }
    }

    if let Some(pb) = spinner.take() {
        pb.finish_and_clear();
    }

    Ok(size_info)
}

fn parse_download_line(line: &str) -> Option<u64> {
    // "Total size of inbound packages is 53 MiB. Need to download 53 MiB."
    let need_part = line.split(". ").nth(1)?;
    let need_part = need_part.trim_end_matches('.');
    let words: Vec<&str> = need_part.split_whitespace().collect();
    if words.len() == 5 && words[..3] == ["Need", "to", "download"] {
        parse_dnf_size(words[3], words[4]).ok()
    } else {
        None
    }
}

fn parse_disk_line(line: &str) -> Option<i64> {
    // "After this operation, 11 MiB extra will be used (install 275 MiB, remove 264 MiB)."
    // "After this operation, 5 MiB will be freed (install 264 MiB, remove 269 MiB)."
    let rest = line.strip_prefix("After this operation, ")?;
    let words: Vec<&str> = rest.split_whitespace().collect();
    if words.len() < 4 {
        return None;
    }
    let bytes = parse_dnf_size(words[0], words[1]).ok()? as i64;
    match &words[2..4] {
        ["extra", "will"] => Some(bytes),
        ["will", "be"] if words.get(4) == Some(&"freed") => Some(-bytes),
        _ => None,
    }
}

/// State of the strict line-by-line parser over the `dnf upgrade` transaction
/// table on stdout. Each state names exactly what the *next* line is allowed to
/// be; any line that does not match the active state is a hard error.
#[derive(Debug)]
enum State {
    /// Before anything: expect the column header, `Nothing to do.`, or blanks.
    Header,
    /// Just after the column header: expect the first section header.
    ExpectSection,
    /// Inside a transaction section: expect a package line, a new section
    /// header, or the blank line that introduces the summary. `group` is the
    /// summary bucket the section's packages count toward; `upgrading` is true
    /// only for the `Upgrading:` section (whose package lines carry a
    /// `replacing` sub-line).
    Section { group: &'static str, upgrading: bool },
    /// Immediately after an upgrade package line: the next line must be its
    /// `replacing` sub-line, naming `name`.
    Replacing { name: String },
    /// After the blank line that ends the sections: expect `Transaction Summary:`.
    SummaryHeader,
    /// Inside the summary: expect ` Label: N package(s)` lines, a trailing
    /// blank, or end of output.
    Summary,
    /// After the trailing blank: only further blank lines are tolerated.
    End,
}

/// One whitespace-delimited transaction-table row (package or `replacing` line).
struct PkgRow<'a> {
    name: &'a str,
    arch: &'a str,
    version: &'a str,
    repo: &'a str,
    size: u64,
}

/// Maps a table section header (without its trailing `:`) to the aggregated
/// label dnf uses for it in the Transaction Summary. `None` means the header is
/// unknown — treated as a hard error so new dnf section types surface loudly
/// rather than being silently skipped.
fn summary_group(section: &str) -> Option<&'static str> {
    Some(match section {
        "Installing" | "Installing dependencies" | "Installing weak dependencies" => "Installing",
        "Upgrading" => "Upgrading",
        "Downgrading" => "Downgrading",
        "Reinstalling" => "Reinstalling",
        "Removing" | "Removing dependent packages" | "Removing unused dependencies" => "Removing",
        _ => return None,
    })
}

/// A column-0, non-empty line ending in `:` is a section/summary header; returns
/// the text without the trailing `:`.
fn section_header(line: &str) -> Option<&str> {
    if line.is_empty() || line.starts_with(' ') {
        return None;
    }
    line.strip_suffix(':')
}

fn is_column_header(line: &str) -> bool {
    line.split_whitespace().collect::<Vec<_>>() == ["Package", "Arch", "Version", "Repository", "Size"]
}

/// Parses a single-space-indented, six-field transaction row. Rejects any other
/// indent (e.g. the three-space `replacing` lines) or field count.
fn parse_package_row(line: &str) -> Result<PkgRow<'_>> {
    if !line.starts_with(' ') || line.starts_with("  ") {
        bail!("expected a single-space-indented package line");
    }
    let fields: Vec<&str> = line.split_whitespace().collect();
    if fields.len() != 6 {
        bail!("package line has {} fields, expected 6", fields.len());
    }
    let size = parse_dnf_size(fields[4], fields[5]).context("parsing package size")?;
    Ok(PkgRow { name: fields[0], arch: fields[1], version: fields[2], repo: fields[3], size })
}

/// Parses a summary count line such as ` Upgrading:        215 packages` into
/// (label, count). The label is everything before the count, sans its `:`.
fn parse_summary_count(line: &str) -> Result<(String, usize)> {
    if !line.starts_with(' ') || line.starts_with("  ") {
        bail!("expected a single-space-indented summary line");
    }
    let fields: Vec<&str> = line.split_whitespace().collect();
    if fields.len() < 3 {
        bail!("summary line has {} fields, expected at least 3", fields.len());
    }
    let (unit, count, label_tokens) =
        (fields[fields.len() - 1], fields[fields.len() - 2], &fields[..fields.len() - 2]);
    if unit != "packages" && unit != "package" {
        bail!("summary line must end with 'package(s)', got {unit:?}");
    }
    let count: usize = count.parse().with_context(|| format!("invalid summary count {count:?}"))?;
    let label = label_tokens.join(" ");
    let label = label
        .strip_suffix(':')
        .ok_or_else(|| anyhow::anyhow!("summary label {label:?} must end with ':'"))?
        .to_string();
    Ok((label, count))
}

/// Strict state machine over the entire `dnf upgrade` stdout transaction table.
///
/// Every line must match the pattern the current state expects; any deviation —
/// unknown line shape, wrong field count, a missing/orphan `replacing` sub-line,
/// an unknown section header, truncated output, or a Transaction Summary count
/// that disagrees with the sections actually parsed — is a hard error. This
/// surfaces a change in dnf's output format immediately instead of silently
/// misbehaving.
fn parse_update_lines(stdout: &str) -> Result<Vec<PackageUpdate>> {
    let mut parser = TableParser::new();
    for (idx, line) in stdout.lines().enumerate() {
        parser
            .feed(line)
            .with_context(|| format!("at stdout line {}: {line:?}", idx + 1))?;
    }
    parser.finish()
}

struct TableParser {
    state: State,
    updates: Vec<PackageUpdate>,
    /// Packages seen per summary bucket, keyed by `summary_group` label.
    section_counts: BTreeMap<String, usize>,
    /// Labels and counts parsed from the Transaction Summary.
    summary_counts: BTreeMap<String, usize>,
    /// Total number of `replacing` sub-lines seen across all sections.
    replacing_count: usize,
}

impl TableParser {
    fn new() -> Self {
        TableParser {
            state: State::Header,
            updates: Vec::new(),
            section_counts: BTreeMap::new(),
            summary_counts: BTreeMap::new(),
            replacing_count: 0,
        }
    }

    fn feed(&mut self, line: &str) -> Result<()> {
        // Take ownership of the state so `Replacing { name }` can be matched by value.
        self.state = match std::mem::replace(&mut self.state, State::Header) {
            State::Header => self.on_header(line)?,
            State::ExpectSection => self.on_expect_section(line)?,
            State::Section { group, upgrading } => self.on_section(line, group, upgrading)?,
            State::Replacing { name } => self.on_replacing(line, &name)?,
            State::SummaryHeader => self.on_summary_header(line)?,
            State::Summary => self.on_summary(line)?,
            State::End => self.on_end(line)?,
        };
        Ok(())
    }

    fn on_header(&self, line: &str) -> Result<State> {
        if line.is_empty() {
            Ok(State::Header)
        } else if is_column_header(line) {
            Ok(State::ExpectSection)
        } else if line.trim() == "Nothing to do." {
            Ok(State::End)
        } else {
            bail!("expected the column header 'Package Arch Version Repository Size'");
        }
    }

    fn on_expect_section(&self, line: &str) -> Result<State> {
        let name = section_header(line).ok_or_else(|| anyhow::anyhow!("expected a section header"))?;
        let group = summary_group(name).ok_or_else(|| anyhow::anyhow!("unknown section header {name:?}"))?;
        Ok(State::Section { group, upgrading: name == "Upgrading" })
    }

    fn on_section(&mut self, line: &str, group: &'static str, upgrading: bool) -> Result<State> {
        if line.is_empty() {
            return Ok(State::SummaryHeader);
        }
        if let Some(name) = section_header(line) {
            let group = summary_group(name).ok_or_else(|| anyhow::anyhow!("unknown section header {name:?}"))?;
            return Ok(State::Section { group, upgrading: name == "Upgrading" });
        }
        let row = parse_package_row(line)?;
        *self.section_counts.entry(group.to_string()).or_default() += 1;
        if upgrading {
            self.updates.push(PackageUpdate {
                name: row.name.to_string(),
                arch: row.arch.to_string(),
                new_version: normalize_version(row.version),
                old_version: String::new(),
                old_repo: String::new(),
                new_repo: row.repo.to_string(),
                download_size: row.size,
            });
            Ok(State::Replacing { name: row.name.to_string() })
        } else {
            Ok(State::Section { group, upgrading })
        }
    }

    fn on_replacing(&mut self, line: &str, name: &str) -> Result<State> {
        let rest = line
            .strip_prefix("   replacing ")
            .ok_or_else(|| anyhow::anyhow!("expected a 'replacing' sub-line for {name:?}"))?;
        let fields: Vec<&str> = rest.split_whitespace().collect();
        if fields.len() != 6 {
            bail!("'replacing' line has {} fields, expected 6", fields.len());
        }
        if fields[0] != name {
            bail!("'replacing' references {:?} but expected {name:?}", fields[0]);
        }
        let update = self.updates.last_mut().expect("an upgrade package precedes every replacing line");
        update.old_version = normalize_version(fields[2]);
        update.old_repo = fields[3].to_string();
        self.replacing_count += 1;
        Ok(State::Section { group: "Upgrading", upgrading: true })
    }

    fn on_summary_header(&self, line: &str) -> Result<State> {
        if section_header(line) == Some("Transaction Summary") {
            Ok(State::Summary)
        } else {
            bail!("expected 'Transaction Summary:'");
        }
    }

    fn on_summary(&mut self, line: &str) -> Result<State> {
        if line.is_empty() {
            return Ok(State::End);
        }
        let (label, count) = parse_summary_count(line)?;
        if self.summary_counts.insert(label.clone(), count).is_some() {
            bail!("duplicate summary label {label:?}");
        }
        Ok(State::Summary)
    }

    fn on_end(&self, line: &str) -> Result<State> {
        if line.is_empty() {
            Ok(State::End)
        } else {
            bail!("unexpected content after the transaction summary");
        }
    }

    fn finish(mut self) -> Result<Vec<PackageUpdate>> {
        match self.state {
            // No table at all (empty stdout or `Nothing to do.`) → nothing to upgrade.
            State::Header | State::End if self.updates.is_empty() && self.summary_counts.is_empty() => {
                return Ok(Vec::new());
            }
            State::Summary | State::End => {}
            other => bail!("dnf output ended unexpectedly in state {other:?}"),
        }

        // Cross-check: the parsed sections must reproduce the Transaction Summary
        // exactly. `replacing` lines map to the summary's `Replacing` bucket.
        let mut expected = self.section_counts.clone();
        if self.replacing_count > 0 {
            expected.insert("Replacing".to_string(), self.replacing_count);
        }
        if expected != self.summary_counts {
            bail!(
                "transaction summary disagrees with the parsed table\n  parsed:  {expected:?}\n  summary: {:?}",
                self.summary_counts
            );
        }

        self.updates.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(self.updates)
    }
}

fn normalize_version(v: &str) -> String {
    v.strip_prefix("0:").unwrap_or(v).to_string()
}

fn parse_dnf_size(number: &str, unit: &str) -> Result<u64> {
    let n: f64 = number.parse().with_context(|| format!("invalid size number: {number:?}"))?;
    Ok(match unit {
        "GiB" => (n * (1u64 << 30) as f64) as u64,
        "MiB" => (n * (1u64 << 20) as f64) as u64,
        "KiB" => (n * (1u64 << 10) as f64) as u64,
        "B" => n as u64,
        _ => bail!("unknown size unit: {unit:?}"),
    })
}

fn format_size(bytes: u64) -> String {
    if bytes >= 1 << 30 {
        format!("{:.1} GiB", bytes as f64 / (1u64 << 30) as f64)
    } else if bytes >= 1 << 20 {
        format!("{:.1} MiB", bytes as f64 / (1u64 << 20) as f64)
    } else if bytes >= 1 << 10 {
        format!("{:.1} KiB", bytes as f64 / (1u64 << 10) as f64)
    } else {
        format!("{} B", bytes)
    }
}

fn highlight_diff(old: &str, new: &str) -> (String, String) {
    let prefix_len = old
        .bytes()
        .zip(new.bytes())
        .take_while(|(a, b)| a == b)
        .count();

    let old_rest = &old[prefix_len..];
    let new_rest = &new[prefix_len..];

    let suffix_len = old_rest
        .bytes()
        .rev()
        .zip(new_rest.bytes().rev())
        .take_while(|(a, b)| a == b)
        .count();

    let prefix = &old[..prefix_len];
    let old_mid = &old_rest[..old_rest.len() - suffix_len];
    let new_mid = &new_rest[..new_rest.len() - suffix_len];
    let suffix = &old_rest[old_rest.len() - suffix_len..];

    let old_str = format!("{}{}{}", prefix.dimmed(), old_mid.red().bold(), suffix.dimmed());
    let new_str = format!("{}{}{}", prefix.dimmed(), new_mid.green().bold(), suffix.dimmed());

    (old_str, new_str)
}

fn shorten_repo(repo: &str) -> String {
    if repo.len() >= 20 && repo.bytes().all(|b| b.is_ascii_hexdigit()) {
        format!("{}..{}", &repo[..2], &repo[repo.len() - 2..])
    } else {
        repo.to_string()
    }
}

fn display_updates(updates: &[PackageUpdate], show_arch: bool, group: GroupBy, size_info: &SizeInfo) {
    let count = updates.len();

    let size_str = match (size_info.download, size_info.net_disk) {
        (Some(dl), Some(disk)) => {
            let disk_str = if disk >= 0 {
                format!("+{}", format_size(disk as u64))
            } else {
                format!("-{}", format_size((-disk) as u64))
            };
            format!("{} download, {} disk", format_size(dl), disk_str)
        }
        (Some(dl), None) => format_size(dl),
        (None, Some(disk)) => {
            if disk >= 0 {
                format!("+{} disk", format_size(disk as u64))
            } else {
                format!("-{} disk", format_size((-disk) as u64))
            }
        }
        (None, None) => {
            let total: u64 = updates.iter().map(|u| u.download_size).sum();
            format_size(total)
        }
    };

    println!(
        "{}",
        format!(
            " :: {} package{} to upgrade  ({})",
            count,
            if count == 1 { "" } else { "s" },
            size_str,
        )
        .cyan()
        .bold()
    );
    println!();

    let max_name = updates.iter().map(|u| u.name.len()).max().unwrap_or(0);
    let max_arch = updates.iter().map(|u| u.arch.len()).max().unwrap_or(0);
    let max_old = updates.iter().map(|u| u.old_version.len()).max().unwrap_or(0);
    let max_new = updates.iter().map(|u| u.new_version.len()).max().unwrap_or(0);
    let max_size = updates
        .iter()
        .map(|u| format_size(u.download_size).len())
        .max()
        .unwrap_or(0);

    let print_row = |update: &PackageUpdate| {
        let (old_ver, new_ver) = highlight_diff(&update.old_version, &update.new_version);

        let name_padded = format!("{:<max_name$}", update.name);
        let old_pad = " ".repeat(max_old.saturating_sub(update.old_version.len()));
        let new_pad = " ".repeat(max_new.saturating_sub(update.new_version.len()));
        let size_str = format_size(update.download_size);
        let size_pad = " ".repeat(max_size.saturating_sub(size_str.len()));

        let old_repo = shorten_repo(&update.old_repo);
        let new_repo = shorten_repo(&update.new_repo);
        let repo_display = if update.old_repo.is_empty() || update.old_repo == update.new_repo {
            new_repo.dimmed().to_string()
        } else {
            let (old_r, new_r) = highlight_diff(&old_repo, &new_repo);
            format!("{} -> {}", old_r, new_r)
        };

        let arch_col = if show_arch {
            format!("  {}", format!("{:<max_arch$}", update.arch).dimmed())
        } else {
            String::new()
        };

        println!(
            "    {}{}  {}{} -> {}{}  {}{}  {}",
            name_padded.bold(),
            arch_col,
            old_ver,
            old_pad,
            new_ver,
            new_pad,
            size_pad,
            size_str.dimmed(),
            repo_display,
        );
    };

    match group {
        GroupBy::None => {
            for update in updates {
                print_row(update);
            }
        }
        GroupBy::Repository => {
            let mut order: Vec<&PackageUpdate> = updates.iter().collect();
            order.sort_by(|a, b| a.new_repo.cmp(&b.new_repo).then_with(|| a.name.cmp(&b.name)));
            let mut current: Option<&str> = None;
            for update in order {
                if current != Some(update.new_repo.as_str()) {
                    if current.is_some() {
                        println!();
                    }
                    current = Some(update.new_repo.as_str());
                    println!("  {}", shorten_repo(&update.new_repo).underline().bold());
                }
                print_row(update);
            }
        }
    }
}

fn upgrade_specs(updates: &[PackageUpdate]) -> Vec<String> {
    // name-[epoch:]version-release.arch — pins dnf to exactly what was displayed
    updates
        .iter()
        .map(|u| format!("{}-{}.{}", u.name, u.new_version, u.arch))
        .collect()
}

fn do_upgrade(updates: &[PackageUpdate]) {
    let status = Command::new(DNF)
        .arg("upgrade")
        .arg("-y")
        .args(upgrade_specs(updates))
        .status()
        .expect("failed to run dnf upgrade");

    std::process::exit(status.code().unwrap_or(1));
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A compact transaction exercising every line type: column header, a
    /// non-upgrade section (Removing), the Upgrading section with `replacing`
    /// sub-lines, another non-upgrade section (Installing), the blank/summary
    /// boundary, all four summary buckets, and a trailing blank line.
    const SAMPLE: &str = "\
Package    Arch   Version          Repository   Size
Removing:
 oldpkg    x86_64 0:1.0-1.fc44     updates      1.0 MiB
Upgrading:
 bar       noarch 1:2.0-1.fc44     fedora       0.0   B
   replacing bar noarch 1:1.9-1.fc44 fedora     0.0   B
 foo       x86_64 0:2.0-1.fc44     updates      3.3 MiB
   replacing foo x86_64 0:1.0-1.fc44 <unknown>  3.3 MiB
Installing:
 newpkg    x86_64 0:3.0-1.fc44     updates      2.0 MiB

Transaction Summary:
 Installing:   1 package
 Upgrading:    2 packages
 Replacing:    2 packages
 Removing:     1 package
";

    #[test]
    fn parses_sample_transaction() {
        let updates = parse_update_lines(SAMPLE).expect("sample parses");
        // Only Upgrading packages become updates, sorted by name.
        assert_eq!(updates.len(), 2);

        assert_eq!(updates[0].name, "bar");
        assert_eq!(updates[0].arch, "noarch");
        assert_eq!(updates[0].old_version, "1:1.9-1.fc44"); // non-zero epoch preserved
        assert_eq!(updates[0].new_version, "1:2.0-1.fc44");
        assert_eq!(updates[0].old_repo, "fedora");
        assert_eq!(updates[0].new_repo, "fedora");

        assert_eq!(updates[1].name, "foo");
        assert_eq!(updates[1].old_version, "1.0-1.fc44"); // `0:` epoch stripped
        assert_eq!(updates[1].new_version, "2.0-1.fc44");
        assert_eq!(updates[1].old_repo, "<unknown>");
        assert_eq!(updates[1].new_repo, "updates");
        assert_eq!(updates[1].download_size, (3.3 * (1u64 << 20) as f64) as u64);
    }

    #[test]
    fn parses_real_world_capture() {
        // A full 215-upgrade transaction captured from dnf5 5.4.2.1.
        let stdout = include_str!("testdata/dnf_upgrade_stdout.txt");
        let updates = parse_update_lines(stdout).expect("real capture parses");
        assert_eq!(updates.len(), 215);
        assert!(updates.windows(2).all(|w| w[0].name <= w[1].name), "sorted by name");
        // Spot-check a package with a multi-digit epoch.
        let bind = updates.iter().find(|u| u.name == "bind-libs").expect("bind-libs present");
        assert_eq!(bind.old_version, "32:9.18.49-1.fc44");
        assert_eq!(bind.new_version, "32:9.18.50-1.fc44");
    }

    #[test]
    fn empty_output_is_no_updates() {
        assert!(parse_update_lines("").unwrap().is_empty());
    }

    #[test]
    fn nothing_to_do_is_no_updates() {
        assert!(parse_update_lines("Nothing to do.\n").unwrap().is_empty());
    }

    fn err(stdout: &str) -> String {
        format!("{:#}", parse_update_lines(stdout).unwrap_err())
    }

    #[test]
    fn rejects_missing_column_header() {
        assert!(err("Removing:\n oldpkg x86_64 0:1.0-1.fc44 updates 1.0 MiB\n").contains("column header"));
    }

    #[test]
    fn rejects_unknown_section_header() {
        let s = "Package Arch Version Repository Size\nFrobnicating:\n";
        assert!(err(s).contains("unknown section header"));
    }

    #[test]
    fn rejects_wrong_field_count() {
        let s = "Package Arch Version Repository Size\nRemoving:\n oldpkg x86_64 0:1.0-1.fc44 updates\n";
        assert!(err(s).contains("expected 6"));
    }

    #[test]
    fn rejects_missing_replacing_line() {
        let s = "\
Package Arch Version Repository Size
Upgrading:
 foo x86_64 0:2.0-1.fc44 updates 3.3 MiB
 bar x86_64 0:2.0-1.fc44 updates 3.3 MiB
";
        assert!(err(s).contains("replacing"));
    }

    #[test]
    fn rejects_replacing_name_mismatch() {
        let s = "\
Package Arch Version Repository Size
Upgrading:
 foo x86_64 0:2.0-1.fc44 updates 3.3 MiB
   replacing bar x86_64 0:1.0-1.fc44 updates 3.3 MiB
";
        assert!(err(s).contains("expected \"foo\""));
    }

    #[test]
    fn rejects_truncated_output() {
        let s = "\
Package Arch Version Repository Size
Upgrading:
 foo x86_64 0:2.0-1.fc44 updates 3.3 MiB
   replacing foo x86_64 0:1.0-1.fc44 updates 3.3 MiB
";
        assert!(err(s).contains("ended unexpectedly"));
    }

    #[test]
    fn rejects_summary_count_mismatch() {
        // Summary claims 2 upgrades but only 1 was listed.
        let s = "\
Package Arch Version Repository Size
Upgrading:
 foo x86_64 0:2.0-1.fc44 updates 3.3 MiB
   replacing foo x86_64 0:1.0-1.fc44 updates 3.3 MiB

Transaction Summary:
 Upgrading: 2 packages
 Replacing: 1 package
";
        assert!(err(s).contains("disagrees with the parsed table"));
    }

    #[test]
    fn rejects_orphan_replacing_line() {
        let s = "\
Package Arch Version Repository Size
Upgrading:
   replacing foo x86_64 0:1.0-1.fc44 updates 3.3 MiB
";
        // A replacing line with no preceding package is a single-space/indent mismatch.
        assert!(parse_update_lines(s).is_err());
    }
}
