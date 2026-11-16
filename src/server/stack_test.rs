//! `misty server stack-test`: runs the whole server stack (compose.yml) against the fake
//! Cloudflare API, on its own Compose project and free ports, and removes everything it made
//! afterwards. It checks that the stack becomes healthy, that a second start keeps every secret
//! and uploads nothing again, that a backup reaches the bucket and restores, and that a lost
//! secrets volume over existing data stops the stack.

use std::{
    fs,
    net::TcpListener,
    path::{Path, PathBuf},
    process::{Child, Stdio},
    thread,
    time::Duration,
};

use anyhow::{bail, Context, Result};
use sha2::{Digest, Sha256};

use crate::process::CommandSpec;

struct Stack {
    server: PathBuf,
    project: String,
    work: tempfile::TempDir,
    fake_port: u16,
    host_port: u16,
    console_port: u16,
    fake: Option<Child>,
}

pub fn stack_test(server: &Path) -> Result<()> {
    let mut stack = Stack {
        server: server.to_path_buf(),
        project: std::env::var("MISTY_STACK_TEST_PROJECT")
            .unwrap_or_else(|_| "misty-stacktest".into()),
        work: tempfile::tempdir()?,
        fake_port: free_port()?,
        host_port: free_port()?,
        console_port: free_port()?,
        fake: None,
    };
    let result = stack.run();
    if result.is_err() {
        let _ = stack
            .compose(&["logs", "--no-color", "--tail", "60", "setup", "migrate", "api"])
            .run(&stack.server);
    }
    stack.clean_up();
    result
}

