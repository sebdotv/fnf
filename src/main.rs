use std::collections::HashMap;
use std::io::{self, Write};
use std::process::{Command, Stdio};

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use colored::Colorize;

#[derive(Debug)]
struct PackageUpdate {
    name: String,
    arch: String,
    old_version: String,
    new_version: String,
    repo: String,
    download_size: u64,
}

const DNF: &str = "/usr/bin/dnf";

const KNOWN_ARCHES: &[&str] = &[
    "x86_64", "i686", "i386", "noarch", "aarch64", "armv7hl", "ppc64le", "s390x", "src",
];

#[derive(Parser)]
#[command(name = "fnf", about = "Fancified YUM — dnf wrapper with improved upgrade output")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    #[command(alias = "up", alias = "update", about = "Upgrade all packages")]
    Upgrade,
}

fn main() {
    let cli = Cli::parse();

    let result = match cli.command {
        Commands::Upgrade => run_upgrade_wrapper(),
    };

    if let Err(e) = result {
        eprintln!("Error: {e:#}");
        std::process::exit(1);
    }
}

fn run_upgrade_wrapper() -> Result<()> {
    let (updates, exit_code) = check_updates().context("checking for updates")?;

    if exit_code == 0 {
        println!("{}", " :: System is up to date.".green().bold());
        return Ok(());
    }

    if exit_code != 100 {
        eprintln!("dnf check-update exited with code {exit_code}");
        std::process::exit(exit_code);
    }

    if updates.is_empty() {
        eprintln!("Could not parse dnf output. Run 'dnf upgrade' directly.");
        std::process::exit(1);
    }

    display_updates(&updates);

    print!("\n{} ", "==> Proceed with upgrade? [Y/n]".bold());
    io::stdout().flush()?;

    let mut input = String::new();
    io::stdin().read_line(&mut input)?;
    let answer = input.trim().to_lowercase();

    if answer.is_empty() || answer == "y" || answer == "yes" {
        do_upgrade();
    } else {
        println!("{}", "Operation cancelled.".yellow());
    }

    Ok(())
}

fn check_updates() -> Result<(Vec<PackageUpdate>, i32)> {
    let output = Command::new(DNF)
        .args(["check-update", "--color=never"])
        .stderr(Stdio::inherit())
        .output()
        .context("running dnf check-update")?;

    let exit_code = output.status.code().unwrap_or(1);
    if exit_code != 100 {
        return Ok((vec![], exit_code));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let installed = get_installed_versions().context("querying installed packages")?;

    let mut updates: Vec<PackageUpdate> = stdout
        .lines()
        .filter_map(|line| parse_update_line(line, &installed))
        .collect();

    updates.sort_by(|a, b| a.name.cmp(&b.name));

    fetch_download_sizes(&mut updates).context("fetching download sizes")?;

    Ok((updates, exit_code))
}

fn parse_update_line(line: &str, installed: &HashMap<String, String>) -> Option<PackageUpdate> {
    let parts: Vec<&str> = line.split_whitespace().collect();
    if parts.len() < 3 {
        return None;
    }

    let pkg_arch = parts[0];
    let new_version_raw = parts[1];
    let repo = parts[2];

    let dot_pos = pkg_arch.rfind('.')?;
    let name = &pkg_arch[..dot_pos];
    let arch = &pkg_arch[dot_pos + 1..];

    if !KNOWN_ARCHES.contains(&arch) {
        return None;
    }

    let new_version = normalize_version(new_version_raw);
    let key = format!("{name}.{arch}");
    let old_version = normalize_version(installed.get(&key)?);

    Some(PackageUpdate {
        name: name.to_string(),
        arch: arch.to_string(),
        old_version,
        new_version,
        repo: repo.to_string(),
        download_size: 0,
    })
}

fn get_installed_versions() -> Result<HashMap<String, String>> {
    let output = Command::new("rpm")
        .args([
            "-qa",
            "--queryformat",
            "%{NAME}.%{ARCH} %{EPOCHNUM}:%{VERSION}-%{RELEASE}\n",
        ])
        .output()
        .context("running rpm -qa")?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let map = stdout
        .lines()
        .filter_map(|line| line.split_once(' ').map(|(k, v)| (k.to_string(), v.to_string())))
        .collect();

    Ok(map)
}

fn fetch_download_sizes(updates: &mut Vec<PackageUpdate>) -> Result<()> {
    if updates.is_empty() {
        return Ok(());
    }

    let mut cmd = Command::new("dnf");
    cmd.arg("repoquery")
        .arg("--queryformat")
        .arg("%{name}.%{arch} %{downloadsize}\n");

    for u in updates.iter() {
        cmd.arg(format!("{}-{}.{}", u.name, u.new_version, u.arch));
    }

    let output = cmd.output().context("running dnf repoquery")?;
    let stdout = String::from_utf8_lossy(&output.stdout);

    let sizes: HashMap<String, u64> = stdout
        .lines()
        .filter_map(|line| {
            let (pkg, size_str) = line.split_once(' ')?;
            let size = size_str.parse().ok()?;
            Some((pkg.to_string(), size))
        })
        .collect();

    for u in updates.iter_mut() {
        let key = format!("{}.{}", u.name, u.arch);
        u.download_size = sizes.get(&key).copied().unwrap_or(0);
    }

    Ok(())
}

fn normalize_version(v: &str) -> String {
    v.strip_prefix("0:").unwrap_or(v).to_string()
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

fn highlight_version_diff(old: &str, new: &str) -> (String, String) {
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

fn display_updates(updates: &[PackageUpdate]) {
    let count = updates.len();
    let total_size = updates.iter().map(|u| u.download_size).sum();

    println!(
        "{}",
        format!(
            " :: {} package{} to upgrade  ({})",
            count,
            if count == 1 { "" } else { "s" },
            format_size(total_size),
        )
        .cyan()
        .bold()
    );
    println!();

    let max_name = updates.iter().map(|u| u.name.len()).max().unwrap_or(0);
    let max_arch = updates.iter().map(|u| u.arch.len()).max().unwrap_or(0);
    let max_old = updates.iter().map(|u| u.old_version.len()).max().unwrap_or(0);
    let max_size = updates
        .iter()
        .map(|u| format_size(u.download_size).len())
        .max()
        .unwrap_or(0);

    for update in updates {
        let (old_colored, new_colored) =
            highlight_version_diff(&update.old_version, &update.new_version);

        let name_padded = format!("{:<max_name$}", update.name);
        let arch_padded = format!("{:<max_arch$}", update.arch);
        let old_pad = " ".repeat(max_old.saturating_sub(update.old_version.len()));
        let size_str = format_size(update.download_size);
        let size_pad = " ".repeat(max_size.saturating_sub(size_str.len()));

        println!(
            "    {}  {}  {}{} -> {}  {}{}  {}",
            name_padded.bold(),
            arch_padded.dimmed(),
            old_colored,
            old_pad,
            new_colored,
            size_pad,
            size_str.dimmed(),
            update.repo.dimmed(),
        );
    }
}

fn do_upgrade() {
    let status = Command::new(DNF)
        .args(["upgrade", "-y"])
        .status()
        .expect("failed to run dnf upgrade");

    std::process::exit(status.code().unwrap_or(1));
}
