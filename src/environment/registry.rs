//! The server settings files in the data folder's .env/server (`~/.misty/dev/default/.env/server`
//! unless misty.conf names another folder) and the names each one owns. `misty.env` and
//! `mailjet.env` are also there, for a deployed server. The CLI leaves those
//! two alone, since the local server takes its address and origins from `runtime.env`.

#[derive(Clone, Copy)]
pub(super) struct FileSpec {
    pub(super) path: &'static str,
    pub(super) names: &'static [&'static str],
}

/// Files that belong in the folder and aren't the CLI's to read.
pub(super) const DEPLOYED_SERVER_FILES: [&str; 2] = ["misty.env", "mailjet.env"];

pub(super) const FILES: &[FileSpec] = &[
    FileSpec {
        path: "runtime.env",
        names: &[
            // Bearer token of the operator account API.
            "MISTY_OPERATOR_TOKEN",
            // Anyone may make an account.
            "MISTY_OPEN_SIGNUP",
            // What an account made through open signup may use, such as Misty Cloud's Free plan.
            "MISTY_SIGNUP_LIMITS",
            // Space levels raised by Boosts, Misty Cloud's; unset, Spaces are unlimited.
            "MISTY_SPACE_LEVELS",
            // The Google account that owns the server: it sees the server's alerts and is
            // always let in. Unset, the server has no owner; signing up first doesn't make one.
            "MISTY_OWNER_EMAIL",
            // Only these emails may make an account or sign in.
            "MISTY_ALLOWED_EMAILS",
            "MISTY_ALLOWED_ORIGINS",
            // Bytes the API serves a day, to one person and to everyone; unset, 25 GiB and
            // 200 GiB.
            "MISTY_EGRESS_MAX_BYTES_PER_IDENTITY_DAY",
            "MISTY_EGRESS_MAX_BYTES_PER_DAY",
            // The largest file each kind of upload takes, in bytes. Only lower than the
            // defaults: 100 MiB, 10 MiB, 15 MiB and 15 MiB.
            "MISTY_LIBRARY_MAX_FILE_BYTES",
            "MISTY_CHAT_ATTACHMENT_MAX_FILE_BYTES",
            "MISTY_NOTE_ATTACHMENT_MAX_FILE_BYTES",
            "MISTY_DRAWING_ASSET_MAX_FILE_BYTES",
            "MISTY_ENVIRONMENT",
            "MISTY_HOST_PORT",
            "MISTY_INSTANCE_NAME",
            "MISTY_PUBLIC_API_URL",
            // The website a development app links to (`misty desktop dev`).
            "MISTY_PUBLIC_URL",
            "MISTY_WEBSITE_URL",
            "PORT",
            "TRUST_PROXY_HEADERS",
            "TRUSTED_PROXY_CIDRS",
        ],
    },
    // Compose reads these where it runs, to choose images and tunnel routes; no container
    // gets this file. compose.dev.yml uses MISTY_API_IMAGE; compose.yml, a deployed
    // server's, uses all three.
    FileSpec {
        path: "compose.env",
        names: &[
            // A published API image to run instead of building this checkout, such as
            // ghcr.io/misty-org/misty-server-api:<commit SHA>.
            "MISTY_API_IMAGE",
            // The same for the tools image (setup, migrate, backups, alerts, console).
            "MISTY_TOOLS_IMAGE",
            // More hostnames the tunnel publishes: host=service, comma-separated.
            "MISTY_TUNNEL_EXTRA_INGRESS",
        ],
    },
    FileSpec {
        path: "database.env",
        names: &[
            "DB_HOST",
            "DB_MIGRATION_PASSWORD",
            "DB_MIGRATION_USER",
            "DB_NAME",
            "DB_PASSWORD",
            "DB_PORT",
            "DB_SSLMODE",
            "DB_MAX_OPEN_CONNS",
            "DB_MAX_IDLE_CONNS",
            "DB_CONN_MAX_LIFETIME",
            "DB_CONN_MAX_IDLE_TIME",
            "DB_USER",
        ],
    },
    FileSpec {
        path: "storage.env",
        names: &[
            "R2_ACCESS_KEY",
            "R2_BUCKET",
            "R2_ENDPOINT",
            "R2_SECRET_KEY",
            "MISTY_R2_ALLOWED_ORIGINS",
            // How long a signed upload and download link stays valid, as a Go duration
            // between 30s and 60m; unset, 15m and 2m.
            "MISTY_R2_UPLOAD_URL_TTL",
            "MISTY_R2_DOWNLOAD_URL_TTL",
        ],
    },
    FileSpec {
        path: "observability.env",
        names: &[
            "MISTY_METRICS_TOKEN",
            "MISTY_RELEASE_CHANNEL",
            "MISTY_SERVER_VERSION",
        ],
    },
    FileSpec {
        path: "cloudflare.env",
        names: &[
            "CLOUDFLARE_ACCOUNT_ID",
            "CLOUDFLARE_ZONE_ID",
            "MISTY_CLOUDFLARE_TUNNEL_NAME",
            "CLOUDFLARE_API_TOKEN",
            "CLOUDFLARE_TUNNEL_TOKEN",
            "MISTY_CLOUDFLARE_WORKER_HOST",
            "MISTY_CLOUDFLARE_WORKER_NAME",
            "MISTY_DEV_ALLOWED_ORIGINS",
            "MISTY_DEV_API_ORIGIN",
            "MISTY_DEV_API_TUNNEL_HOSTNAME",
            "PARTYKIT_HOST",
        ],
    },
    FileSpec {
        path: "google.env",
        names: &[
            "GOOGLE_CLIENT_ID",
            "GOOGLE_CLIENT_SECRET",
            "GOOGLE_SIGN_IN_REDIRECT_URL",
        ],
    },
    FileSpec {
        path: "crypto/documents.env",
        names: &["DOCUMENT_SIGNING_KEY"],
    },
    FileSpec {
        path: "crypto/journal.env",
        names: &[
            "JOURNAL_COLLAB_CONTROL_SECRET",
            "JOURNAL_COLLAB_CONTROL_SECRET_PREVIOUS",
            "JOURNAL_COLLAB_PROJECTION_SECRET",
            "JOURNAL_COLLAB_PROJECTION_SECRET_PREVIOUS",
            "JOURNAL_COLLAB_ROOM_SALT",
            "JOURNAL_COLLAB_TICKET_PRIVATE_KEY",
            "JOURNAL_COLLAB_TICKET_PUBLIC_KEY",
            // The public key of the ticket key before a rotation, which the Worker still
            // accepts until it is retired.
            "JOURNAL_COLLAB_TICKET_PUBLIC_KEY_PREVIOUS",
        ],
    },
    FileSpec {
        path: "crypto/spaces.env",
        names: &["SPACE_LINK_ENCRYPTION_KEY"],
    },
    FileSpec {
        path: "crypto/services.env",
        names: &["MISTY_AUTH_SIGNING_KEY", "MISTY_AUTH_SIGNING_KEY_PREVIOUS"],
    },
];