impl Stack {
    fn run(&mut self) -> Result<()> {
        let s3_port = free_port()?;
        let work = self.work.path().to_path_buf();
        let env_dir = work.join("env");
        std::env::set_var("MISTY_HOST_PORT", self.host_port.to_string());
        std::env::set_var("MISTY_CONSOLE_PORT", self.console_port.to_string());
        std::env::set_var("MISTY_ENV_DIR", &env_dir);

        // A throwaway CA is the only one the containers trust, so a call that would leave for a
        // real HTTPS service (R2, Google) fails instead.
        step(&format!(
            "test CA and fake Cloudflare on :{} (S3 on :{s3_port})",
            self.fake_port
        ));
        let ca = work.join("ca");
        fs::create_dir_all(&ca)?;
        let openssl = |arguments: &[&str]| CommandSpec::new("openssl").args(arguments).capture(&ca);
        openssl(&[
            "req", "-x509", "-newkey", "rsa:2048", "-nodes", "-days", "1", "-subj",
            "/CN=misty-stack-test-ca", "-keyout", "ca.key", "-out", "ca.pem",
        ])?;
        openssl(&[
            "req", "-newkey", "rsa:2048", "-nodes", "-subj", "/CN=host.docker.internal",
            "-keyout", "tls.key", "-out", "tls.csr",
        ])?;
        fs::write(ca.join("ext"), "subjectAltName=DNS:host.docker.internal\n")?;
        openssl(&[
            "x509", "-req", "-days", "1", "-in", "tls.csr", "-CA", "ca.pem", "-CAkey", "ca.key",
            "-CAcreateserial", "-extfile", "ext", "-out", "tls.pem",
        ])?;
        make_readable(&ca)?;
        let fake_binary = work.join("fake-cloudflare");
        CommandSpec::new("go")
            .args(["build", "-o"])
            .arg(fake_binary.as_os_str())
            .arg("./deploy/setup/fakecloudflare/cmd/fake-cloudflare")
            .run(&self.server)?;
        let log = fs::File::create(work.join("fake.log"))?;
        self.fake = Some(
            std::process::Command::new(&fake_binary)
                .arg("-addr")
                .arg(format!("127.0.0.1:{}", self.fake_port))
                .arg("-tls-addr")
                .arg(format!("127.0.0.1:{s3_port}"))
                .arg("-tls-cert")
                .arg(ca.join("tls.pem"))
                .arg("-tls-key")
                .arg(ca.join("tls.key"))
                .stdout(log.try_clone()?)
                .stderr(log)
                .spawn()
                .context("could not start the fake Cloudflare API")?,
        );

        fs::create_dir_all(&env_dir)?;
        let fake = format!("http://host.docker.internal:{}", self.fake_port);
        fs::write(env_dir.join("misty.env"), "MISTY_HOSTNAME=cloud.misty.example\n")?;
        fs::write(
            env_dir.join("cloudflare.env"),
            format!(
                "CLOUDFLARE_API_TOKEN=fake-cloudflare-token\nCLOUDFLARE_ACCOUNT_ID=0123456789abcdef0123456789abcdef\nCLOUDFLARE_ZONE_ID=fedcba9876543210fedcba9876543210\nMISTY_CLOUDFLARE_API_URL={fake}/client/v4\nMISTY_R2_ENDPOINT_URL=https://host.docker.internal:{s3_port}\n"
            ),
        )?;
        fs::write(
            env_dir.join("google.env"),
            "GOOGLE_CLIENT_ID=test.apps.googleusercontent.com\nGOOGLE_CLIENT_SECRET=test\n",
        )?;
        // The tunnel connector would dial Cloudflare's real edge; here it only checks that setup
        // left it a token.
        fs::write(
            work.join("override.yml"),
            format!(
                r#"x-test-ca: &test-ca
  environment:
    SSL_CERT_FILE: /run/misty-test/ca.pem
  volumes:
    - {ca}:/run/misty-test:ro
services:
  setup: *test-ca
  migrate: *test-ca
  api: *test-ca
  backups: *test-ca
  console: *test-ca
  cloudflared:
    entrypoint: ["/bin/sh", "-ec", "test -s /run/misty/secrets/cloudflared/token; exec sleep infinity"]
    healthcheck: {{disable: true}}
"#,
                ca = ca.display()
            ),
        )?;

        step("first start");
        self.compose(&["up", "-d", "--build"]).run(&self.server)?;
        self.wait_healthy()?;
        let first_state = self.state()?;
        let first = self.secrets_digest()?;
        println!("healthy; {first_state}");

        step("the console answers on its loopback port");
        let console = format!("http://127.0.0.1:{}/health", self.console_port);
        retry(30, || curl(&["-fsS", &console]).is_ok(), "the console never answered")?;
        let logs = self
            .compose(&["logs", "--no-color", "console"])
            .capture(&self.server)?;
        if !logs.contains(&format!("http://127.0.0.1:{}/?token=", self.console_port)) {
            bail!("FAIL: the console printed no sign-in link");
        }

        step("the first account registers; the next needs an invitation");
        expect(self.register("owner")?, "201", "the first account could not register")?;
        expect(
            self.register("stranger")?,
            "403",
            "a second account registered without an invitation",
        )?;

        step("second start keeps every secret");
        self.compose(&["up", "-d"]).run(&self.server)?;
        self.wait_healthy()?;
        if self.secrets_digest()? != first {
            bail!("FAIL: a second up changed the secrets");
        }
        let state = self.state()?;
        if !state.contains(r#""uploads":1"#) {
            bail!("FAIL: the Workers were uploaded again: {state}");
        }
        if !state.contains(r#""tunnels":1"#) {
            bail!("FAIL: a second tunnel was made");
        }

        step("a backup reaches the bucket");
        self.compose(&["exec", "-T", "backups", "misty-setup", "backup", "--now"])
            .run(&self.server)?;
        if !self.state()?.contains(r#""objects":1"#) {
            bail!("FAIL: no backup object in the bucket");
        }
        let backups = self
            .compose(&["exec", "-T", "backups", "misty-setup", "backups"])
            .capture(&self.server)?;
        if !backups.contains("dump.age") {
            bail!("FAIL: the backup is not listed");
        }

        step("the backup restores");
        self.compose(&["stop", "api"]).capture(&self.server)?;
        self.compose(&["exec", "-T", "backups", "misty-setup", "restore", "latest"])
            .run(&self.server)?;
        self.compose(&["up", "-d"]).run(&self.server)?;
        self.wait_healthy()?;
        expect(
            self.register("owner")?,
            "409",
            "the restored database lost its account",
        )?;

        step("lost secrets over existing data stop the stack");
        self.compose(&["down"]).run(&self.server)?;
        CommandSpec::new("docker")
            .args(["volume", "rm"])
            .arg(format!("{}_secrets", self.project))
            .capture(&self.server)?;
        if self.compose(&["up", "-d"]).succeeds(&self.server) {
            bail!("FAIL: the stack started without its secrets");
        }
        let setup = self
            .compose(&["logs", "--no-color", "setup"])
            .capture(&self.server)
            .unwrap_or_default();
        let Some(refusal) = setup.lines().rev().find(|line| line.contains("secrets lost"))
        else {
            bail!("FAIL: setup did not explain the refusal");
        };
        let api = self
            .compose(&["ps", "--status", "running", "-q", "api"])
            .capture(&self.server)?;
        if !api.trim().is_empty() {
            bail!("FAIL: the API started anyway");
        }
        println!("refused: {refusal}");

        step("passed");
        Ok(())
    }

