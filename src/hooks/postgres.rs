//! A disposable PostgreSQL container for test suites, on the same pinned image production runs.
//!
//! The container runs attached, as a child of this process, so an interrupt or a stop at the
//! commit limit reaches `docker run` too, which passes it on and removes the container. Dropping
//! the handle removes it as well.

use std::{
    path::Path,
    process::{Child, Stdio},
    thread,
    time::{Duration, Instant},
};

use anyhow::{bail, Context, Result};

use crate::process::CommandSpec;

pub const IMAGE: &str = "pgvector/pgvector:pg16@sha256:1d533553fefe4f12e5d80c7b80622ba0c382abb5758856f52983d8789179f0fb";

pub struct Disposable {
    pub name: String,
    pub port: u16,
    pub user: String,
    child: Child,
}

pub struct Options<'a> {
    pub label: &'a str,
    pub user: &'a str,
    pub password: &'a str,
    pub database: &'a str,
    /// The in-memory data directory's size, such as `2g`.
    pub memory: &'a str,
    /// Extra server settings, as `-c` pairs.
    pub settings: &'a [&'a str],
}

impl Disposable {
    /// Starts the container and waits until it accepts connections.
    pub fn start(options: Options<'_>) -> Result<Self> {
        let name = format!("misty-{}-{}", options.label, std::process::id());
        let mut command = std::process::Command::new("docker");
        // The data directory lives in memory: tests churn table files constantly, and Docker's
        // disk is slow on macOS. Nothing here needs to survive the run.
        command
            .args(["run", "--rm", "--name", name.as_str(), "-p", "127.0.0.1::5432"])
            .arg("--tmpfs")
            .arg(format!("/var/lib/postgresql/data:rw,size={}", options.memory))
            .arg("-e")
            .arg(format!("POSTGRES_USER={}", options.user))
            .arg("-e")
            .arg(format!("POSTGRES_PASSWORD={}", options.password))
            .arg("-e")
            .arg(format!("POSTGRES_DB={}", options.database))
            .arg(IMAGE);
        for setting in options.settings {
            command.args(["-c", setting]);
        }
        let child = command
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .context("could not start Docker")?;
        let mut database = Self {
            name,
            port: 0,
            user: options.user.to_string(),
            child,
        };
        database.port = database.wait_for_port()?;
        database.wait_until_ready(options.database)?;
        Ok(database)
    }

    fn wait_for_port(&mut self) -> Result<u16> {
        let deadline = Instant::now() + Duration::from_secs(120);
        loop {
            if let Ok(mapping) = self.docker(&["port", &self.name, "5432/tcp"]).capture(Path::new(".")) {
                if let Some(port) = mapping
                    .lines()
                    .next()
                    .and_then(|line| line.rsplit(':').next())
                    .and_then(|port| port.trim().parse().ok())
                {
                    return Ok(port);
                }
            }
            if self.child.try_wait()?.is_some() || Instant::now() > deadline {
                bail!("Test PostgreSQL did not start (is Docker running?)");
            }
            thread::sleep(Duration::from_millis(500));
        }
    }

    fn wait_until_ready(&mut self, database: &str) -> Result<()> {
        let mut waited = 0;
        while !self
            .docker(&["exec", &self.name, "pg_isready", "-U", &self.user, "-d", database])
            .succeeds(Path::new("."))
        {
            // A container that exits (for example, Docker out of disk) never becomes ready.
            waited += 1;
            if self.child.try_wait()?.is_some() || waited > 60 {
                eprintln!("Test PostgreSQL did not start:");
                let _ = self
                    .docker(&["logs", "--tail", "20", &self.name])
                    .run(Path::new("."));
                bail!("Test PostgreSQL did not start");
            }
            thread::sleep(Duration::from_secs(1));
        }
        Ok(())
    }

    /// Runs a command inside the container, such as `createdb` or `psql`.
    pub fn exec(&self, arguments: &[&str]) -> Result<()> {
        let mut all = vec!["exec", self.name.as_str()];
        all.extend_from_slice(arguments);
        self.docker(&all).capture(Path::new(".")).map(|_| ())
    }

    fn docker(&self, arguments: &[&str]) -> CommandSpec {
        CommandSpec::new("docker").args(arguments)
    }
}

impl Drop for Disposable {
    fn drop(&mut self) {
        let _ = CommandSpec::new("docker")
            .args(["rm", "-fv", self.name.as_str()])
            .succeeds(Path::new("."));
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