/// Names a production server refuses to start with (misty-server's
/// internal/app/production_environment.go): a Misty server holds no model keys or models.
pub(super) const RETIRED: &[&str] = &[
    "AI_GATEWAY_API_KEY",
    "OPENAI_API_KEY",
    "ANTHROPIC_API_KEY",
    "GEMINI_API_KEY",
    "OPENROUTER_API_KEY",
    "NVIDIA_API_KEY",
    "ELEVENLABS_API_KEY",
    "SONIOX_API_KEY",
    "OLLAMA_API_KEY",
    "OPENAI_COMPATIBLE_API_KEY",
    "VERCEL_OIDC_TOKEN",
    "MISTY_AGENT_MODEL",
    "MISTY_AGENT_RUNTIME_URL",
    "MISTY_AGENT_RUNTIME_CONTROL_SECRET",
    "MISTY_COMPOSIO_API_KEY",
];

pub(super) const DEV_REQUIRED: &[&str] = &[
    "CLOUDFLARE_API_TOKEN",
    "CLOUDFLARE_TUNNEL_TOKEN",
    "CLOUDFLARE_ACCOUNT_ID",
    "MISTY_DEV_API_ORIGIN",
    "MISTY_DEV_API_TUNNEL_HOSTNAME",
    "MISTY_CLOUDFLARE_WORKER_HOST",
];
