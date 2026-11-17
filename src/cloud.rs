//! `misty cloud`: misty-cloud's releases, its server pin, and its production stack.
//!
//! Misty Cloud's production stack (misty-cloud's compose.prod.yml) runs the open Misty server and
//! billing together on the VPS. `release`, `pin-server` and `ship` run on this computer, in the
//! misty-cloud git checkout; `prod` runs on the VPS, in the deploy folder `ship` fills (by
//! default ~/misty-cloud), which has no git.

use std::{
    fs,
    path::{Path, PathBuf},
    process::Stdio,
};

use anyhow::{bail, Context, Result};
use clap::{Args, Subcommand};

use crate::{hooks::Repo, process::CommandSpec, workspace::Workspace};

const IMAGE: &str = "ghcr.io/misty-org/misty-cloud";
const SERVER_IMAGE: &str = "ghcr.io/misty-org/misty-server";
/// The pins, committed in misty-cloud: which billing image and which server release run.
const BILLING_VERSION: &str = "deploy/billing/version.env";
const SERVER_VERSION: &str = "deploy/misty-server/version.env";
const SERVER_COMPOSE: &str = "deploy/misty-server/compose.yml";
const PROD_FILES: &str = "deploy/prod-files.txt";
/// Billing's one setting, made on the VPS and never copied.
const STRIPE_ENV: &str = ".env/prod/stripe.env";
/// Files billing used to need; billing-setup makes what they held now.
const RETIRED_FILES: [&str; 2] = [".env/prod/billing.env", ".env/prod/misty-server.env"];
/// Settings stripe.env used to hold that nothing reads now.
const RETIRED_STRIPE: [&str; 4] = [
    "STRIPE_PRICE_PLUS_MONTHLY",
    "STRIPE_PRICE_PLUS_YEARLY",
    "STRIPE_PRICE_BOOST_MONTHLY",
    "STRIPE_PORTAL_LOGIN_URL",
];

#[derive(Debug, Args)]
pub struct Cloud {
    #[command(subcommand)]
    pub command: CloudCommand,
}

#[derive(Debug, Subcommand)]
pub enum CloudCommand {
    /// Build the billing image here, push it to GHCR, tag the commit and pin its digest.
    Release {
        /// The release version, such as 0.1.0.
        version: String,
    },
    /// Pin the misty-server release production runs: its compose.yml and published images.
    PinServer {
        /// The misty-server commit, 40 lowercase hex characters.
        commit: String,
    },
    /// Copy the deploy files (deploy/prod-files.txt) to the VPS.
    Ship {
        #[arg(long, default_value = "misty-vps")]
        host: String,
        /// The deploy folder on the VPS, relative to its home.
        #[arg(long, default_value = "misty-cloud")]
        path: String,
        /// Then run `misty cloud prod up` there.
        #[arg(long)]
        up: bool,
    },
    /// Operate the production stack, on the VPS.
    Prod {
        #[command(subcommand)]
        command: ProdCommand,
    },
}

#[derive(Debug, Subcommand)]
pub enum ProdCommand {
    /// Validate the pins, the settings files, the volumes and the Compose file.
    Check,
    /// Check, pull and start the server and billing.
    Up,
    /// Stop the stack (its data stays).
    Down,
    /// Follow one service's logs.
    Logs {
        /// Such as billing, billing-setup, api or setup.
        #[arg(default_value = "billing")]
        service: String,
    },
    /// Make this database the charging authority (once, on a fresh database).
    EnableWriter {
        #[arg(long)]
        yes: bool,
    },
}

