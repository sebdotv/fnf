use std::collections::HashMap;
use std::io::{self, Write};
use std::process::{Command, Stdio};

use anyhow::{Context, Result};
use colored::Colorize;

#[derive(Debug)]
struct PackageUpdate {
    name: String,
    arch: String,
    old_version: String,
    new_version: String,
}

const DNF: &str = "/usr/bin/dnf";

const KNOWN_ARCHES: &[&str] = &[
    "x86_64", "i686", "i386", "noarch", "aarch64", "armv7hl", "ppc64le", "s390x", "src",
];

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let subcommand = args.first().map(String::as_str).unwrap_or("upgrade");

    let result = match subcommand {
        "upgrade" | "up" | "update" => run_upgrade_wrapper(),
        _ => {
            eprintln!("Usage: dnf [upgrade|up|update]");
            std::process::exit(1);
        }
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

    Ok((updates, exit_code))
}

fn parse_update_line(line: &str, installed: &HashMap<String, String>) -> Option<PackageUpdate> {
    let parts: Vec<&str> = line.split_whitespace().collect();
    if parts.len() < 3 {
        return None;
    }

    let pkg_arch = parts[0];
    let new_version_raw = parts[1];

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

fn normalize_version(v: &str) -> String {
    v.strip_prefix("0:").unwrap_or(v).to_string()
}

fn highlight_version_diff(old: &str, new: &str) -> (String, String) {
    let common_bytes = old
        .bytes()
        .zip(new.bytes())
        .take_while(|(a, b)| a == b)
        .count();

    let prefix = &old[..common_bytes];
    let old_suffix = &old[common_bytes..];
    let new_suffix = &new[common_bytes..];

    let old_str = format!("{}{}", prefix.dimmed(), old_suffix.red().bold());
    let new_str = format!("{}{}", prefix.dimmed(), new_suffix.green().bold());

    (old_str, new_str)
}

fn display_updates(updates: &[PackageUpdate]) {
    let count = updates.len();
    println!(
        "{}",
        format!(
            " :: {} package{} to upgrade",
            count,
            if count == 1 { "" } else { "s" }
        )
        .cyan()
        .bold()
    );
    println!();

    let max_name = updates.iter().map(|u| u.name.len()).max().unwrap_or(0);
    let max_arch = updates.iter().map(|u| u.arch.len()).max().unwrap_or(0);

    for update in updates {
        let (old_colored, new_colored) =
            highlight_version_diff(&update.old_version, &update.new_version);

        let name_padded = format!("{:<max_name$}", update.name);
        let arch_padded = format!("{:<max_arch$}", update.arch);

        println!(
            "    {}  {}  {} -> {}",
            name_padded.bold(),
            arch_padded.dimmed(),
            old_colored,
            new_colored,
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
