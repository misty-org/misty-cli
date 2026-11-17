//! Validation and status reports for one target's environment.

use super::*;

pub fn validate(workspace: &Workspace) -> Result<()> {
    read_cli_files(workspace)?;
    let values = effective(workspace)?;
    let missing = required()
        .iter()
        .filter(|name| {
            values
                .get(**name)
                .is_none_or(|value| value.trim().is_empty())
        })
        .copied()
        .collect::<Vec<_>>();
    if !missing.is_empty() {
        bail!(
            "server settings are missing required values: {}",
            missing.join(", ")
        );
    }

    {
        {
            let name = "MISTY_DEV_API_ORIGIN";
            let value = values.get(name).context("missing API origin")?;
            let url = url::Url::parse(value).context("API origin must be a valid HTTPS origin")?;
            if url.scheme() != "https"
                || url.host_str().is_none()
                || !matches!(url.path(), "" | "/")
                || url.query().is_some()
                || url.fragment().is_some()
                || !url.username().is_empty()
                || url.password().is_some()
            {
                bail!("{name} must be an HTTPS origin without a path or credentials");
            }
            if url.host_str()
                != values
                    .get("MISTY_DEV_API_TUNNEL_HOSTNAME")
                    .map(String::as_str)
            {
                bail!("API origin and tunnel hostname must match");
            }
        }
        if let Some(port) = values.get("MISTY_HOST_PORT") {
            if port.parse::<u16>().ok().is_none_or(|p| p == 0) {
                bail!("MISTY_HOST_PORT must be a port between 1 and 65535");
            }
        }
    }
    if let Some(email) = values.get("MISTY_OWNER_EMAIL") {
        validate_owner_email(email)?;
    }
    validate_images(
        values.get("MISTY_API_IMAGE").map(String::as_str),
        values.get("MISTY_TOOLS_IMAGE").map(String::as_str),
    )?;
    if let Some(value) = values.get("MISTY_TUNNEL_EXTRA_INGRESS") {
        let hostname = values
            .get("MISTY_DEV_API_TUNNEL_HOSTNAME")
            .map(String::as_str)
            .unwrap_or_default();
        parse_extra_ingress(value, hostname)?;
    }
    Ok(())
}

/// The server trims and lowercases MISTY_OWNER_EMAIL and compares it with the Google account
/// signing in, so anything but one address can never match. Empty means no owner.
pub(super) fn validate_owner_email(value: &str) -> Result<()> {
    let email = value.trim();
    if email.is_empty() {
        return Ok(());
    }
    let one_address = email.split_once('@').is_some_and(|(local, domain)| {
        !local.is_empty() && !domain.is_empty() && !domain.contains('@')
    });
    if !one_address || email.chars().any(char::is_whitespace) {
        bail!("MISTY_OWNER_EMAIL must be one email address: the owner's Google account");
    }
    Ok(())
}

const PUBLISHED_API_IMAGE: &str = "ghcr.io/misty-org/misty-server-api:";
const PUBLISHED_TOOLS_IMAGE: &str = "ghcr.io/misty-org/misty-server-tools:";

/// Compose puts MISTY_API_IMAGE and MISTY_TOOLS_IMAGE in `image:` as they are, and treats an
/// empty one as unset. misty-server publishes both images for each commit and says to run
/// both from the same one.
pub(super) fn validate_images(api: Option<&str>, tools: Option<&str>) -> Result<()> {
    let api = api.filter(|value| !value.is_empty());
    let tools = tools.filter(|value| !value.is_empty());
    for (name, value, example) in [
        ("MISTY_API_IMAGE", api, PUBLISHED_API_IMAGE),
        ("MISTY_TOOLS_IMAGE", tools, PUBLISHED_TOOLS_IMAGE),
    ] {
        if value
            .is_some_and(|image| image.chars().any(char::is_whitespace) || image.contains("://"))
        {
            bail!("{name} must be an image reference such as {example}<commit SHA>, without a scheme or spaces");
        }
    }
    let tag = |image: &str, prefix: &str| {
        image
            .strip_prefix(prefix)
            .map(|rest| rest.split('@').next().unwrap_or(rest).to_owned())
    };
    if let (Some(Some(api)), Some(Some(tools))) = (
        api.map(|image| tag(image, PUBLISHED_API_IMAGE)),
        tools.map(|image| tag(image, PUBLISHED_TOOLS_IMAGE)),
    ) {
        if api != tools {
            bail!("MISTY_API_IMAGE and MISTY_TOOLS_IMAGE must be the same misty-server release");
        }
    }
    Ok(())
}

