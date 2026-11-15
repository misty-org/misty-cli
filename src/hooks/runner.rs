//! How suites run: a `──` header and a `✓` line with the time; steps one after another with their
//! output prefixed, each held to a few cores (`jobs`); and suites one at a time, each in its own
//! process group, under an optional time limit.

use std::{
    io::{BufRead, BufReader, Read},
    path::Path,
    process::{Child, Stdio},
    sync::{Arc, Mutex},
    thread,
    time::{Duration, Instant},
};

use anyhow::{Context, Result};

use crate::process::CommandSpec;

/// A check failed with this exit code; `misty` exits with it unchanged.
#[derive(Debug)]
pub struct Failed(pub i32);

impl std::fmt::Display for Failed {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "a check failed (exit code {})", self.0)
    }
}

impl std::error::Error for Failed {}

/// One step of a suite: its commands run in order.
pub struct Step {
    pub name: &'static str,
    pub commands: Vec<CommandSpec>,
}

impl Step {
    pub fn new(name: &'static str, commands: impl IntoIterator<Item = CommandSpec>) -> Self {
        Self {
            name,
            commands: commands.into_iter().collect(),
        }
    }
}

/// Runs one suite in this process, framed by its name and how long it took.
pub fn run(name: &str, suite: impl FnOnce() -> Result<()>) -> Result<()> {
    println!("── {name}");
    let started = Instant::now();
    suite()?;
    println!("✓ {name} ({}s)", started.elapsed().as_secs());
    Ok(())
}

/// Runs a list of commands in order, stopping at the first that fails.
pub fn commands(directory: &Path, commands: impl IntoIterator<Item = CommandSpec>) -> Result<()> {
    for command in commands {
        let code = command.exit_code(directory)?;
        if code != 0 {
            return Err(Failed(code).into());
        }
    }
    Ok(())
}

/// How many CPU cores a check may keep busy: `MISTY_HOOK_JOBS` (also from misty.conf's `[cli]`),
/// else 4, so a commit or push never takes the whole computer.
pub fn jobs() -> String {
    std::env::var("MISTY_HOOK_JOBS")
        .ok()
        .and_then(|value| value.trim().parse::<usize>().ok())
        .filter(|jobs| *jobs > 0)
        .unwrap_or(4)
        .to_string()
}

/// A command held to `jobs()` cores: Cargo's build jobs, Rust's test threads and Go's threads.
pub fn light(command: CommandSpec) -> CommandSpec {
    let jobs = jobs();
    command
        .env("CARGO_BUILD_JOBS", &jobs)
        .env("RUST_TEST_THREADS", &jobs)
        .env("GOMAXPROCS", &jobs)
}

/// Runs the steps one after another, each held to `jobs()` cores, streaming each line prefixed
/// with its step's name. Stops at the first step that fails, with its exit code.
pub fn in_turn(directory: &Path, steps: Vec<Step>) -> Result<()> {
    let output = Arc::new(Mutex::new(()));
    for step in steps {
        println!("▶ {}", step.name);
        let started = Instant::now();
        for command in step.commands {
            let code = stream(directory, &light(command), step.name, &output);
            if code != 0 {
                println!("✗ {}", step.name);
                return Err(Failed(code).into());
            }
        }
        println!("✓ {} ({}s)", step.name, started.elapsed().as_secs());
    }
    Ok(())
}

/// Runs one command with its stdout and stderr streamed line by line under `[name]`.
fn stream(directory: &Path, command: &CommandSpec, name: &str, output: &Arc<Mutex<()>>) -> i32 {
    let child = command
        .command(directory)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn();
    let mut child = match child {
        Ok(child) => child,
        Err(error) => {
            let _guard = output.lock();
            println!("[{name}] could not start {}: {error}", command.display());
            return 127;
        }
    };
    let readers: Vec<_> = [
        child.stdout.take().map(|pipe| Box::new(pipe) as Box<dyn Read + Send>),
        child.stderr.take().map(|pipe| Box::new(pipe) as Box<dyn Read + Send>),
    ]
    .into_iter()
    .flatten()
    .map(|pipe| {
        let output = output.clone();
        let name = name.to_string();
        thread::spawn(move || {
            for line in BufReader::new(pipe).lines().map_while(Result::ok) {
                let _guard = output.lock();
                println!("[{name}] {line}");
            }
        })
    })
    .collect();
    for reader in readers {
        let _ = reader.join();
    }
    child
        .wait()
        .map(|status| status.code().unwrap_or(1))
        .unwrap_or(1)
}

/// One suite at a time, in the order given, each as `misty hook run <suite>` in its own process
/// group. With a limit (seconds, 0 for none), a suite still going when it's reached is stopped
/// and named as unchecked; a failure before then still fails.
pub fn sequence(root: &Path, limit: u64, suites: &[&str]) -> Result<()> {
    let started = Instant::now();
    let over = || limit > 0 && started.elapsed().as_secs() >= limit;
    let program = std::env::current_exe().context("could not find the misty binary")?;
    let mut unfinished = Vec::new();
    for &name in suites {
        if over() {
            unfinished.push(name);
            continue;
        }
        let mut child = spawn_suite(&program, root, name)?;
        let status = loop {
            if let Some(status) = child.try_wait()? {
                break Some(status);
            }
            if over() {
                stop_group(&mut child);
                unfinished.push(name);
                break None;
            }
            thread::sleep(Duration::from_millis(200));
        };
        if let Some(status) = status {
            if !status.success() {
                return Err(Failed(status.code().unwrap_or(1)).into());
            }
        }
    }
    if !unfinished.is_empty() {
        println!(
            "⚠ Stopped at the {limit}s limit, so these weren't fully checked: {}.",
            unfinished.join(" ")
        );
    }
    Ok(())
}

fn spawn_suite(program: &Path, root: &Path, name: &str) -> Result<Child> {
    let mut command = std::process::Command::new(program);
    command.args(["hook", "run", name]).current_dir(root);
    // Its own process group, so stopping it stops everything it started.
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    command
        .spawn()
        .with_context(|| format!("could not start the {name} suite"))
}

/// Asks the suite's process group to stop so it can clean up, then insists: nothing may hold a
/// commit past its limit.
#[cfg(unix)]
fn stop_group(child: &mut Child) {
    let group = -(child.id() as i32);
    // SAFETY: kill only sends a signal; a group that has already gone just fails.
    unsafe { libc::kill(group, libc::SIGTERM) };
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline && unsafe { libc::kill(group, 0) } == 0 {
        let _ = child.try_wait();
        thread::sleep(Duration::from_millis(500));
    }
    unsafe { libc::kill(group, libc::SIGKILL) };
    let _ = child.wait();
}

#[cfg(not(unix))]
fn stop_group(child: &mut Child) {
    let _ = child.kill();
    let _ = child.wait();
}
