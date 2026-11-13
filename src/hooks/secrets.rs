//! Secret scanning with a pinned gitleaks image. It scans commits or the staged snapshot, never
//! the working folder, so git-ignored .env files stay out.

use std::path::{Path, PathBuf};

use anyhow::Result;

use super::git;
use crate::process::CommandSpec;

const IMAGE: &str =
    "zricethezav/gitleaks@sha256:e1b35e12a8c6fa8901f060459cfb6b2fc4c484d3afbe3b029733a3bbfab07055";

/// How one repository scans.
pub struct Scan {
    /// The gitleaks configuration, relative to the repository root.
    pub config: Option<&'static str>,
    /// With no upstream, scan only commits no remote branch has (otherwise all of HEAD).
    pub unpushed_without_upstream: bool,
}

/// The changes a commit would make.
pub fn staged(root: &Path, scan: &Scan) -> Result<()> {
    gitleaks(root, scan, &["protect".into(), "--staged".into()])
}

/// The commits the remote does not have yet.
pub fn commits(root: &Path, scan: &Scan) -> Result<()> {
    let upstream = git(
        root,
        &["rev-parse", "--abbrev-ref", "--symbolic-full-name", "@{upstream}"],
    )
    .ok()
    .map(|name| name.trim().to_string())
    .filter(|name| !name.is_empty());
    let range: Vec<String> = match upstream {
        Some(upstream) => vec![format!("{upstream}..HEAD")],
        None if scan.unpushed_without_upstream => {
            vec!["HEAD".into(), "--not".into(), "--remotes".into()]
        }
        None => vec!["HEAD".into()],
    };
    let mut list = vec!["rev-list"];
    list.extend(range.iter().map(String::as_str));
    if git(root, &list).unwrap_or_default().trim().is_empty() {
        println!("No new commits to scan.");
        return Ok(());
    }
    gitleaks(
        root,
        scan,
        &["detect".into(), format!("--log-opts={}", range.join(" "))],
    )
}

fn gitleaks(root: &Path, scan: &Scan, arguments: &[String]) -> Result<()> {
    let (repo, mounts) = mounts(root)?;
    let mut command = CommandSpec::new("docker")
        .args(["run", "--rm"])
        .args(mounts)
        .args(["-w", repo.as_str()])
        .args(["-e", "GIT_CONFIG_COUNT=1", "-e", "GIT_CONFIG_KEY_0=safe.directory"])
        .args(["-e".to_string(), format!("GIT_CONFIG_VALUE_0={repo}")])
        .arg(IMAGE)
        .args(arguments)
        .arg(format!("--source={repo}"))
        .arg("--redact");
    if let Some(config) = scan.config {
        command = command.arg(format!("--config={repo}/{config}"));
    }
    super::runner::commands(root, [command])
}

/// A linked worktree and the main repository's git directory point at each other by absolute
/// path, so a worktree is mounted at its own path; a plain checkout is mounted at /repo.
fn mounts(root: &Path) -> Result<(String, Vec<String>)> {
    let common = git(root, &["rev-parse", "--git-common-dir"])?;
    let common = root
        .join(common.trim())
        .canonicalize()
        .unwrap_or_else(|_| PathBuf::from(common.trim()));
    let own = root.join(".git").canonicalize().ok();
    let root_text = root.display().to_string();
    if own.as_ref() == Some(&common) {
        return Ok((
            "/repo".into(),
            vec!["-v".into(), format!("{root_text}:/repo:ro")],
        ));
    }
    let common = common.display().to_string();
    Ok((
        root_text.clone(),
        vec![
            "-v".into(),
            format!("{root_text}:{root_text}:ro"),
            "-v".into(),
            format!("{common}:{common}:ro"),
        ],
    ))
}
