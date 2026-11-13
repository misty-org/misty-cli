//! Local development stack: startup, shutdown, logs and its private secrets.

use super::*;

pub fn up(workspace: &Workspace, build: bool, verbose: bool) -> Result<()> {
    environment::check(workspace)?;
    validate_development_secrets(workspace)?;
    ports(workspace)?;
    println!("Starting server containers…");
    let command = development_up_command(build);
    let log = workspace.server.join(".misty/logs/startup.log");
    let result = command.run_logged(&workspace.server, &log, &diagnostic_secrets(workspace)?);
    if verbose && log.exists() {
        eprintln!("{}", fs::read_to_string(&log)?);
    }
    result?;
    let started = Instant::now();
    while health(workspace).is_err() {
        if started.elapsed() > Duration::from_secs(120) {
            status(workspace)?;
            bail!(
                "server was not ready within 120 seconds; run misty server logs SERVICE --tail 100"
            );
        }
        thread::sleep(Duration::from_secs(2));
    }
    status(workspace)?;
    println!("API: {}", development_api_url());
    println!("Worker deployment: misty server deploy");
    Ok(())
}

pub fn initialize_development_secrets(workspace: &Workspace) -> Result<()> {
    ensure_worker_secrets(workspace)
}

pub fn down(workspace: &Workspace, volumes: bool) -> Result<()> {
    development_down_command(volumes).run(&workspace.server)
}

pub(super) fn development_up_command(build: bool) -> CommandSpec {
    let mut command = development_compose().arg("up");
    if build {
        command = command.arg("--build");
    }
    // Compose already recreates services whose image or configuration changed.
    // Avoid forcing the stable Cloudflare connector down during every API
    // rebuild: doing so leaves the named hostname with no connector and turns
    // one backend restart into a burst of misleading 530/CORS failures.
    command.args(["--remove-orphans", "--detach"])
}

pub(super) fn development_down_command(volumes: bool) -> CommandSpec {
    let mut command = development_compose().arg("down");
    if volumes {
        command = command.arg("--volumes");
    }
    command.arg("--remove-orphans")
}

pub fn logs(workspace: &Workspace, service: Option<&str>, follow: bool, tail: u32) -> Result<()> {
    let mut command = development_compose().args(["logs", "--tail", &tail.to_string()]);
    if follow {
        command = command.arg("--follow");
    }
    if let Some(service) = service {
        if service.starts_with('-') {
            bail!("invalid service name");
        }
        command = command.arg(if service == "api" {
            "misty-api"
        } else {
            service
        });
    }
    command.run(&workspace.server)
}

pub fn validate_development_secrets(workspace: &Workspace) -> Result<()> {
    let values = environment::read(workspace)?;
    let signing = SigningKey::from_pkcs8_der(
        &STANDARD.decode(required_map_value(&values, TICKET_PRIVATE_KEY)?)?,
    )?;
    if STANDARD.encode(VerifyingKey::from(&signing).to_bytes())
        != required_map_value(&values, TICKET_PUBLIC_KEY)?
    {
        bail!("the Worker ticket keys do not match; restore the matching pair");
    }
    for name in [CONTROL_SECRET, PROJECTION_SECRET] {
        required_map_value(&values, name)?;
    }
    let salt = STANDARD.decode(required_map_value(&values, ROOM_SALT)?)?;
    if salt.len() < 32 {
        bail!("room salt must contain at least 32 bytes");
    }
    Ok(())
}

pub(super) fn diagnostic_secrets(workspace: &Workspace) -> Result<Vec<String>> {
    Ok(environment::effective(workspace)?
        .into_iter()
        .filter(|(name, _)| {
            ["TOKEN", "SECRET", "KEY", "PASSWORD", "PEPPER", "SALT"]
                .iter()
                .any(|part| name.contains(part))
        })
        .map(|(_, value)| value)
        .collect())
}

pub fn deploy_development(workspace: &Workspace) -> Result<()> {
    environment::check(workspace)?;
    println!("Deploying development collaboration Worker…");
    development_compose()
        .args(["run", "--rm", "--no-deps", "--build", "setup", "deploy"])
        .run_logged(
            &workspace.server,
            &workspace.server.join(".misty/logs/deploy.log"),
            &diagnostic_secrets(workspace)?,
        )
}