/// MISTY_TUNNEL_EXTRA_INGRESS as misty-server's setup reads it (deploy/setup/inputs.go): a
/// comma-separated list of host=service, such as billing.misty.example=http://billing:8091.
/// Each host is a DNS name other than the tunnel's own `hostname`, listed once, and each
/// service an http or https URL with a host and no path, query or user. Setup also requires
/// each host to be in the hostname's zone, which only Cloudflare knows.
pub(super) fn parse_extra_ingress(value: &str, hostname: &str) -> Result<Vec<(String, String)>> {
    let mut rules = Vec::new();
    let mut seen = BTreeSet::from([hostname.trim().to_lowercase()]);
    for entry in value.split(',') {
        let entry = entry.trim();
        if entry.is_empty() {
            continue;
        }
        let (host, service) = entry.split_once('=').unwrap_or_default();
        let (host, service) = (host.trim().to_lowercase(), service.trim());
        if host.is_empty() || service.is_empty() {
            bail!("MISTY_TUNNEL_EXTRA_INGRESS entries are host=service, such as billing.misty.example=http://billing:8091: {entry:?}");
        }
        valid_hostname(&format!("MISTY_TUNNEL_EXTRA_INGRESS host {host}"), &host)?;
        if !seen.insert(host.clone()) {
            bail!("MISTY_TUNNEL_EXTRA_INGRESS lists {host} twice, or as the tunnel's own hostname");
        }
        let routable = url::Url::parse(service).is_ok_and(|url| {
            matches!(url.scheme(), "http" | "https")
                && url.host_str().is_some_and(|host| !host.is_empty())
                && url.username().is_empty()
                && url.password().is_none()
                && url.path().trim_matches('/').is_empty()
                && url.query().is_none_or(str::is_empty)
                && url.fragment().is_none_or(str::is_empty)
        });
        if !routable {
            bail!("MISTY_TUNNEL_EXTRA_INGRESS service for {host} must be an http or https URL without a path, such as http://billing:8091: {service:?}");
        }
        rules.push((host, service.trim_end_matches('/').to_owned()));
    }
    Ok(rules)
}

/// A DNS name as setup accepts one: at most 253 characters, at least two labels, each label
/// lowercase letters, digits and inner hyphens, at most 63 long.
fn valid_hostname(name: &str, host: &str) -> Result<()> {
    let label = |label: &str| {
        let bytes = label.as_bytes();
        (1..=63).contains(&bytes.len())
            && bytes
                .iter()
                .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || *byte == b'-')
            && bytes.first() != Some(&b'-')
            && bytes.last() != Some(&b'-')
    };
    if host.len() > 253 || !host.contains('.') || !host.split('.').all(label) {
        bail!("{name} must be a DNS name such as cloud.misty.example, without scheme or path");
    }
    Ok(())
}

pub fn check(workspace: &Workspace) -> Result<()> {
    validate(workspace)?;
    println!("Server settings are valid.");
    Ok(())
}

pub fn status(workspace: &Workspace) -> Result<()> {
    let values = read(workspace)?;
    println!("Server settings");
    for spec in FILES {
        println!("  {}", spec.path);
        for name in spec.names {
            let state = match values.get(*name) {
                Some(value) if !value.trim().is_empty() => "set",
                Some(_) => "empty",
                None => "unset",
            };
            println!("    {name:<48} {state}");
        }
    }
    let missing = required()
        .iter()
        .filter(|name| {
            values
                .get(**name)
                .is_none_or(|value| value.trim().is_empty())
        })
        .copied()
        .collect::<Vec<_>>();
    if missing.is_empty() {
        println!("  required values                    ready");
    } else {
        println!(
            "  missing required                   {}",
            missing.join(", ")
        );
    }
    Ok(())
}
