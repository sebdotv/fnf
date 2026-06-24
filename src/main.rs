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
    let updates = check_updates().context("checking for updates")?;

    if updates.is_empty() {
        println!("{}", " :: System is up to date.".green().bold());
        return Ok(());
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

fn check_updates() -> Result<Vec<PackageUpdate>> {
    let output = Command::new(DNF)
        .args(["upgrade", "--assumeno", "--color=never"])
        .stderr(Stdio::inherit())
        .output()
        .context("running dnf upgrade --assumeno")?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    Ok(parse_update_lines(&stdout))
}

fn parse_update_lines(stdout: &str) -> Vec<PackageUpdate> {
    let mut updates: HashMap<String, PackageUpdate> = HashMap::new();
    let mut in_upgrading = false;

    for line in stdout.lines() {
        if !line.starts_with(' ') {
            in_upgrading = line.trim_end() == "Upgrading:";
            continue;
        }

        if !in_upgrading {
            continue;
        }

        if let Some(rest) = line.strip_prefix("   replacing ") {
            let parts: Vec<&str> = rest.split_whitespace().collect();
            if parts.len() >= 3 {
                if let Some(u) = updates.get_mut(parts[0]) {
                    u.old_version = normalize_version(parts[2]);
                }
            }
        } else {
            let parts: Vec<&str> = line.split_whitespace().collect();
            if parts.len() >= 6 {
                let key = parts[0].to_string();
                updates.insert(
                    key,
                    PackageUpdate {
                        name: parts[0].to_string(),
                        arch: parts[1].to_string(),
                        new_version: normalize_version(parts[2]),
                        old_version: String::new(),
                        repo: parts[3].to_string(),
                        download_size: parse_dnf_size(parts[4], parts[5]),
                    },
                );
            }
        }
    }

    let mut updates: Vec<PackageUpdate> = updates
        .into_values()
        .filter(|u| !u.old_version.is_empty())
        .collect();
    updates.sort_by(|a, b| a.name.cmp(&b.name));
    updates
}

fn normalize_version(v: &str) -> String {
    v.strip_prefix("0:").unwrap_or(v).to_string()
}

fn parse_dnf_size(number: &str, unit: &str) -> u64 {
    let n: f64 = number.parse().unwrap_or(0.0);
    match unit {
        "GiB" => (n * (1u64 << 30) as f64) as u64,
        "MiB" => (n * (1u64 << 20) as f64) as u64,
        "KiB" => (n * (1u64 << 10) as f64) as u64,
        _ => n as u64,
    }
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
