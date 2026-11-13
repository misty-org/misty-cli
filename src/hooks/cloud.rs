//! misty-cloud's checks: vet, race tests and a build, with the database suites on disposable
//! PostgreSQL instead of being skipped. A commit runs them when its staged changes touch Go or
//! deployment; a push runs everything on a clean tree.

use std::path::Path;

use anyhow::{bail, Result};

use super::{
    postgres::{Disposable, Options},
    runner,
    secrets::{self, Scan},
};
use crate::process::CommandSpec;

pub const ALL: &[&str] = &["secrets", "go"];

const SCAN: Scan = Scan {
    config: None,
    unpushed_without_upstream: false,
};

pub fn suite(root: &Path, name: &str) -> Result<()> {
    match name {
        "go" => go(root),
        "secrets" => secrets::commits(root, &SCAN),
        "staged-secrets" => secrets::staged(root, &SCAN),
        _ => bail!("Unknown suite {name}; misty-cloud has go, secrets and all"),
    }
}

fn go(root: &Path) -> Result<()> {
    let password = "isolated-test-password";
    let database = Disposable::start(Options {
        label: "cloud-tests",
        user: "billing_test",
        password,
        database: "misty_go_billing_test",
        memory: "1g",
        settings: &["fsync=off", "synchronous_commit=off", "full_page_writes=off"],
    })?;
    database.exec(&["createdb", "-U", "billing_test", "misty_payments_test"])?;
    let dsn = format!(
        "host=127.0.0.1 port={} user=billing_test password={password} sslmode=disable",
        database.port
    );
    let build = std::env::temp_dir().join(format!("misty-cloud-check-{}", std::process::id()));
    let result = runner::commands(
        root,
        [
            CommandSpec::new("go").args(["vet", "./..."]),
            CommandSpec::new("go")
                .args(["test", "-race", "-count=1", "./..."])
                .env(
                    "MISTY_GO_BILLING_TEST_DSN",
                    format!("{dsn} dbname=misty_go_billing_test"),
                )
                .env(
                    "MISTY_BILLING_PAYMENTS_TEST_DSN",
                    format!("{dsn} dbname=misty_payments_test"),
                ),
            CommandSpec::new("go")
                .args(["build", "-o"])
                .arg(build.as_os_str())
                .arg("./cmd/misty-cloud"),
        ],
    );
    let _ = std::fs::remove_file(&build);
    // The open server's account API is exercised against a fake in internal/payments; the
    // server's own suite tests the real one.
    result
}

pub fn suites_for<'a>(paths: impl IntoIterator<Item = &'a str>) -> Vec<&'static str> {
    let touches = |path: &str| {
        path.ends_with(".go")
            || matches!(path, "go.mod" | "go.sum")
            || ["internal/", "cmd/", "deploy/"]
                .iter()
                .any(|folder| path.starts_with(folder))
    };
    if paths.into_iter().any(touches) {
        vec!["go"]
    } else {
        Vec::new()
    }
}

pub fn pre_commit(root: &Path) -> Result<()> {
    super::require_staged_snapshot(root)?;
    runner::run("staged-secrets", || suite(root, "staged-secrets"))?;
    let staged = super::staged_paths(root)?;
    if suites_for(staged.lines()).is_empty() {
        println!("No test suites affected.");
        return Ok(());
    }
    runner::run("go", || suite(root, "go"))
}

pub fn pre_push(root: &Path) -> Result<()> {
    super::require_clean_tree(root, true)?;
    for name in ALL {
        runner::run(name, || suite(root, name))?;
    }
    Ok(())
}
