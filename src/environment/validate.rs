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
    for (name, ceiling) in BYTE_LIMITS {
        if let Some(value) = values.get(*name) {
            validate_bytes(name, value, *ceiling)?;
        }
    }
    for name in TRANSFER_TTLS {
        if let Some(value) = values.get(name) {
            validate_transfer_ttl(name, value)?;
        }
    }
    if let Some(value) = values.get("JOURNAL_COLLAB_TICKET_PUBLIC_KEY_PREVIOUS") {
        validate_public_key("JOURNAL_COLLAB_TICKET_PUBLIC_KEY_PREVIOUS", value)?;
    }
    Ok(())
}

const MIB: i64 = 1 << 20;

/// Byte settings and the most each may be (misty-server's internal/spaces/permissions.go: an
/// upload limit can only lower its default). The egress budgets have no ceiling.
const BYTE_LIMITS: &[(&str, Option<i64>)] = &[
    ("MISTY_EGRESS_MAX_BYTES_PER_IDENTITY_DAY", None),
    ("MISTY_EGRESS_MAX_BYTES_PER_DAY", None),
    ("MISTY_LIBRARY_MAX_FILE_BYTES", Some(100 * MIB)),
    ("MISTY_CHAT_ATTACHMENT_MAX_FILE_BYTES", Some(10 * MIB)),
    ("MISTY_NOTE_ATTACHMENT_MAX_FILE_BYTES", Some(15 * MIB)),
    ("MISTY_DRAWING_ASSET_MAX_FILE_BYTES", Some(15 * MIB)),
];

const TRANSFER_TTLS: [&str; 2] = ["MISTY_R2_UPLOAD_URL_TTL", "MISTY_R2_DOWNLOAD_URL_TTL"];

/// The server reads a byte setting as a whole number and quietly keeps its default when that
/// fails or isn't positive, then refuses to start when an upload limit is above its ceiling.
/// Here both are refused. Empty means the default.
pub(super) fn validate_bytes(name: &str, value: &str, ceiling: Option<i64>) -> Result<()> {
    let value = value.trim();
    if value.is_empty() {
        return Ok(());
    }
    let bytes = value.parse::<i64>().ok().filter(|bytes| *bytes > 0);
    match (bytes, ceiling) {
        (Some(bytes), Some(ceiling)) if bytes > ceiling => {
            bail!("{name} must be between 1 and {ceiling} bytes")
        }
        (Some(_), _) => Ok(()),
        (None, Some(ceiling)) => bail!("{name} must be between 1 and {ceiling} bytes"),
        (None, None) => bail!("{name} must be a whole number of bytes above 0"),
    }
}

/// The server reads a link lifetime as a Go duration, quietly keeps its default when that fails
/// or isn't positive, and refuses to start outside 30s to 60m. Empty means the default.
pub(super) fn validate_transfer_ttl(name: &str, value: &str) -> Result<()> {
    let value = value.trim();
    if value.is_empty() {
        return Ok(());
    }
    if !go_duration_seconds(value).is_some_and(|seconds| (30.0..=3600.0).contains(&seconds)) {
        bail!("{name} must be a duration between 30s and 60m, such as 15m or 90s");
    }
    Ok(())
}

/// Seconds in a Go duration such as 1h30m, 2.5s or 300ms (time.ParseDuration), or None.
pub(super) fn go_duration_seconds(value: &str) -> Option<f64> {
    let (sign, mut rest) = match value.as_bytes().first() {
        Some(b'-') => (-1.0, &value[1..]),
        Some(b'+') => (1.0, &value[1..]),
        _ => (1.0, value),
    };
    if rest == "0" {
        return Some(0.0);
    }
    if rest.is_empty() {
        return None;
    }
    let mut total = 0.0;
    while !rest.is_empty() {
        let number = rest
            .find(|c: char| !(c.is_ascii_digit() || c == '.'))
            .unwrap_or(rest.len());
        let (digits, after) = rest.split_at(number);
        if digits.is_empty() || digits == "." || digits.matches('.').count() > 1 {
            return None;
        }
        let unit = after
            .find(|c: char| c.is_ascii_digit() || c == '.')
            .unwrap_or(after.len());
        let (unit, after) = after.split_at(unit);
        let scale = match unit {
            "ns" => 1e-9,
            "us" | "µs" | "μs" => 1e-6,
            "ms" => 1e-3,
            "s" => 1.0,
            "m" => 60.0,
            "h" => 3600.0,
            _ => return None,
        };
        total += digits.parse::<f64>().ok()? * scale;
        rest = after;
    }
    Some(sign * total)
}

/// An Ed25519 public key as setup and the CLI write one: 32 bytes, standard base64.
pub(super) fn validate_public_key(name: &str, value: &str) -> Result<()> {
    use base64::Engine;
    let value = value.trim();
    if value.is_empty() {
        return Ok(());
    }
    let decoded = base64::engine::general_purpose::STANDARD.decode(value);
    if !decoded.is_ok_and(|key| key.len() == 32) {
        bail!("{name} must be an Ed25519 public key: 32 bytes in base64");
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
