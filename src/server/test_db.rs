//! `misty server test`: the Go suite against a test database. With no database configured (or a
//! local one), it bootstraps a fresh test database in the development stack's PostgreSQL and
//! applies the migrations to it; database-backed packages then run one at a time on it.

use std::{env, fs, path::Path};

use anyhow::{bail, Context, Result};

use crate::process::{command_exists, CommandSpec};

pub fn test(server: &Path, arguments: &[String]) -> Result<()> {
    let compose = development_compose();
    let explicit_host = var("TEST_DB_HOST");
    let explicit_user = var("TEST_DB_USER");
    let explicit_sslmode = var("TEST_DB_SSLMODE");

    set_default("TEST_DB_HOST", var("DB_HOST").unwrap_or_default());
    set_default("TEST_DB_PORT", var("DB_PORT").unwrap_or_else(|| "5432".into()));
    set_default("TEST_DB_USER", var("DB_USER").unwrap_or_default());
    set_default("TEST_DB_PASSWORD", var("DB_PASSWORD").unwrap_or_default());
    set_default("TEST_DB_SSLMODE", var("DB_SSLMODE").unwrap_or_else(|| "disable".into()));
    if var("TEST_DB_NAME").is_none() {
        if let Some(name) = var("DB_NAME") {
            let test_name = if name.contains("test") { name } else { format!("{name}_test") };
            env::set_var("TEST_DB_NAME", test_name);
        }
    }

    let bootstrap = var("BOOTSTRAP_TEST_DB").unwrap_or_else(|| "auto".into());
    let local = matches!(
        var("TEST_DB_HOST").as_deref(),
        None | Some("localhost" | "127.0.0.1" | "::1")
    );
    if bootstrap == "1" || bootstrap == "true" || (bootstrap == "auto" && explicit_host.is_none() && local) {
        bootstrap_database(server, &compose, explicit_user.is_some(), explicit_sslmode.is_some())?;
    }

    // Database-backed packages share this one disposable database. Keep package execution
    // serial so one package cannot truncate or reseed beneath another.
    let code = CommandSpec::new("go")
        .args(["test", "-p", "1", "./...", "-count=1"])
        .args(arguments)
        .exit_code(server)?;
    if code != 0 {
        return Err(crate::hooks::Failed(code).into());
    }
    Ok(())
}

