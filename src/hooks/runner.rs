//! How suites run: a `──` header and a `✓` line with the time; steps side by side with their
//! output prefixed; and suites one at a time, each in its own process group, under an optional
//! time limit.

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

/// One step of a parallel suite: its commands run in order.
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

/// Runs the steps at the same time, streaming each line prefixed with its step's name, and
/// reports each as it finishes. Periodic updates keep long-running steps visible even when their
/// tools buffer output. Fails with the exit code of the first step (in launch order) that failed.
pub fn parallel(directory: &Path, steps: Vec<Step>) -> Result<()> {
    let output = Arc::new(Mutex::new(()));
    let mut running = Vec::new();
    for step in steps {
        println!("▶ {} started", step.name);
        let directory = directory.to_path_buf();
        let output = output.clone();
        let name = step.name;
        let handle = thread::spawn(move || {
            let started = Instant::now();
            for command in &step.commands {
                let code = stream(&directory, command, name, &output);
                if code != 0 {
                    return (code, started.elapsed().as_secs());
                }
            }
            (0, started.elapsed().as_secs())
        });
        running.push((name, Some(handle)));
    }
    let started = Instant::now();
    let mut last_report = Instant::now();
    let mut codes = vec![0; running.len()];
    loop {
        for (index, (name, slot)) in running.iter_mut().enumerate() {
            if slot.as_ref().is_some_and(|handle| handle.is_finished()) {
                let (code, seconds) = slot.take().unwrap().join().unwrap_or((1, 0));
                let _guard = output.lock();
                if code == 0 {
                    println!("✓ {name} ({seconds}s)");
                } else {
                    println!("✗ {name}");
                }
                codes[index] = code;
            }
        }
        let still: Vec<&str> = running
            .iter()
            .filter(|(_, slot)| slot.is_some())
            .map(|(name, _)| *name)
            .collect();
        if still.is_empty() {
            break;
        }
        if last_report.elapsed() >= Duration::from_secs(15) {
            let _guard = output.lock();
            println!(
                "… still running after {}s: {}",
                started.elapsed().as_secs(),
                still.join(", ")
            );
            last_report = Instant::now();
        }
        thread::sleep(Duration::from_millis(200));
    }
    match codes.into_iter().find(|code| *code != 0) {
        Some(code) => Err(Failed(code).into()),
        None => Ok(()),
    }
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