    fn compose(&self, arguments: &[&str]) -> CommandSpec {
        CommandSpec::new("docker")
            .args(["compose", "-p", self.project.as_str(), "-f"])
            .arg(self.server.join("compose.yml").as_os_str())
            .arg("-f")
            .arg(self.work.path().join("override.yml").as_os_str())
            .args(arguments)
    }

    fn wait_healthy(&self) -> Result<()> {
        let url = format!("http://127.0.0.1:{}/health", self.host_port);
        let mut health = String::new();
        retry(
            90,
            || match curl(&["-fsS", &url]) {
                Ok(body) => {
                    health = body;
                    true
                }
                Err(_) => false,
            },
            "the API never answered /health",
        )?;
        if !health.contains(r#""status":"ok""#) {
            bail!("FAIL: health is not ok: {health}");
        }
        Ok(())
    }

    fn state(&self) -> Result<String> {
        curl(&["-fsS", &format!("http://127.0.0.1:{}/_state", self.fake_port)])
    }

    fn secrets_digest(&self) -> Result<String> {
        let secrets = self
            .compose(&[
                "run", "--rm", "--no-deps", "-T", "--entrypoint", "sh", "setup", "-c",
                "cat /run/misty/secrets/server.env /run/misty/secrets/postgres/superuser-password /run/misty/secrets/backup/identity",
            ])
            .capture(&self.server)?;
        Ok(format!("{:x}", Sha256::digest(secrets.as_bytes())))
    }

    fn register(&self, name: &str) -> Result<String> {
        curl(&[
            "-s", "-o", "/dev/null", "-w", "%{http_code}", "-X", "POST",
            &format!("http://127.0.0.1:{}/v1/register", self.host_port),
            "-H", "Content-Type: application/json",
            "-H", "X-Misty-CSRF: 1",
            "-H", "Origin: tauri://localhost",
            "-d",
            &format!(
                r#"{{"username":"{name}","email":"{name}@misty.example","password":"stack test password"}}"#
            ),
        ])
    }

    fn clean_up(&mut self) {
        let _ = self
            .compose(&["down", "--volumes", "--remove-orphans"])
            .succeeds(&self.server);
        let _ = CommandSpec::new("docker")
            .args(["image", "rm"])
            .arg(format!("{}-tools:local", self.project))
            .arg(format!("{}-api:local", self.project))
            .succeeds(&self.server);
        if let Some(mut fake) = self.fake.take() {
            let _ = fake.kill();
            let _ = fake.wait();
        }
    }
}

fn step(name: &str) {
    println!("\n== {name}");
}

fn expect(actual: String, wanted: &str, failure: &str) -> Result<()> {
    if actual.trim() != wanted {
        bail!("FAIL: {failure} (HTTP {})", actual.trim());
    }
    Ok(())
}

fn retry(attempts: u32, mut check: impl FnMut() -> bool, failure: &str) -> Result<()> {
    for _ in 0..attempts {
        if check() {
            return Ok(());
        }
        thread::sleep(Duration::from_secs(2));
    }
    bail!("FAIL: {failure}")
}

fn curl(arguments: &[&str]) -> Result<String> {
    let output = std::process::Command::new("curl")
        .args(arguments)
        .stderr(Stdio::null())
        .output()
        .context("could not start curl")?;
    if !output.status.success() {
        bail!("curl failed");
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

fn free_port() -> Result<u16> {
    Ok(TcpListener::bind("127.0.0.1:0")?.local_addr()?.port())
}

#[cfg(unix)]
fn make_readable(directory: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    for entry in fs::read_dir(directory)? {
        fs::set_permissions(entry?.path(), fs::Permissions::from_mode(0o644))?;
    }
    Ok(())
}

#[cfg(not(unix))]
fn make_readable(_: &Path) -> Result<()> {
    Ok(())
}
