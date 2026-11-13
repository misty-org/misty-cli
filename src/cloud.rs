//! `misty cloud`: misty-cloud's releases and its production billing stack.
//!
//! Both work on the misty-cloud checkout around the current folder (on the VPS, that is the only
//! checkout there), or else the one beside misty.

use std::{
    fs,
    path::{Path, PathBuf},
};

use anyhow::{bail, Context, Result};
use clap::{Args, Subcommand};

use crate::{hooks::Repo, process::CommandSpec, workspace::Workspace};

const IMAGE: &str = "ghcr.io/misty-org/misty-cloud";
const ENV_FILE: &str = ".env/prod/billing.env";

#[derive(Debug, Args)]
pub struct Cloud {
    #[command(subcommand)]
    pub command: CloudCommand,
}

#[derive(Debug, Subcommand)]
pub enum CloudCommand {
    /// Build the billing image here, push it to GHCR, tag the commit and save the digest.
    Release {
        /// The release version, such as 0.1.0.
        version: String,
    },
    /// Operate the production billing stack on the VPS.
    Prod {
        #[command(subcommand)]
        command: ProdCommand,
    },
}

#[derive(Debug, Subcommand)]
pub enum ProdCommand {
    /// Validate .env/prod and the Compose file.
    Check,
    /// Check, pull and start.
    Up,
    /// Stop the stack.
    Down,
    /// Follow the billing service's logs.
    Logs,
    /// Make this database the charging authority (once, on a fresh database).
    EnableWriter {
        #[arg(long)]
        yes: bool,
    },
}

pub fn dispatch(workspace: &Workspace, command: CloudCommand) -> Result<()> {
    let root = checkout(workspace)?;
    match command {
        CloudCommand::Release { version } => release(&root, &version),
        CloudCommand::Prod { command } => match command {
            ProdCommand::Check => check(&root),
            ProdCommand::Up => {
                check(&root)?;
                compose(&root, &["pull"])?;
                compose(&root, &["up", "--detach"])
            }
            ProdCommand::Down => compose(&root, &["down", "--remove-orphans"]),
            ProdCommand::Logs => compose(&root, &["logs", "--follow", "billing"]),
            ProdCommand::EnableWriter { yes } => {
                if !yes {
                    bail!("This makes this database the charging authority; rerun with --yes.");
                }
                check(&root)?;
                prod_compose(&[
                    "exec", "-T", "postgres", "psql", "-U", "billing_owner", "-d",
                    "misty_billing", "-v", "ON_ERROR_STOP=1", "-q",
                ])
                .run_with_input(&root, ENABLE_WRITER.as_bytes())?;
                println!("Billing writer enabled.");
                Ok(())
            }
        },
    }
}

const ENABLE_WRITER: &str = "BEGIN;
SELECT pg_advisory_xact_lock(842315792);
UPDATE billing.usage_writer
SET enabled = true,
    cutover_evidence = jsonb_build_object('kind', 'fresh_production', 'enabled_at', now())
WHERE id AND NOT enabled;
COMMIT;
";

fn checkout(workspace: &Workspace) -> Result<PathBuf> {
    let here = std::env::current_dir()
        .ok()
        .and_then(|directory| crate::hooks::toplevel(&directory).ok())
        .filter(|root| root.join("cmd/misty-cloud").is_dir());
    let root = here.unwrap_or_else(|| workspace.cloud.clone());
    if !root.join("compose.prod.yml").is_file() {
        bail!(
            "This needs a misty-cloud checkout, here or at {}",
            workspace.cloud.display()
        );
    }
    Ok(root)
}

fn prod_compose(arguments: &[&str]) -> CommandSpec {
    CommandSpec::new("docker")
        .env("COMPOSE_DISABLE_ENV_FILE", "1")
        .args(["compose", "--env-file", ENV_FILE, "--file", "compose.prod.yml"])
        .args(arguments)
}

fn compose(root: &Path, arguments: &[&str]) -> Result<()> {
    prod_compose(arguments).run(root)
}

/// Each production env file must be private, and billing.env must hold strong, pinned values.
fn check(root: &Path) -> Result<()> {
    private(&root.join(ENV_FILE), true)?;
    private(&root.join(".env/prod/stripe.env"), false)?;
    private(&root.join(".env/prod/misty-server.env"), false)?;
    let values: Vec<(String, String)> = dotenvy::from_path_iter(root.join(ENV_FILE))?
        .collect::<Result<_, _>>()
        .with_context(|| format!("could not read {ENV_FILE}"))?;
    let value = |name: &str| {
        values
            .iter()
            .rev()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.clone())
            .or_else(|| std::env::var(name).ok())
            .filter(|value| !value.is_empty())
    };
    for name in [
        "MISTY_BILLING_IMAGE",
        "BILLING_OWNER_PASSWORD",
        "BILLING_RUNTIME_PASSWORD",
        "MISTY_BILLING_SECRET",
    ] {
        if value(name).is_none() {
            bail!("{name} is missing from {ENV_FILE}.");
        }
    }
    for name in ["BILLING_OWNER_PASSWORD", "BILLING_RUNTIME_PASSWORD"] {
        if !lowercase_hex(&value(name).unwrap_or_default(), 32) {
            bail!("{name} must be lowercase hex (openssl rand -hex 32).");
        }
    }
    if value("MISTY_BILLING_SECRET").unwrap_or_default().len() < 32 {
        bail!("MISTY_BILLING_SECRET must contain at least 32 bytes.");
    }
    if !value("MISTY_BILLING_IMAGE")
        .unwrap_or_default()
        .contains("@sha256:")
    {
        bail!("MISTY_BILLING_IMAGE must be pinned by digest.");
    }
    prod_compose(&["config", "--quiet"]).run(root)?;
    println!("Billing production configuration is valid.");
    Ok(())
}