fn bootstrap_database(
    server: &Path,
    compose: &CommandSpec,
    explicit_user: bool,
    explicit_sslmode: bool,
) -> Result<()> {
    if !command_exists("docker") {
        bail!("TEST_DB_HOST is not set and Docker is not available.\nSet TEST_DB_* or DB_* for an existing Postgres instance, then rerun misty server test.");
    }
    // The development Postgres port is intentionally IPv4 loopback-only. Avoid macOS resolving
    // localhost to ::1 for the bootstrapped container.
    let host = var("TEST_DB_HOST").unwrap_or_else(|| "localhost".into());
    let host = if matches!(host.as_str(), "localhost" | "::1") { "127.0.0.1".into() } else { host };
    env::set_var("TEST_DB_HOST", host);
    set_default("TEST_DB_PORT", "5432".into());
    set_default("TEST_DB_USER", "misty".into());
    set_default("TEST_DB_PASSWORD", "misty".into());
    // DB_USER is the application role and is deliberately unprivileged, so it can neither
    // recreate the test database nor TRUNCATE between tests. Bootstrap and run as the same
    // migration role Compose uses, including its development default.
    if !explicit_user {
        env::set_var("TEST_DB_USER", var("DB_MIGRATION_USER").unwrap_or_else(|| "misty".into()));
        env::set_var(
            "TEST_DB_PASSWORD",
            var("DB_MIGRATION_PASSWORD").unwrap_or_else(|| "misty".into()),
        );
    }
    let admin = var("TEST_DB_USER").unwrap_or_default();
    // The bootstrapped container has no TLS, so a production DB_SSLMODE inherited from
    // database.env would fail every connection.
    if !explicit_sslmode {
        env::set_var("TEST_DB_SSLMODE", "disable");
    }
    set_default("TEST_DB_NAME", "misty_server_test".into());
    set_default("TEST_DB_SSLMODE", "disable".into());
    set_default("DB_USER", admin.clone());
    set_default("DB_PASSWORD", var("TEST_DB_PASSWORD").unwrap_or_default());
    set_default("DB_NAME", "misty_server".into());
    set_default("DB_PORT", var("TEST_DB_PORT").unwrap_or_default());
    let name = var("TEST_DB_NAME").unwrap_or_default();
    let maintenance = var("DB_NAME").unwrap_or_default();

    compose.clone().args(["up", "-d", "postgres"]).run(server)?;
    while !compose
        .clone()
        .args(["exec", "-T", "postgres", "pg_isready", "-U", admin.as_str(), "-d", maintenance.as_str()])
        .succeeds(server)
    {
        std::thread::sleep(std::time::Duration::from_secs(1));
    }
    if !name.to_lowercase().contains("test") {
        bail!("Refusing to recreate non-test database: {name}");
    }
    if !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
        bail!("Refusing test database name with unsupported characters: {name}");
    }
    let psql = |database: &str| {
        compose.clone().args([
            "exec", "-T", "postgres", "psql", "-v", "ON_ERROR_STOP=1", "-U", admin.as_str(), "-d", database,
        ])
    };
    psql("postgres").run_with_input(
        server,
        format!(
            "SELECT pg_terminate_backend(pid) FROM pg_stat_activity WHERE datname = '{name}' AND pid <> pg_backend_pid();\nDROP DATABASE IF EXISTS \"{name}\";\nCREATE DATABASE \"{name}\";\n"
        )
        .as_bytes(),
    )?;

    let mut migrations: Vec<_> = fs::read_dir(server.join("internal/platform/db/migrations"))?
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter(|path| path.extension().is_some_and(|ext| ext == "sql"))
        .collect();
    migrations.sort();
    let mut versions = String::from(
        "CREATE TABLE IF NOT EXISTS goose_db_version (id SERIAL PRIMARY KEY, version_id BIGINT NOT NULL, is_applied BOOLEAN NOT NULL, tstamp TIMESTAMP NULL DEFAULT now());\nTRUNCATE goose_db_version RESTART IDENTITY;\nINSERT INTO goose_db_version (version_id, is_applied) VALUES (0, true);\n",
    );
    for migration in &migrations {
        let text = fs::read_to_string(migration)
            .with_context(|| format!("could not read {}", migration.display()))?;
        psql(&name)
            .arg("--single-transaction")
            .run_with_input(server, up_section(&text).as_bytes())?;
        let file = migration.file_name().unwrap_or_default().to_string_lossy();
        let version = file.split('_').next().unwrap_or_default();
        versions.push_str(&format!(
            "INSERT INTO goose_db_version (version_id, is_applied) VALUES ({version}, true);\n"
        ));
    }
    // Migrations are applied with psql rather than goose, so record them in the version table
    // goose would have written. checkSchemaVersion reads it, and without this a bootstrapped
    // database looks unmigrated.
    psql(&name).run_with_input(server, versions.as_bytes())
}

/// The statements between `-- +goose Up` and `-- +goose Down`, without goose's own annotations.
pub(super) fn up_section(migration: &str) -> String {
    let mut inside = false;
    let mut up = String::new();
    for line in migration.lines() {
        if line == "-- +goose Up" {
            inside = true;
            continue;
        }
        if line == "-- +goose Down" {
            inside = false;
        }
        if inside && !line.starts_with("-- +goose") {
            up.push_str(line);
            up.push('\n');
        }
    }
    up
}

/// `docker compose` for compose.dev.yml, reading every env file in the server's environment
/// folder and exporting database.env, as the stack itself sees them.
fn development_compose() -> CommandSpec {
    let directory = crate::misty_conf::server_env_dir();
    let mut compose = CommandSpec::new("docker").arg("compose");
    if directory.is_dir() {
        if let Ok(values) = dotenvy::from_path_iter(directory.join("database.env")) {
            for (name, value) in values.flatten() {
                env::set_var(name, value);
            }
        }
        let mut files: Vec<_> = walkdir::WalkDir::new(&directory)
            .into_iter()
            .filter_map(|entry| entry.ok())
            .filter(|entry| entry.file_type().is_file())
            .map(|entry| entry.into_path())
            .filter(|path| path.extension().is_some_and(|ext| ext == "env"))
            .collect();
        files.sort();
        for file in files {
            compose = compose.arg("--env-file").arg(file.as_os_str());
        }
    }
    compose.args(["-f", "compose.dev.yml"])
}

fn var(name: &str) -> Option<String> {
    env::var(name).ok().filter(|value| !value.is_empty())
}

fn set_default(name: &str, value: String) {
    if var(name).is_none() {
        env::set_var(name, value);
    }
}
