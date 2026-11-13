//! Runs the Go server suite against a disposable PostgreSQL container, exactly as production runs
//! it (same pinned image); the dev database is untouched.
//!
//! Arguments replace the default `./...` package list, e.g.
//! `misty server test --isolated ./test/contract/postgres -run NativeNote`. With none the suite
//! runs in parallel: every job gets its own copy of the migrated database, and the slowest
//! package's tests are split across jobs.

use std::{
    path::Path,
    process::{Output, Stdio},
    thread,
};

use anyhow::{Context, Result};

use super::{
    postgres::{Disposable, Options},
    runner::Failed,
};
use crate::process::CommandSpec;

const PASSWORD: &str = "misty-test-password";
const GOOSE_TAGS: &str = "no_clickhouse no_libsql no_mssql no_mysql no_sqlite3 no_vertica no_ydb";
/// One package holds most of the database tests (about a minute alone).
const HEAVY: &str = "./test/contract/postgres";

pub fn run(root: &Path, arguments: &[String]) -> Result<()> {
    let database = Disposable::start(Options {
        label: "server-tests",
        user: "misty",
        password: PASSWORD,
        database: "misty_test",
        memory: "2g",
        settings: &[
            "checkpoint_timeout=30s",
            "fsync=off",
            "synchronous_commit=off",
            "full_page_writes=off",
        ],
    })?;
    let port = database.port.to_string();
    for (name, value) in [
        ("DB_HOST", "127.0.0.1"),
        ("DB_PORT", port.as_str()),
        ("DB_USER", "misty"),
        ("DB_PASSWORD", PASSWORD),
        ("DB_NAME", "misty_test"),
        ("DB_SSLMODE", "disable"),
        ("TEST_DB_HOST", "127.0.0.1"),
        ("TEST_DB_PORT", port.as_str()),
        ("TEST_DB_USER", "misty"),
        ("TEST_DB_PASSWORD", PASSWORD),
        ("TEST_DB_NAME", "misty_test"),
        ("TEST_DB_SSLMODE", "disable"),
        ("MISTY_ENVIRONMENT", ""),
    ] {
        std::env::set_var(name, value);
    }
    migrate(root, &port)?;

    if !arguments.is_empty() {
        let output = go_test(root, "misty_test", arguments)?;
        print_quietly(&output);
        return finish(output.status.code());
    }

    let heavy_shards = count("MISTY_GO_HEAVY_SHARDS", 4);
    let other_jobs = count("MISTY_GO_OTHER_JOBS", 2);
    let go = |arguments: &[&str]| CommandSpec::new("go").args(arguments).capture(root);
    let heavy_tests: Vec<String> = go(&["test", "-list", "^Test", HEAVY])?
        .lines()
        .filter(|line| line.starts_with("Test"))
        .map(str::to_string)
        .collect();
    let heavy_package = go(&["list", HEAVY])?.trim().to_string();
    let others: Vec<String> = go(&["list", "./..."])?
        .lines()
        .filter(|package| *package != heavy_package)
        .map(str::to_string)
        .collect();

    // Tests reset their database between cases, so jobs never share one. A clone of the
    // migrated database is a file copy: nothing is connected to the template.
    let mut jobs: Vec<(String, Vec<String>)> = Vec::new();
    for shard in 0..heavy_shards {
        let names: Vec<&str> = heavy_tests
            .iter()
            .skip(shard)
            .step_by(heavy_shards)
            .map(String::as_str)
            .collect();
        if !names.is_empty() {
            jobs.push((
                format!("misty_test_{}", shard + 1),
                vec![HEAVY.into(), "-run".into(), format!("^({})$", names.join("|"))],
            ));
        }
    }
    for slot in 0..other_jobs {
        let packages: Vec<String> = others.iter().skip(slot).step_by(other_jobs).cloned().collect();
        if !packages.is_empty() {
            jobs.push((format!("misty_test_{}", heavy_shards + slot + 1), packages));
        }
    }
    for (name, _) in &jobs {
        database.exec(&[
            "psql", "-q", "-U", "misty", "-d", "postgres", "-v", "ON_ERROR_STOP=1", "-c",
            &format!("CREATE DATABASE {name} TEMPLATE misty_test"),
        ])?;
    }
    let handles: Vec<_> = jobs
        .into_iter()
        .map(|(name, arguments)| {
            let root = root.to_path_buf();
            thread::spawn(move || go_test(&root, &name, &arguments))
        })
        .collect();
    let mut failed = false;
    for handle in handles {
        match handle.join() {
            Ok(Ok(output)) => {
                failed |= !output.status.success();
                print_quietly(&output);
            }
            Ok(Err(error)) => {
                failed = true;
                eprintln!("{error:#}");
            }
            Err(_) => failed = true,
        }
    }
    drop(database);
    finish(Some(if failed { 1 } else { 0 }))
}

/// Applies the migrations with a goose kept in .misty/bin, then gives the API's restricted role
/// what production gives it.
fn migrate(root: &Path, port: &str) -> Result<()> {
    let bin = root.join(".misty/bin");
    let goose = bin.join("goose");
    if !goose.is_file() {
        CommandSpec::new("go")
            .arg("install")
            .arg(format!("-tags={GOOSE_TAGS}"))
            .arg("github.com/pressly/goose/v3/cmd/goose@v3.27.3")
            .env("GOBIN", bin.as_os_str())
            .run(root)?;
    }
    let dsn = format!(
        "host=127.0.0.1 port={port} user=misty password={PASSWORD} dbname=misty_test sslmode=disable"
    );
    CommandSpec::new(goose.as_os_str())
        .args(["-dir", "internal/platform/db/migrations", "postgres", dsn.as_str(), "up"])
        .capture(root)
        .context("could not apply the migrations")?;
    CommandSpec::new("go")
        .args(["run", "./cmd/misty-dev-setup", "grant-app-role"])
        .env("PGHOST", "127.0.0.1")
        .env("PGPORT", port)
        .env("PGUSER", "misty")
        .env("PGPASSWORD", PASSWORD)
        .env("PGDATABASE", "misty_test")
        .env("MISTY_APP_DB_USER", "misty_app")
        .env("MISTY_APP_DB_PASSWORD", "misty-app-test-password")
        .capture(root)
        .context("could not grant the application role")?;
    Ok(())
}

fn go_test(root: &Path, database: &str, arguments: &[String]) -> Result<Output> {
    CommandSpec::new("go")
        .args(["test", "-p", "1", "-timeout", "30m", "-count=1"])
        .args(arguments)
        .env("DB_NAME", database)
        .env("TEST_DB_NAME", database)
        .command(root)
        .stdin(Stdio::null())
        .output()
        .context("could not start go test")
}

/// Shows everything but the passing-package and no-test-files lines.
fn print_quietly(output: &Output) {
    for bytes in [&output.stdout, &output.stderr] {
        for line in String::from_utf8_lossy(bytes).lines() {
            if !line.starts_with("ok ") && !line.contains("no test files") {
                println!("{line}");
            }
        }
    }
}

fn count(name: &str, default: usize) -> usize {
    std::env::var(name)
        .ok()
        .and_then(|value| value.trim().parse().ok())
        .filter(|value| *value > 0)
        .unwrap_or(default)
}

fn finish(code: Option<i32>) -> Result<()> {
    match code {
        Some(0) => Ok(()),
        code => Err(Failed(code.unwrap_or(1)).into()),
    }
}