fn private(path: &Path, required: bool) -> Result<()> {
    let Ok(metadata) = fs::metadata(path) else {
        if required {
            bail!("Missing {}.", path.display());
        }
        return Ok(());
    };
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if metadata.permissions().mode() & 0o777 != 0o600 {
            bail!(
                "{0} must be mode 0600 (chmod 600 {0}).",
                path.display()
            );
        }
    }
    let _ = metadata;
    Ok(())
}

pub(crate) fn lowercase_hex(value: &str, minimum: usize) -> bool {
    value.len() >= minimum && value.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

pub(crate) fn valid_version(version: &str) -> bool {
    let (core, suffix) = match version.split_once('-') {
        Some((core, suffix)) => (core, Some(suffix)),
        None => (version, None),
    };
    let parts: Vec<&str> = core.split('.').collect();
    parts.len() == 3
        && parts
            .iter()
            .all(|part| !part.is_empty() && part.bytes().all(|b| b.is_ascii_digit()))
        && suffix.is_none_or(|suffix| {
            !suffix.is_empty()
                && suffix
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'.')
        })
}

/// Releases build on this computer, never on GitHub: from a clean, pushed commit it runs the
/// checks, builds the linux/amd64 image, pushes it, tags the commit and saves the digest.
fn release(root: &Path, version: &str) -> Result<()> {
    if !valid_version(version) {
        bail!("Versions look like 0.1.0.");
    }
    let tag = format!("v{version}");
    let git = |arguments: &[&str]| crate::hooks::git(root, arguments);
    let quiet = |arguments: &[&str]| CommandSpec::new("git").args(arguments).succeeds(root);
    if !quiet(&["diff", "--quiet"]) || !quiet(&["diff", "--cached", "--quiet"]) {
        bail!("Commit or discard local changes first.");
    }
    git(&["fetch", "--quiet", "origin"])?;
    if git(&["rev-parse", "HEAD"])?.trim() != git(&["rev-parse", "@{upstream}"])?.trim() {
        bail!("Push this commit first, so every release is a commit on GitHub.");
    }
    if quiet(&["rev-parse", "-q", "--verify", &format!("refs/tags/{tag}")]) {
        bail!("{tag} already exists.");
    }
    let env_file = root.join(ENV_FILE);
    if !env_file.is_file() {
        bail!("Create {ENV_FILE} first (misty env init prod does).");
    }

    for suite in ["secrets", "go"] {
        Repo::Cloud.run(root, suite)?;
    }

    let sha = git(&["rev-parse", "HEAD"])?.trim().to_string();
    let metadata = tempfile::NamedTempFile::new()?;
    CommandSpec::new("docker")
        .args(["buildx", "build", "--platform", "linux/amd64", "--provenance=false", "--push"])
        .args(["--file", "deploy/Dockerfile", "--tag"])
        .arg(format!("{IMAGE}:{version}"))
        .arg("--tag")
        .arg(format!("{IMAGE}:sha-{sha}"))
        .arg("--metadata-file")
        .arg(metadata.path().as_os_str())
        .arg(".")
        .run(root)?;
    let built: serde_json::Value = serde_json::from_slice(&fs::read(metadata.path())?)?;
    let Some(digest) = built["containerimage.digest"].as_str() else {
        bail!("docker buildx did not report the image digest");
    };

    git(&["tag", "-a", &tag, "-m", &format!("Misty billing {version}"), &sha])?;
    // The commit was already checked above.
    git(&["push", "--quiet", "--no-verify", "origin", &tag])?;

    // Replace the image line in place; the file keeps its 0600 mode.
    let reference = format!("{IMAGE}@{digest}");
    let mut lines: Vec<String> = fs::read_to_string(&env_file)?
        .lines()
        .filter(|line| !line.starts_with("MISTY_BILLING_IMAGE="))
        .map(str::to_string)
        .collect();
    lines.push(format!("MISTY_BILLING_IMAGE={reference}"));
    fs::write(&env_file, lines.join("\n") + "\n")?;
    println!("Released {tag}: MISTY_BILLING_IMAGE={reference} (saved to {ENV_FILE})");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn release_versions_and_production_secrets_are_validated() {
        for good in ["0.1.0", "12.0.3", "1.2.3-rc.1"] {
            assert!(valid_version(good), "{good}");
        }
        for bad in ["1.2", "v1.2.3", "1.2.3-", "1.2.3-rc_1", "1..3"] {
            assert!(!valid_version(bad), "{bad}");
        }
        assert!(lowercase_hex(&"ab12".repeat(8), 32));
        assert!(!lowercase_hex(&"AB12".repeat(8), 32));
        assert!(!lowercase_hex("ab12", 32));
    }
}