pub fn dispatch(workspace: &Workspace, command: CloudCommand) -> Result<()> {
    match command {
        CloudCommand::Release { version } => release(&checkout(workspace)?, &version),
        CloudCommand::PinServer { commit } => pin_server(&checkout(workspace)?, &commit),
        CloudCommand::Ship { host, path, up } => ship(&checkout(workspace)?, &host, &path, up),
        CloudCommand::Prod { command } => {
            let root = deploy_folder(workspace)?;
            match command {
                ProdCommand::Check => check(&root),
                ProdCommand::Up => up(&root),
                ProdCommand::Down => compose(&root, &["down", "--remove-orphans"]),
                ProdCommand::Logs { service } => compose(&root, &["logs", "--follow", &service]),
                ProdCommand::EnableWriter { yes } => {
                    if !yes {
                        bail!("This makes this database the charging authority; rerun with --yes.");
                    }
                    check(&root)?;
                    prod_compose(&[
                        "exec",
                        "-T",
                        "billing-postgres",
                        "psql",
                        "-U",
                        "billing_owner",
                        "-d",
                        "misty_billing",
                        "-v",
                        "ON_ERROR_STOP=1",
                        "-q",
                    ])
                    .run_with_input(&root, ENABLE_WRITER.as_bytes())?;
                    println!("Billing writer enabled.");
                    Ok(())
                }
            }
        }
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

/// The misty-cloud git checkout around the current folder, or the one beside misty.
fn checkout(workspace: &Workspace) -> Result<PathBuf> {
    let here = std::env::current_dir()
        .ok()
        .and_then(|directory| crate::hooks::toplevel(&directory).ok())
        .filter(|root| root.join("cmd/misty-cloud").is_dir());
    let root = here.unwrap_or_else(|| workspace.cloud.clone());
    if !root.join("cmd/misty-cloud").is_dir() || !root.join("compose.prod.yml").is_file() {
        bail!(
            "This needs the misty-cloud checkout, here or at {}",
            workspace.cloud.display()
        );
    }
    Ok(root)
}

fn is_deploy_folder(directory: &Path) -> bool {
    directory.join("compose.prod.yml").is_file() && directory.join(PROD_FILES).is_file()
}

/// The deploy folder: the current folder or one above it, else ~/misty-cloud (the VPS's), else
/// the misty-cloud checkout beside misty. None needs git.
fn deploy_folder(workspace: &Workspace) -> Result<PathBuf> {
    let here = std::env::current_dir().ok();
    let found = here
        .iter()
        .flat_map(|directory| directory.ancestors())
        .find(|directory| is_deploy_folder(directory))
        .map(Path::to_path_buf);
    let home = dirs::home_dir().map(|home| home.join("misty-cloud"));
    found
        .into_iter()
        .chain(home)
        .chain([workspace.cloud.clone()])
        .find(|directory| is_deploy_folder(directory))
        .with_context(|| {
            "This needs misty-cloud's deploy folder (compose.prod.yml and deploy/prod-files.txt): \
             run it there, or `misty cloud ship` one to ~/misty-cloud"
        })
}

/// docker compose for the stack. Both pins go to the CLI, not to an include's env_file:
/// compose.prod.yml interpolates them itself, and the included server file inherits them.
fn prod_compose(arguments: &[&str]) -> CommandSpec {
    CommandSpec::new("docker")
        .env("COMPOSE_DISABLE_ENV_FILE", "1")
        .env_remove("COMPOSE_ENV_FILES")
        .args([
            "compose",
            "--env-file",
            SERVER_VERSION,
            "--env-file",
            BILLING_VERSION,
        ])
        .args(["--file", "compose.prod.yml"])
        .args(arguments)
}

fn compose(root: &Path, arguments: &[&str]) -> Result<()> {
    prod_compose(arguments).run(root)
}

fn up(root: &Path) -> Result<()> {
    check(root)?;
    let plain = CommandSpec::new("docker")
        .args([
            "ps",
            "-q",
            "--filter",
            "label=com.docker.compose.project=misty",
        ])
        .capture(root)?;
    if !plain.trim().is_empty() {
        bail!("Stop the plain misty-server deploy first: docker compose -p misty down (no --volumes).");
    }
    compose(root, &["pull"])?;
    // The server's compose.yml keeps its build: sections; the VPS never builds.
    compose(root, &["up", "--detach", "--no-build", "--remove-orphans"])?;
    println!("Started. Billing answers at http://127.0.0.1:8091/health once it is up.");
    Ok(())
}

/// KEY=value pairs of an env file, the last one of each name winning.
fn read_env(path: &Path) -> Result<Vec<(String, String)>> {
    dotenvy::from_path_iter(path)
        .with_context(|| format!("could not read {}", path.display()))?
        .collect::<Result<_, _>>()
        .with_context(|| format!("could not read {}", path.display()))
}

fn env_value(values: &[(String, String)], name: &str) -> String {
    values
        .iter()
        .rev()
        .find(|(key, _)| key == name)
        .map(|(_, value)| value.clone())
        .unwrap_or_default()
}

pub(crate) fn lowercase_hex(value: &str, minimum: usize) -> bool {
    value.len() >= minimum
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

fn valid_commit(commit: &str) -> bool {
    commit.len() == 40 && lowercase_hex(commit, 40)
}

/// `<repository>:<tag>@sha256:<64 hex>`, or `<repository>@sha256:<64 hex>` with no tag.
fn pinned(reference: &str, repository: &str, tag: Option<&str>) -> bool {
    let prefix = match tag {
        Some(tag) => format!("{repository}:{tag}@sha256:"),
        None => format!("{repository}@sha256:"),
    };
    reference
        .strip_prefix(&prefix)
        .is_some_and(|digest| digest.len() == 64 && lowercase_hex(digest, 64))
}

/// Problems with the server pin, or none.
fn server_pin_problems(values: &[(String, String)]) -> Vec<String> {
    let commit = env_value(values, "MISTY_SERVER_COMMIT");
    if !valid_commit(&commit) {
        return vec![format!(
            "MISTY_SERVER_COMMIT in {SERVER_VERSION} must be a 40-character commit: misty cloud pin-server <commit>"
        )];
    }
    let mut problems = Vec::new();
    for (name, service) in [("MISTY_API_IMAGE", "api"), ("MISTY_TOOLS_IMAGE", "tools")] {
        if !pinned(
            &env_value(values, name),
            &format!("{SERVER_IMAGE}-{service}"),
            Some(&commit),
        ) {
            problems.push(format!(
                "{name} in {SERVER_VERSION} must be {SERVER_IMAGE}-{service}:{commit}@sha256:…"
            ));
        }
    }
    problems
}

fn billing_pin_problem(values: &[(String, String)]) -> Option<String> {
    (!pinned(&env_value(values, "MISTY_BILLING_IMAGE"), IMAGE, None)).then(|| {
        format!("MISTY_BILLING_IMAGE in {BILLING_VERSION} must be {IMAGE}@sha256:…: misty cloud release <version>")
    })
}

/// The pins, the settings files, the server's volumes and the Compose file.
fn check(root: &Path) -> Result<()> {
    let mut problems = server_pin_problems(&read_env(&root.join(SERVER_VERSION))?);
    problems.extend(billing_pin_problem(&read_env(&root.join(BILLING_VERSION))?));
    if !problems.is_empty() {
        bail!(problems.join("\n"));
    }

    let server_env = match std::env::var_os("MISTY_ENV_DIR") {
        Some(directory) => PathBuf::from(directory),
        None => dirs::home_dir()
            .context("no home folder")?
            .join(".misty/.env/server"),
    };
    for name in ["misty.env", "cloudflare.env", "google.env"] {
        private(&server_env.join(name), true)?;
    }
    if !private(&server_env.join("mailjet.env"), false)? {
        eprintln!(
            "warning: no {}: the server sends no email.",
            server_env.join("mailjet.env").display()
        );
    }

    if private(&root.join(STRIPE_ENV), false)? {
        let stripe = read_env(&root.join(STRIPE_ENV))?;
        if env_value(&stripe, "STRIPE_SECRET_KEY").is_empty() {
            eprintln!("warning: {STRIPE_ENV} has no STRIPE_SECRET_KEY: nothing is sold.");
        }
        for name in RETIRED_STRIPE {
            if stripe.iter().any(|(key, _)| key == name) {
                eprintln!("warning: {name} in {STRIPE_ENV} isn't read any more; delete it.");
            }
        }
    } else {
        eprintln!("warning: no {STRIPE_ENV}: nothing is sold.");
    }
    for file in RETIRED_FILES {
        if root.join(file).exists() {
            eprintln!("warning: {file} isn't read any more (billing-setup makes what it held); delete it.");
        }
    }

    if !CommandSpec::new("docker")
        .args(["volume", "inspect", "misty_pgdata", "misty_secrets"])
        .succeeds(root)
    {
        bail!(
            "The server's volumes misty_pgdata and misty_secrets are missing. On a new server, \
             create them once: docker volume create misty_pgdata && docker volume create misty_secrets. \
             Otherwise check the plain misty-server deploy's volume names."
        );
    }
    prod_compose(&["config", "--quiet"]).run(root)?;
    let images = prod_compose(&["config", "--images"]).capture(root)?;
    let loose: Vec<&str> = images
        .lines()
        .map(str::trim)
        .filter(|image| !image.is_empty() && !image.contains("@sha256:"))
        .collect();
    if !loose.is_empty() {
        bail!(
            "Every image must be pinned by digest; these aren't: {}",
            loose.join(", ")
        );
    }
    println!("Production configuration is valid.");
    Ok(())
}

/// Whether path exists; it must be mode 0600 when it does, and exist when required.
fn private(path: &Path, required: bool) -> Result<bool> {
    let Ok(metadata) = fs::metadata(path) else {
        if required {
            bail!("Missing {}.", path.display());
        }
        return Ok(false);
    };
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if metadata.permissions().mode() & 0o777 != 0o600 {
            bail!("{0} must be mode 0600 (chmod 600 {0}).", path.display());
        }
    }
    let _ = metadata;
    Ok(true)
}

/// Replaces the KEY= lines of an env file in place, keeping its comments and other lines; a key
/// the file lacks is added at the end.
fn set_env_lines(text: &str, values: &[(&str, &str)]) -> String {
    let mut lines: Vec<String> = Vec::new();
    let mut done = vec![false; values.len()];
    for line in text.lines() {
        match values
            .iter()
            .position(|(key, _)| line.starts_with(&format!("{key}=")))
        {
            Some(index) if !done[index] => {
                lines.push(format!("{}={}", values[index].0, values[index].1));
                done[index] = true;
            }
            Some(_) => {}
            None => lines.push(line.to_string()),
        }
    }
    for (index, (key, value)) in values.iter().enumerate() {
        if !done[index] {
            lines.push(format!("{key}={value}"));
        }
    }
    lines.join("\n") + "\n"
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
/// checks, builds the linux/amd64 image, pushes it, tags the commit and pins the digest in
/// deploy/billing/version.env, which is then committed.
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
    let pin = root.join(BILLING_VERSION);
    if !pin.is_file() {
        bail!("{BILLING_VERSION} is missing from this checkout.");
    }

    for suite in ["secrets", "go"] {
        Repo::Cloud.run(root, suite)?;
    }

    let sha = git(&["rev-parse", "HEAD"])?.trim().to_string();
    let metadata = tempfile::NamedTempFile::new()?;
    CommandSpec::new("docker")
        .args([
            "buildx",
            "build",
            "--platform",
            "linux/amd64",
            "--provenance=false",
            "--push",
        ])
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

    git(&[
        "tag",
        "-a",
        &tag,
        "-m",
        &format!("Misty billing {version}"),
        &sha,
    ])?;
    // The commit was already checked above.
    git(&["push", "--quiet", "--no-verify", "origin", &tag])?;

    let reference = format!("{IMAGE}@{digest}");
    let text = fs::read_to_string(&pin)?;
    fs::write(
        &pin,
        set_env_lines(&text, &[("MISTY_BILLING_IMAGE", &reference)]),
    )?;
    println!("Released {tag}: MISTY_BILLING_IMAGE={reference}");
    println!("Commit {BILLING_VERSION} (e.g. chore(deploy): run billing {tag}), push, then misty cloud ship.");
    Ok(())
}

/// Fetches misty-server's compose.yml at commit: from GitHub's raw files, or through gh.
fn server_compose_at(root: &Path, commit: &str) -> Result<String> {
    let url =
        format!("https://raw.githubusercontent.com/misty-org/misty-server/{commit}/compose.yml");
    if let Ok(text) = CommandSpec::new("curl").args(["-fsSL", &url]).capture(root) {
        return Ok(text);
    }
    CommandSpec::new("gh")
        .args(["api", "-H", "Accept: application/vnd.github.raw"])
        .arg(format!(
            "repos/misty-org/misty-server/contents/compose.yml?ref={commit}"
        ))
        .capture(root)
        .with_context(|| format!("could not fetch misty-server's compose.yml at {commit}"))
}

/// The digest of a published image's manifest list.
fn published_digest(root: &Path, reference: &str) -> Result<String> {
    let manifest = CommandSpec::new("docker")
        .args([
            "buildx",
            "imagetools",
            "inspect",
            reference,
            "--format",
            "{{json .Manifest}}",
        ])
        .capture(root)
        .with_context(|| format!("{reference} isn't published"))?;
    let manifest: serde_json::Value = serde_json::from_str(manifest.trim())?;
    let digest = manifest["digest"].as_str().unwrap_or_default();
    match digest.strip_prefix("sha256:") {
        Some(hex) if hex.len() == 64 && lowercase_hex(hex, 64) => Ok(digest.to_string()),
        _ => bail!("{reference} has no sha256 digest"),
    }
}

fn pin_server(root: &Path, commit: &str) -> Result<()> {
    if !valid_commit(commit) {
        bail!("Name the misty-server commit by its 40 lowercase hex characters.");
    }
    let mut images = Vec::new();
    for service in ["api", "tools"] {
        let tag = format!("{SERVER_IMAGE}-{service}:{commit}");
        let digest = published_digest(root, &tag)?;
        images.push(format!("{tag}@{digest}"));
    }
    let compose = server_compose_at(root, commit)?;
    fs::write(root.join(SERVER_COMPOSE), compose)?;
    let pin = root.join(SERVER_VERSION);
    let text = fs::read_to_string(&pin)?;
    fs::write(
        &pin,
        set_env_lines(
            &text,
            &[
                ("MISTY_SERVER_COMMIT", commit),
                ("MISTY_API_IMAGE", &images[0]),
                ("MISTY_TOOLS_IMAGE", &images[1]),
            ],
        ),
    )?;
    println!("Pinned misty-server {commit}.");
    println!(
        "Commit {SERVER_VERSION} and {SERVER_COMPOSE} (e.g. chore(deploy): run misty-server {}).",
        &commit[..8]
    );
    Ok(())
}

/// The files deploy/prod-files.txt lists, without blank lines and # comments.
fn prod_files(text: &str) -> Vec<String> {
    text.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .map(str::to_string)
        .collect()
}

/// A host or folder name safe to put in an ssh command line unquoted.
fn plain_name(value: &str) -> bool {
    !value.is_empty()
        && !value.starts_with('-')
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"._-/@".contains(&b))
}

fn ship(root: &Path, host: &str, path: &str, up: bool) -> Result<()> {
    if !plain_name(host) || !plain_name(path) {
        bail!("--host and --path take plain names, such as misty-vps and misty-cloud.");
    }
    let files = prod_files(&fs::read_to_string(root.join(PROD_FILES))?);
    for file in &files {
        if !root.join(file).is_file() {
            bail!("{PROD_FILES} lists {file}, which doesn't exist.");
        }
    }
    let changed = CommandSpec::new("git")
        .args(["status", "--porcelain", "--"])
        .args(&files)
        .capture(root)?;
    if !changed.trim().is_empty() {
        bail!(
            "Commit these first, so the VPS runs only committed files:\n{}",
            changed.trim_end()
        );
    }
    let mut problems = server_pin_problems(&read_env(&root.join(SERVER_VERSION))?);
    problems.extend(billing_pin_problem(&read_env(&root.join(BILLING_VERSION))?));
    if !problems.is_empty() {
        bail!(problems.join("\n"));
    }

    // tar here into ssh there; .env/ is never listed, so settings never leave the VPS.
    let mut tar = CommandSpec::new("tar").env("COPYFILE_DISABLE", "1");
    if cfg!(target_os = "macos") {
        tar = tar.arg("--no-xattrs");
    }
    let mut tar = tar
        .args(["-c", "-f", "-"])
        .args(&files)
        .command(root)
        .stdout(Stdio::piped())
        .spawn()
        .context("could not start tar")?;
    let archive = tar.stdout.take().context("tar has no output")?;
    let status = CommandSpec::new("ssh")
        .arg(host)
        .arg(format!("mkdir -p {path} && tar -x -f - -C {path}"))
        .command(root)
        .stdin(archive)
        .status()
        .context("could not start ssh")?;
    let packed = tar.wait()?;
    if !packed.success() || !status.success() {
        bail!("Copying to {host}:{path} failed.");
    }
    println!("Shipped {} files to {host}:~/{path}.", files.len());
    if up {
        CommandSpec::new("ssh")
            .args(["-t", host])
            .arg(format!("cd {path} && misty cloud prod up"))
            .run(root)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn release_versions_are_validated() {
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

    fn pairs(values: &[(&str, &str)]) -> Vec<(String, String)> {
        values
            .iter()
            .map(|(key, value)| (key.to_string(), value.to_string()))
            .collect()
    }

    #[test]
    fn pins_name_a_commit_and_digests() {
        let commit = "0a9c9318".repeat(5);
        let digest = format!("sha256:{}", "e".repeat(64));
        let api = format!("{SERVER_IMAGE}-api:{commit}@{digest}");
        let tools = format!("{SERVER_IMAGE}-tools:{commit}@{digest}");
        let good = pairs(&[
            ("MISTY_SERVER_COMMIT", &commit),
            ("MISTY_API_IMAGE", &api),
            ("MISTY_TOOLS_IMAGE", &tools),
        ]);
        assert!(server_pin_problems(&good).is_empty());
        let empty = pairs(&[
            ("MISTY_SERVER_COMMIT", ""),
            ("MISTY_API_IMAGE", ""),
            ("MISTY_TOOLS_IMAGE", ""),
        ]);
        assert_eq!(server_pin_problems(&empty).len(), 1);
        let other = format!("{SERVER_IMAGE}-api:{}@{digest}", "1".repeat(40));
        let mismatched = pairs(&[
            ("MISTY_SERVER_COMMIT", &commit),
            ("MISTY_API_IMAGE", &other),
            ("MISTY_TOOLS_IMAGE", &tools),
        ]);
        assert_eq!(server_pin_problems(&mismatched).len(), 1);

        assert!(billing_pin_problem(&pairs(&[(
            "MISTY_BILLING_IMAGE",
            &format!("{IMAGE}@{digest}")
        )]))
        .is_none());
        assert!(billing_pin_problem(&pairs(&[("MISTY_BILLING_IMAGE", "")])).is_some());
        assert!(billing_pin_problem(&pairs(&[(
            "MISTY_BILLING_IMAGE",
            &format!("{IMAGE}:0.1.0")
        )]))
        .is_some());
    }

    #[test]
    fn pin_files_keep_their_comments() {
        let text =
            "# The pin.\nMISTY_SERVER_COMMIT=\n# images\nMISTY_API_IMAGE=old\nMISTY_TOOLS_IMAGE=\n";
        let pinned = set_env_lines(
            text,
            &[
                ("MISTY_SERVER_COMMIT", "abc"),
                ("MISTY_API_IMAGE", "api"),
                ("MISTY_TOOLS_IMAGE", "tools"),
            ],
        );
        assert_eq!(pinned, "# The pin.\nMISTY_SERVER_COMMIT=abc\n# images\nMISTY_API_IMAGE=api\nMISTY_TOOLS_IMAGE=tools\n");
        assert_eq!(
            set_env_lines("# c\n", &[("MISTY_BILLING_IMAGE", "x")]),
            "# c\nMISTY_BILLING_IMAGE=x\n"
        );
    }

    #[test]
    fn shipping_reads_the_file_list_and_plain_names() {
        let files = prod_files("# comment\ncompose.prod.yml\n\n  deploy/billing/version.env\n");
        assert_eq!(files, ["compose.prod.yml", "deploy/billing/version.env"]);
        assert!(
            plain_name("misty-vps")
                && plain_name("misty-cloud")
                && plain_name("deploy/misty-cloud")
        );
        for bad in ["", "-oProxyCommand=x", "a b", "a;b", "$(x)"] {
            assert!(!plain_name(bad), "{bad}");
        }
    }
}
