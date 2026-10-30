//! Application settings, sourced entirely from the process environment --
//! see docs/adrs/0001 (layering) and the port of template-fastapi's
//! `app/config.py`. No `.env` file is read here, matching `docs/TEMPLATE.md`'s
//! "Don't" section: every value below comes from the process environment,
//! which the compose files set directly.

use std::env;
use std::time::Duration;

/// Startup-time mode: controls which backend (real vs. in-memory/mock) every
/// downstream layer selects. Read once, at process startup -- never
/// per-request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// Local/devcontainer development: debug-friendly defaults.
    Dev,
    /// Every external service replaced by a local fake; requires
    /// `allow_mock_mode` -- see `Settings::from_env`.
    Mock,
    /// Real deployment: strict validation of every security-relevant field.
    Production,
}

impl Mode {
    fn parse(raw: &str) -> Result<Self, String> {
        match raw {
            "dev" => Ok(Mode::Dev),
            "mock" => Ok(Mode::Mock),
            "production" => Ok(Mode::Production),
            other => Err(format!(
                "MODE must be one of dev/mock/production, got {other:?}"
            )),
        }
    }
}

/// A libpq `sslmode`, as `POSTGRES_SSLMODE` spells it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PostgresSslMode {
    Disable,
    Allow,
    Prefer,
    Require,
    VerifyCa,
    VerifyFull,
}

impl PostgresSslMode {
    fn parse(raw: &str) -> Result<Self, String> {
        match raw {
            "disable" => Ok(PostgresSslMode::Disable),
            "allow" => Ok(PostgresSslMode::Allow),
            "prefer" => Ok(PostgresSslMode::Prefer),
            "require" => Ok(PostgresSslMode::Require),
            "verify-ca" => Ok(PostgresSslMode::VerifyCa),
            "verify-full" => Ok(PostgresSslMode::VerifyFull),
            other => Err(format!(
                "POSTGRES_SSLMODE must be one of disable/allow/prefer/require/verify-ca/\
                 verify-full, got {other:?}"
            )),
        }
    }

    /// Whether the mode refuses a plaintext connection (what
    /// `Mode::Production` requires).
    fn encrypts(self) -> bool {
        matches!(
            self,
            PostgresSslMode::Require | PostgresSslMode::VerifyCa | PostgresSslMode::VerifyFull
        )
    }
}

/// Database pool and deployment settings (`src/db.rs`,
/// `docs/adrs/0022`). Every sizing and timeout field is `None` when its
/// env var is unset, so `src/db.rs` passes only what is set and
/// sea-orm/sqlx's own defaults apply -- an unconfigured process behaves
/// exactly as it did before these settings existed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DbSettings {
    /// `POSTGRES_SSLMODE`: unset leaves whatever `DATABASE_URL` (or
    /// sqlx's `prefer`) says; set, it overrides it.
    pub sslmode: Option<PostgresSslMode>,
    /// `DB_POOL_SIZE`: the pool's maximum connections (sqlx has no
    /// separate overflow, so this is the whole budget per process).
    pub pool_size: Option<u32>,
    /// `DB_POOL_MIN_SIZE`: connections kept open while idle.
    pub pool_min_size: Option<u32>,
    /// `DB_POOL_RECYCLE`: seconds before a connection is closed and
    /// replaced (sqlx `max_lifetime`).
    pub pool_recycle: Option<Duration>,
    /// `DB_POOL_IDLE_TIMEOUT`: seconds an unused connection may idle
    /// before it is closed.
    pub pool_idle_timeout: Option<Duration>,
    /// `DB_CONNECT_TIMEOUT`: seconds a caller waits for a
    /// connection, covering both a free slot and opening a new one --
    /// sea-orm maps `connect_timeout` and `acquire_timeout` onto the same
    /// sqlx field, so this is the one timeout for both.
    pub connect_timeout: Option<Duration>,
    /// `RUN_MIGRATIONS_ON_STARTUP` (default true).
    pub run_migrations_on_startup: bool,
}

impl Default for DbSettings {
    fn default() -> Self {
        Self {
            sslmode: None,
            pool_size: None,
            pool_min_size: None,
            pool_recycle: None,
            pool_idle_timeout: None,
            connect_timeout: None,
            run_migrations_on_startup: true,
        }
    }
}

impl DbSettings {
    fn from_env() -> Result<Self, String> {
        let sslmode = match env::var("POSTGRES_SSLMODE") {
            Ok(raw) => Some(PostgresSslMode::parse(&raw)?),
            Err(_) => None,
        };
        let pool_size = env_positive("DB_POOL_SIZE")?;
        let pool_min_size = env_positive("DB_POOL_MIN_SIZE")?;
        if let (Some(min), Some(max)) = (pool_min_size, pool_size) {
            if min > max {
                return Err(format!(
                    "DB_POOL_MIN_SIZE ({min}) must not exceed DB_POOL_SIZE ({max})"
                ));
            }
        }
        Ok(Self {
            sslmode,
            pool_size,
            pool_min_size,
            pool_recycle: env_seconds("DB_POOL_RECYCLE")?,
            pool_idle_timeout: env_seconds("DB_POOL_IDLE_TIMEOUT")?,
            connect_timeout: env_seconds("DB_CONNECT_TIMEOUT")?,
            run_migrations_on_startup: env_bool("RUN_MIGRATIONS_ON_STARTUP", true)?,
        })
    }
}

/// Typed application settings, equivalent to `app.config.Settings`.
///
/// `app_name`/`oidc_authorization_url`/`oidc_token_url` are read (env-
/// sourced, validated at startup) but not yet consumed by any handler --
/// they mirror `config.py`'s fields 1:1 for a future Swagger/OpenAPI UI
/// and an interactive Authorization Code + PKCE login flow, neither of
/// which phase 2 builds (this backend only ever *validates* a bearer
/// token a frontend already obtained, per ADR 0003).
#[allow(dead_code)]
#[derive(Debug, Clone)]
pub struct Settings {
    pub app_name: String,
    pub mode: Mode,
    /// Second, explicitly-named gate on `Mode::Mock` -- see
    /// `_require_allow_mock_mode_flag` in the Python original: `MODE=mock`
    /// bypasses auth entirely, so reaching it requires this flag too, never
    /// `MODE`'s own default/typo alone.
    pub allow_mock_mode: bool,

    pub postgres_user: String,
    pub postgres_password: String,
    pub postgres_db: String,
    pub postgres_host: String,
    pub postgres_port: u16,
    /// Pool, TLS and migration settings for the Postgres connection.
    pub db: DbSettings,

    pub s3_endpoint_url: String,
    pub s3_access_key: String,
    pub s3_secret_key: String,

    pub redis_url: String,

    /// The MQTT broker backing the CRUD event stream (`craftainer_crud::events`,
    /// `docs/adrs/0016`). Host/port rather than a URL, matching rumqttc's
    /// own `MqttOptions::new(id, host, port)` shape -- there is no
    /// credential here to protect, so `validate()`'s production checks have
    /// nothing to add beyond what the stack's own network isolation gives.
    pub mqtt_host: String,
    pub mqtt_port: u16,

    /// Requests per 60s window a caller may make to `POST /mock/token`
    /// before `AppError::TooManyRequests` (`src/rate_limit.rs`) -- mirrors
    /// `config.py`'s `rate_limit_mock_token` ("10/minute"), as a bare count
    /// rather than a `"<count>/<period>"` expression since this app's
    /// hand-rolled limiter has no expression parser to reuse (see
    /// `docs/adrs/0011`).
    pub rate_limit_mock_token_per_minute: u32,
    /// Requests per 60s window a caller may make to a Hero
    /// create/update/delete route before the same error.
    pub rate_limit_hero_write_per_minute: u32,

    /// Refuse a bulk update/delete whose filters match more than this many
    /// records -- mirrors `config.py`'s `bulk_action_max_matched` (1000).
    /// See `docs/adrs/0013`.
    pub bulk_action_max_matched: u64,

    pub oidc_issuer_url: String,
    pub oidc_authorization_url: String,
    pub oidc_token_url: String,
    pub oidc_client_id: String,
    pub oidc_audience: Option<String>,
}

impl Settings {
    /// Build `Settings` from the process environment, then run every
    /// production-only validation -- returns `Err` (never panics itself) so
    /// the caller (`main`) decides how to fail fast.
    pub fn from_env() -> Result<Self, String> {
        let mode = Mode::parse(&env_or("MODE", "dev"))?;
        let settings = Settings {
            app_name: env_or("APP_NAME", "template-axum"),
            mode,
            allow_mock_mode: env_flag("ALLOW_MOCK_MODE"),

            postgres_user: env_or("POSTGRES_USER", "app"),
            postgres_password: env_or("POSTGRES_PASSWORD", "app"),
            postgres_db: env_or("POSTGRES_DB", "app"),
            postgres_host: env_or("POSTGRES_HOST", "localhost"),
            postgres_port: env_or("POSTGRES_PORT", "5432")
                .parse()
                .map_err(|_| "POSTGRES_PORT must be a valid port number".to_string())?,
            db: DbSettings::from_env()?,

            s3_endpoint_url: env_or("S3_ENDPOINT_URL", "http://localhost:9000"),
            s3_access_key: env::var("RUSTFS_ACCESS_KEY")
                .or_else(|_| env::var("S3_ACCESS_KEY"))
                .unwrap_or_else(|_| "rustfsadmin".to_string()),
            s3_secret_key: env::var("RUSTFS_SECRET_KEY")
                .or_else(|_| env::var("S3_SECRET_KEY"))
                .unwrap_or_else(|_| "rustfsadmin".to_string()),

            redis_url: env_or("REDIS_URL", "redis://localhost:6379/0"),

            mqtt_host: env_or("MQTT_HOST", "localhost"),
            mqtt_port: env_or("MQTT_PORT", "1883")
                .parse()
                .map_err(|_| "MQTT_PORT must be a valid port number".to_string())?,

            rate_limit_mock_token_per_minute: env_or("RATE_LIMIT_MOCK_TOKEN_PER_MINUTE", "10")
                .parse()
                .map_err(|_| {
                    "RATE_LIMIT_MOCK_TOKEN_PER_MINUTE must be a valid integer".to_string()
                })?,
            rate_limit_hero_write_per_minute: env_or("RATE_LIMIT_HERO_WRITE_PER_MINUTE", "20")
                .parse()
                .map_err(|_| {
                    "RATE_LIMIT_HERO_WRITE_PER_MINUTE must be a valid integer".to_string()
                })?,

            bulk_action_max_matched: env_or("BULK_ACTION_MAX_MATCHED", "1000")
                .parse()
                .map_err(|_| "BULK_ACTION_MAX_MATCHED must be a valid integer".to_string())?,

            oidc_issuer_url: env_or(
                "OIDC_ISSUER_URL",
                "http://localhost:8080/realms/template-fastapi",
            ),
            oidc_authorization_url: env_or(
                "OIDC_AUTHORIZATION_URL",
                "http://localhost:8080/realms/template-fastapi/protocol/openid-connect/auth",
            ),
            oidc_token_url: env_or(
                "OIDC_TOKEN_URL",
                "http://localhost:8080/realms/template-fastapi/protocol/openid-connect/token",
            ),
            oidc_client_id: env_or("OIDC_CLIENT_ID", "api"),
            // Always checked (RFC 9700 section 2.3): unset means `api`, and
            // `ResourceServer::from_env` rejects an empty value.
            oidc_audience: Some(env_or("OIDC_AUDIENCE", DEFAULT_AUDIENCE)),
        };

        settings.validate()?;
        // Fail fast on the resource-server settings (`PUBLIC_BASE_URL`,
        // `SECURITY_TXT_*`, `OIDC_ISSUER`, ...) too; the wiring reads them
        // again via `ResourceServer::from_env`.
        ResourceServer::from_env(&settings)?;
        Ok(settings)
    }

    /// The `sslmode` the connection will use: `POSTGRES_SSLMODE` when set
    /// (it overrides the URL, see `db.rs`), else a recognised `sslmode=`
    /// query parameter of `DATABASE_URL`.
    fn effective_sslmode(&self) -> Option<PostgresSslMode> {
        self.db.sslmode.or_else(|| {
            let url = self.database_url();
            let query = url.split_once('?')?.1;
            query
                .split('&')
                .find_map(|pair| pair.strip_prefix("sslmode="))
                .and_then(|raw| PostgresSslMode::parse(raw).ok())
        })
    }

    /// The composed async Postgres DSN SeaORM/sqlx connects with.
    pub fn database_url(&self) -> String {
        env::var("DATABASE_URL").unwrap_or_else(|_| {
            format!(
                "postgres://{}:{}@{}:{}/{}",
                self.postgres_user,
                self.postgres_password,
                self.postgres_host,
                self.postgres_port,
                self.postgres_db
            )
        })
    }

    /// Run every "fail fast on invalid production config" check --
    /// mirrors `config.py`'s `@model_validator(mode="after")` methods.
    fn validate(&self) -> Result<(), String> {
        if self.mode == Mode::Mock && !self.allow_mock_mode {
            return Err("MODE=mock requires ALLOW_MOCK_MODE=1 to be set".to_string());
        }

        if self.mode != Mode::Production {
            return Ok(());
        }

        let defaults: [(&str, &str, &str); 3] = [
            ("POSTGRES_PASSWORD", &self.postgres_password, "app"),
            (
                "RUSTFS_ACCESS_KEY/S3_ACCESS_KEY",
                &self.s3_access_key,
                "rustfsadmin",
            ),
            (
                "RUSTFS_SECRET_KEY/S3_SECRET_KEY",
                &self.s3_secret_key,
                "rustfsadmin",
            ),
        ];
        let left_at_default: Vec<&str> = defaults
            .iter()
            .filter(|(_, actual, default)| actual == default)
            .map(|(name, _, _)| *name)
            .collect();
        if !left_at_default.is_empty() {
            return Err(format!(
                "{} must not be left at their default value when MODE=production",
                left_at_default.join(", ")
            ));
        }

        let mut insecure = Vec::new();
        if !self
            .effective_sslmode()
            .is_some_and(PostgresSslMode::encrypts)
        {
            insecure.push("POSTGRES_SSLMODE must be require, verify-ca, or verify-full");
        }
        if !self.s3_endpoint_url.starts_with("https://") {
            insecure.push("S3_ENDPOINT_URL must use https://");
        }
        if !self.redis_url.starts_with("rediss://") {
            insecure.push("REDIS_URL must use rediss://");
        }
        if !env::var("OIDC_ISSUER")
            .unwrap_or_else(|_| self.oidc_issuer_url.clone())
            .starts_with("https://")
        {
            insecure.push("OIDC_ISSUER_URL/OIDC_ISSUER must use https://");
        }
        if !insecure.is_empty() {
            return Err(format!("{} when MODE=production", insecure.join("; ")));
        }

        Ok(())
    }
}

/// `GRACEFUL_SHUTDOWN_TIMEOUT_SECONDS`: how long in-flight requests get
/// after the shutdown signal before `main` stops waiting for them
/// (`docs/adrs/0027`). `None` (unset) waits without a bound, as
/// template-fastapi leaves uvicorn's default. Read by `main` next to
/// `Settings::from_env` rather than held in `Settings`: it is a process
/// concern no request handler reads. An unparsable value is an error, like
/// every other setting.
pub fn graceful_shutdown_timeout() -> Result<Option<Duration>, String> {
    env_seconds("GRACEFUL_SHUTDOWN_TIMEOUT_SECONDS")
}

/// `STATS_CACHE_TTL_SECONDS`: how long a cached `/stats` or `/predict`
/// response lives (`docs/adrs/0024`). Unset (the default) turns the shared
/// cache off; a set value must be a whole number >= 1, as in
/// template-fastapi. Standalone, like `graceful_shutdown_timeout`: only the
/// wiring reads it, no request handler.
pub fn stats_cache_ttl() -> Result<Option<Duration>, String> {
    env_seconds("STATS_CACHE_TTL_SECONDS")
}

/// `SUBSCRIBER_SECRET`: the key `craftainer_crud`'s event bus signs the
/// subscriber ids it mints with (`docs/adrs/0020`). Mirrors
/// template-fastapi's `subscriber_secret`: unset is allowed (the bus then
/// uses a random per-process key, so ids survive neither a restart nor a
/// second replica), a set value must be at least 32 characters, and
/// `Mode::Production` refuses to start without one. Read by `main` (to fail
/// fast) and `build_state_with_db` rather than held in `Settings`, so the
/// secret is never part of a `Debug`-printable struct. An error message
/// never contains the value.
pub fn subscriber_secret(mode: Mode) -> Result<Option<String>, String> {
    match env::var("SUBSCRIBER_SECRET") {
        Ok(secret) if secret.chars().count() >= MIN_SUBSCRIBER_SECRET_CHARS => Ok(Some(secret)),
        Ok(_) => Err(format!(
            "SUBSCRIBER_SECRET must be at least {MIN_SUBSCRIBER_SECRET_CHARS} characters"
        )),
        Err(env::VarError::NotUnicode(_)) => {
            Err("SUBSCRIBER_SECRET must be valid UTF-8".to_string())
        }
        Err(env::VarError::NotPresent) if mode == Mode::Production => {
            Err("SUBSCRIBER_SECRET must be set when MODE=production".to_string())
        }
        Err(env::VarError::NotPresent) => Ok(None),
    }
}

/// The shortest `SUBSCRIBER_SECRET`, in characters (`config.py`'s
/// `min_length`); also at least `craftainer_crud`'s 32-byte minimum.
const MIN_SUBSCRIBER_SECRET_CHARS: usize = 32;

/// The audience a bearer token must carry when `OIDC_AUDIENCE` is unset;
/// the bundled realm's `api` and `bff` clients both add it.
const DEFAULT_AUDIENCE: &str = "api";

/// Longest `Expires` a `security.txt` may carry (RFC 9116 section 2.5.5
/// advises under a year; a leap year's 366 days is the accepted bound).
const SECURITY_TXT_MAX_AHEAD_DAYS: i64 = 366;

/// An RFC 9116 `security.txt` (`SECURITY_TXT_*`), served at
/// `/.well-known/security.txt` only when a contact is configured.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SecurityTxt {
    pub contact: String,
    pub expires: chrono::DateTime<chrono::Utc>,
    pub policy: Option<String>,
    pub preferred_languages: Option<String>,
    pub canonical: Option<String>,
}

/// What this service is as an OAuth 2.0 resource server (RFC 9728): which
/// issuer's tokens it accepts, for which audience, where it lives publicly
/// and how to report a vulnerability. Read from the environment next to
/// `Settings` (not inside it) by `ResourceServer::from_env`.
///
/// The expected issuer (the `iss` a token must carry, and the RFC 9728
/// authorization server) is a different thing from the URL this service
/// reaches the provider at: with Keycloak's `KC_HOSTNAME` set, tokens
/// carry the public `http://localhost:8080/realms/...` while discovery is
/// fetched from `http://keycloak:8080/...` inside the compose network.
/// `OIDC_ISSUER` is the former, `OIDC_DISCOVERY_URL` (a full
/// `.../.well-known/openid-configuration` URL) the latter; each falls back
/// to `OIDC_ISSUER_URL` (the original single variable), so an instance
/// that sets only that behaves as before.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResourceServer {
    /// Resource name advertised in the RFC 9728 document (`APP_NAME`).
    pub name: String,
    /// The `iss` a token must carry, compared as an exact string.
    pub issuer: String,
    /// Where the provider's discovery document is fetched (may be internal).
    pub discovery_url: String,
    /// The `aud` a token must carry. Never optional.
    pub audience: String,
    /// `scopes_supported` in the RFC 9728 document (`OIDC_SCOPES_SUPPORTED`,
    /// a JSON list). The API authorizes by role, not scope.
    pub scopes_supported: Vec<String>,
    /// This service's public origin without a trailing slash: the RFC 9728
    /// `resource` and the base of `resource_metadata`. From configuration,
    /// never from the request's `Host` header, which a client controls.
    pub public_base_url: String,
    pub security_txt: Option<SecurityTxt>,
}

impl ResourceServer {
    /// Origin used when `PUBLIC_BASE_URL` is unset (anything but production).
    pub const DEFAULT_PUBLIC_BASE_URL: &'static str = "http://localhost:8000";

    /// Defaults derived from `settings` alone: no resource-server variable
    /// is read, so the result cannot depend on the process environment.
    pub fn from_settings(settings: &Settings) -> Self {
        Self {
            name: settings.app_name.clone(),
            issuer: settings.oidc_issuer_url.clone(),
            discovery_url: discovery_url_for(&settings.oidc_issuer_url),
            audience: settings
                .oidc_audience
                .clone()
                .unwrap_or_else(|| DEFAULT_AUDIENCE.to_string()),
            scopes_supported: vec!["openid".to_string()],
            public_base_url: Self::DEFAULT_PUBLIC_BASE_URL.to_string(),
            security_txt: None,
        }
    }

    /// Read and validate the resource-server settings from the process
    /// environment (`settings` supplies the legacy `OIDC_ISSUER_URL`).
    pub fn from_env(settings: &Settings) -> Result<Self, String> {
        Self::from_lookup(settings, |key| env::var(key).ok())
    }

    /// `from_env` over an arbitrary variable source, so tests don't touch
    /// the process environment.
    pub fn from_lookup(
        settings: &Settings,
        get: impl Fn(&str) -> Option<String>,
    ) -> Result<Self, String> {
        let production = settings.mode == Mode::Production;
        let mut problems: Vec<String> = Vec::new();

        let issuer = get("OIDC_ISSUER").unwrap_or_else(|| settings.oidc_issuer_url.clone());
        let discovery_url = get("OIDC_DISCOVERY_URL").unwrap_or_else(|| discovery_url_for(&issuer));
        if production {
            if !issuer.starts_with("https://") {
                problems.push("OIDC_ISSUER_URL/OIDC_ISSUER must use https://".to_string());
            }
            if !discovery_url.starts_with("https://") {
                problems.push("OIDC_DISCOVERY_URL must use https://".to_string());
            }
        }

        let audience = get("OIDC_AUDIENCE")
            .or_else(|| settings.oidc_audience.clone())
            .unwrap_or_else(|| DEFAULT_AUDIENCE.to_string());
        if audience.is_empty() {
            problems.push("OIDC_AUDIENCE must not be empty".to_string());
        }

        let scopes_supported = match get("OIDC_SCOPES_SUPPORTED") {
            None => vec!["openid".to_string()],
            Some(raw) => serde_json::from_str::<Vec<String>>(&raw).unwrap_or_else(|_| {
                problems.push("OIDC_SCOPES_SUPPORTED must be a JSON list of strings".to_string());
                Vec::new()
            }),
        };

        let public_base_url = get("PUBLIC_BASE_URL");
        if let Some(url) = &public_base_url {
            if !is_origin(url) {
                problems.push("PUBLIC_BASE_URL must be an http(s) origin with no path".to_string());
            }
        }
        if production {
            match &public_base_url {
                None => problems.push("PUBLIC_BASE_URL must be set when MODE=production".to_string()),
                Some(url) if !url.starts_with("https://") => {
                    problems.push("PUBLIC_BASE_URL must use https:// when MODE=production".to_string());
                }
                Some(_) => {}
            }
        }

        let security_txt = match SecurityTxt::from_lookup(&get, chrono::Utc::now()) {
            Ok(file) => file,
            Err(mut found) => {
                problems.append(&mut found);
                None
            }
        };

        if !problems.is_empty() {
            return Err(problems.join("; "));
        }
        Ok(Self {
            name: settings.app_name.clone(),
            issuer,
            discovery_url,
            audience,
            scopes_supported,
            public_base_url: public_base_url
                .unwrap_or_else(|| Self::DEFAULT_PUBLIC_BASE_URL.to_string())
                .trim_end_matches('/')
                .to_string(),
            security_txt,
        })
    }

    /// The public issuer's discovery URL (what a client can reach), as the
    /// manifest advertises it; `discovery_url` is where *this* service
    /// fetches it, which may be a compose-internal host.
    pub fn public_discovery_url(&self) -> String {
        discovery_url_for(&self.issuer)
    }

    /// Absolute URL of the RFC 9728 protected-resource metadata document.
    pub fn resource_metadata_url(&self) -> String {
        format!("{}/.well-known/oauth-protected-resource", self.public_base_url)
    }
}

impl SecurityTxt {
    /// `None` when `SECURITY_TXT_CONTACT` is unset; with a contact set,
    /// `SECURITY_TXT_EXPIRES` is required (RFC 3339 with a zone, in the
    /// future and at most a year ahead: an RFC 9116 file without `Expires`
    /// is not compliant, and one expiring far out defeats the field).
    /// Values go into a text file line by line, so control characters and
    /// blanks are rejected.
    fn from_lookup(
        get: &impl Fn(&str) -> Option<String>,
        now: chrono::DateTime<chrono::Utc>,
    ) -> Result<Option<Self>, Vec<String>> {
        let contact = get("SECURITY_TXT_CONTACT");
        let Some(contact) = contact else {
            return Ok(None);
        };
        let policy = get("SECURITY_TXT_POLICY");
        let canonical = get("SECURITY_TXT_CANONICAL");
        let preferred_languages = get("SECURITY_TXT_PREFERRED_LANGUAGES");

        let mut problems = Vec::new();
        for (key, value) in [
            ("SECURITY_TXT_CONTACT", Some(&contact)),
            ("SECURITY_TXT_POLICY", policy.as_ref()),
            ("SECURITY_TXT_CANONICAL", canonical.as_ref()),
            ("SECURITY_TXT_PREFERRED_LANGUAGES", preferred_languages.as_ref()),
        ] {
            if value.is_some_and(|v| v.trim().is_empty() || v.chars().any(char::is_control)) {
                problems.push(format!("{key} must be a single line of text"));
            }
        }
        if !["https://", "mailto:", "tel:"]
            .iter()
            .any(|scheme| contact.starts_with(scheme))
        {
            problems.push("SECURITY_TXT_CONTACT must be an https:, mailto: or tel: URI".to_string());
        }
        if [&policy, &canonical]
            .iter()
            .any(|v| v.as_ref().is_some_and(|v| !v.starts_with("https://")))
        {
            problems.push(
                "SECURITY_TXT_POLICY and SECURITY_TXT_CANONICAL must be https:// URLs".to_string(),
            );
        }

        let expires = match get("SECURITY_TXT_EXPIRES") {
            None => {
                problems.push("SECURITY_TXT_EXPIRES must be set when SECURITY_TXT_CONTACT is".to_string());
                None
            }
            Some(raw) => match chrono::DateTime::parse_from_rfc3339(&raw) {
                Err(_) => {
                    problems.push(
                        "SECURITY_TXT_EXPIRES must be an RFC 3339 timestamp with a time zone"
                            .to_string(),
                    );
                    None
                }
                Ok(at) => {
                    let at = at.with_timezone(&chrono::Utc);
                    if at <= now || at > now + chrono::Duration::days(SECURITY_TXT_MAX_AHEAD_DAYS) {
                        problems.push(
                            "SECURITY_TXT_EXPIRES must be in the future and at most one year ahead"
                                .to_string(),
                        );
                    }
                    Some(at)
                }
            },
        };

        match expires {
            Some(expires) if problems.is_empty() => Ok(Some(Self {
                contact,
                expires,
                policy,
                preferred_languages,
                canonical,
            })),
            _ => Err(problems),
        }
    }

    /// The RFC 9116 file body.
    pub fn render(&self) -> String {
        let mut lines = vec![
            format!("Contact: {}", self.contact),
            format!("Expires: {}", self.expires.format("%Y-%m-%dT%H:%M:%SZ")),
        ];
        for (field, value) in [
            ("Preferred-Languages", &self.preferred_languages),
            ("Policy", &self.policy),
            ("Canonical", &self.canonical),
        ] {
            if let Some(value) = value {
                lines.push(format!("{field}: {value}"));
            }
        }
        lines.join("\n") + "\n"
    }
}

fn discovery_url_for(issuer: &str) -> String {
    format!("{}/.well-known/openid-configuration", issuer.trim_end_matches('/'))
}

/// `scheme://host[:port]` with at most one trailing slash and nothing else.
fn is_origin(url: &str) -> bool {
    let Some(rest) = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))
    else {
        return false;
    };
    let host = rest.strip_suffix('/').unwrap_or(rest);
    !host.is_empty() && !host.contains(['/', '?', '#', '@', ' ']) && host.chars().all(|c| !c.is_control())
}

fn env_or(key: &str, default: &str) -> String {
    env::var(key).unwrap_or_else(|_| default.to_string())
}

/// An optional integer >= 1 (`None` when unset).
fn env_positive(key: &str) -> Result<Option<u32>, String> {
    match env::var(key) {
        Err(_) => Ok(None),
        Ok(raw) => match raw.parse::<u32>() {
            Ok(value) if value >= 1 => Ok(Some(value)),
            _ => Err(format!("{key} must be a whole number >= 1, got {raw:?}")),
        },
    }
}

/// An optional whole number of seconds >= 1 (`None` when unset).
fn env_seconds(key: &str) -> Result<Option<Duration>, String> {
    Ok(env_positive(key)?.map(|secs| Duration::from_secs(u64::from(secs))))
}

/// A boolean with a default when unset; `1`/`true`/`True` and `0`/`false`/
/// `False` are accepted, anything else is rejected (a typo must not
/// silently flip a deployment switch).
fn env_bool(key: &str, default: bool) -> Result<bool, String> {
    match env::var(key).as_deref() {
        Err(_) => Ok(default),
        Ok("1" | "true" | "True") => Ok(true),
        Ok("0" | "false" | "False") => Ok(false),
        Ok(other) => Err(format!("{key} must be true or false, got {other:?}")),
    }
}

fn env_flag(key: &str) -> bool {
    matches!(env::var(key).as_deref(), Ok("1") | Ok("true") | Ok("True"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    // Settings::from_env reads ambient process env vars, which Rust's test
    // harness runs in parallel by default -- serialize these tests so one
    // doesn't observe another's env::set_var/remove_var.
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    fn clear_all() {
        for key in [
            "MODE",
            "ALLOW_MOCK_MODE",
            // Every POSTGRES_*/MQTT_* key, not just the ones a test sets:
            // the devcontainer exports POSTGRES_HOST=postgres and
            // MQTT_HOST=mqtt into the process these tests run in, so a
            // test asserting on a *default* has to clear the ambient value
            // first or it reads the stack's, not the default.
            "POSTGRES_USER",
            "POSTGRES_PASSWORD",
            "POSTGRES_DB",
            "POSTGRES_HOST",
            "POSTGRES_PORT",
            "MQTT_HOST",
            "MQTT_PORT",
            "RUSTFS_ACCESS_KEY",
            "S3_ACCESS_KEY",
            "RUSTFS_SECRET_KEY",
            "S3_SECRET_KEY",
            "S3_ENDPOINT_URL",
            "REDIS_URL",
            "RATE_LIMIT_MOCK_TOKEN_PER_MINUTE",
            "RATE_LIMIT_HERO_WRITE_PER_MINUTE",
            "BULK_ACTION_MAX_MATCHED",
            "OIDC_ISSUER_URL",
            "OIDC_ISSUER",
            "OIDC_DISCOVERY_URL",
            "OIDC_AUDIENCE",
            "OIDC_SCOPES_SUPPORTED",
            "PUBLIC_BASE_URL",
            "SECURITY_TXT_CONTACT",
            "SECURITY_TXT_EXPIRES",
            "SECURITY_TXT_POLICY",
            "SECURITY_TXT_CANONICAL",
            "SECURITY_TXT_PREFERRED_LANGUAGES",
            "DATABASE_URL",
            "POSTGRES_SSLMODE",
            "DB_POOL_SIZE",
            "DB_POOL_MIN_SIZE",
            "DB_POOL_RECYCLE",
            "DB_POOL_IDLE_TIMEOUT",
            "DB_CONNECT_TIMEOUT",
            "RUN_MIGRATIONS_ON_STARTUP",
            "GRACEFUL_SHUTDOWN_TIMEOUT_SECONDS",
            "SUBSCRIBER_SECRET",
        ] {
            unsafe { env::remove_var(key) };
        }
    }

    #[test]
    fn graceful_shutdown_timeout_is_unset_by_default_and_must_be_a_positive_whole_number() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        // SAFETY: serialized by ENV_LOCK, like `clear_all`.
        let key = "GRACEFUL_SHUTDOWN_TIMEOUT_SECONDS";
        unsafe {
            env::remove_var(key);
        }
        assert_eq!(graceful_shutdown_timeout(), Ok(None));
        unsafe {
            env::set_var(key, "10");
        }
        assert_eq!(
            graceful_shutdown_timeout(),
            Ok(Some(Duration::from_secs(10)))
        );
        for bad in ["0", "-1", "1.5", "soon", ""] {
            unsafe {
                env::set_var(key, bad);
            }
            assert!(
                graceful_shutdown_timeout().is_err(),
                "{bad:?} must be rejected"
            );
        }
        unsafe {
            env::remove_var(key);
        }
    }

    #[test]
    fn stats_cache_ttl_is_off_by_default_and_must_be_a_positive_whole_number() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let key = "STATS_CACHE_TTL_SECONDS";
        // SAFETY: serialized by ENV_LOCK, like `clear_all`.
        unsafe { env::remove_var(key) };
        assert_eq!(stats_cache_ttl(), Ok(None));
        unsafe { env::set_var(key, "30") };
        assert_eq!(stats_cache_ttl(), Ok(Some(Duration::from_secs(30))));
        for bad in ["0", "-1", "1.5", "soon", ""] {
            unsafe { env::set_var(key, bad) };
            assert!(stats_cache_ttl().is_err(), "{bad:?} must be rejected");
        }
        unsafe { env::remove_var(key) };
    }

    #[test]
    fn subscriber_secret_is_optional_outside_production_but_must_be_32_characters() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        clear_all();
        for mode in [Mode::Dev, Mode::Mock] {
            assert_eq!(subscriber_secret(mode), Ok(None));
        }
        let good = "k".repeat(32);
        unsafe { env::set_var("SUBSCRIBER_SECRET", &good) };
        assert_eq!(subscriber_secret(Mode::Dev), Ok(Some(good)));
        for bad in ["", "short", &"k".repeat(31)] {
            unsafe { env::set_var("SUBSCRIBER_SECRET", bad) };
            let err = subscriber_secret(Mode::Dev).expect_err("a short secret must be rejected");
            assert!(err.contains("SUBSCRIBER_SECRET"), "{err}");
            if !bad.is_empty() {
                assert!(!err.contains(bad), "the error must not echo the secret");
            }
        }
        clear_all();
    }

    #[test]
    fn production_requires_a_subscriber_secret() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        clear_all();
        let err = subscriber_secret(Mode::Production).expect_err("unset must fail in production");
        assert_eq!(err, "SUBSCRIBER_SECRET must be set when MODE=production");
        let good = "p".repeat(40);
        unsafe { env::set_var("SUBSCRIBER_SECRET", &good) };
        assert_eq!(subscriber_secret(Mode::Production), Ok(Some(good)));
        clear_all();
    }

    #[test]
    fn defaults_to_dev_mode() {
        let _guard = ENV_LOCK.lock().unwrap();
        clear_all();
        let settings = Settings::from_env().expect("dev defaults must validate");
        assert_eq!(settings.mode, Mode::Dev);
        assert!(settings
            .database_url()
            .starts_with("postgres://app:app@localhost"));
    }

    #[test]
    fn rejects_an_unrecognized_mode() {
        let _guard = ENV_LOCK.lock().unwrap();
        clear_all();
        unsafe { env::set_var("MODE", "bogus") };
        let err = Settings::from_env().expect_err("an unrecognized MODE must be rejected");
        assert!(err.contains("bogus"));
        clear_all();
    }

    #[test]
    fn rejects_a_non_numeric_rate_limit() {
        let _guard = ENV_LOCK.lock().unwrap();
        clear_all();
        unsafe { env::set_var("RATE_LIMIT_MOCK_TOKEN_PER_MINUTE", "not-a-number") };
        let err = Settings::from_env().expect_err("a non-numeric rate limit must be rejected");
        assert!(err.contains("RATE_LIMIT_MOCK_TOKEN_PER_MINUTE"));
        clear_all();

        unsafe { env::set_var("RATE_LIMIT_HERO_WRITE_PER_MINUTE", "not-a-number") };
        let err = Settings::from_env().expect_err("a non-numeric rate limit must be rejected");
        assert!(err.contains("RATE_LIMIT_HERO_WRITE_PER_MINUTE"));
        clear_all();
    }

    #[test]
    fn mock_mode_requires_allow_mock_mode() {
        let _guard = ENV_LOCK.lock().unwrap();
        clear_all();
        unsafe { env::set_var("MODE", "mock") };
        assert!(Settings::from_env().is_err());
        unsafe { env::set_var("ALLOW_MOCK_MODE", "1") };
        assert!(Settings::from_env().is_ok());
        clear_all();
    }

    #[test]
    fn production_rejects_default_credentials() {
        let _guard = ENV_LOCK.lock().unwrap();
        clear_all();
        unsafe { env::set_var("MODE", "production") };
        let err = Settings::from_env().expect_err("default credentials must be rejected");
        assert!(err.contains("POSTGRES_PASSWORD"));
        clear_all();
    }

    #[test]
    fn production_rejects_insecure_urls_despite_hardened_credentials() {
        let _guard = ENV_LOCK.lock().unwrap();
        clear_all();
        unsafe { env::set_var("MODE", "production") };
        unsafe { env::set_var("OIDC_AUDIENCE", "api") };
        unsafe { env::set_var("POSTGRES_PASSWORD", "a-real-secret") };
        unsafe { env::set_var("RUSTFS_ACCESS_KEY", "a-real-key") };
        unsafe { env::set_var("RUSTFS_SECRET_KEY", "a-real-secret-key") };
        // S3_ENDPOINT_URL/REDIS_URL/OIDC_ISSUER_URL/POSTGRES_SSLMODE left at
        // their plaintext defaults -- all four must be flagged.
        let err = Settings::from_env().expect_err("plaintext URLs must be rejected");
        assert!(err.contains("POSTGRES_SSLMODE"));
        assert!(err.contains("S3_ENDPOINT_URL"));
        assert!(err.contains("REDIS_URL"));
        assert!(err.contains("OIDC_ISSUER_URL"));
        clear_all();
    }

    #[test]
    fn production_accepts_hardened_config() {
        let _guard = ENV_LOCK.lock().unwrap();
        clear_all();
        unsafe { env::set_var("MODE", "production") };
        unsafe { env::set_var("OIDC_AUDIENCE", "api") };
        unsafe { env::set_var("POSTGRES_PASSWORD", "a-real-secret") };
        unsafe { env::set_var("RUSTFS_ACCESS_KEY", "a-real-key") };
        unsafe { env::set_var("RUSTFS_SECRET_KEY", "a-real-secret-key") };
        unsafe { env::set_var("S3_ENDPOINT_URL", "https://s3.example.com") };
        unsafe { env::set_var("REDIS_URL", "rediss://redis.example.com:6379/0") };
        unsafe { env::set_var("OIDC_ISSUER_URL", "https://issuer.example.com/realms/api") };
        unsafe { env::set_var("PUBLIC_BASE_URL", "https://api.example.com") };
        unsafe { env::set_var("POSTGRES_SSLMODE", "verify-full") };
        assert!(Settings::from_env().is_ok());
        clear_all();
    }

    #[test]
    fn production_rejects_a_non_encrypting_sslmode() {
        let _guard = ENV_LOCK.lock().unwrap();
        clear_all();
        unsafe { env::set_var("MODE", "production") };
        unsafe { env::set_var("OIDC_AUDIENCE", "api") };
        unsafe { env::set_var("POSTGRES_PASSWORD", "a-real-secret") };
        unsafe { env::set_var("RUSTFS_ACCESS_KEY", "a-real-key") };
        unsafe { env::set_var("RUSTFS_SECRET_KEY", "a-real-secret-key") };
        unsafe { env::set_var("S3_ENDPOINT_URL", "https://s3.example.com") };
        unsafe { env::set_var("REDIS_URL", "rediss://redis.example.com:6379/0") };
        unsafe { env::set_var("OIDC_ISSUER_URL", "https://issuer.example.com/realms/api") };
        unsafe { env::set_var("PUBLIC_BASE_URL", "https://api.example.com") };
        for weak in ["disable", "allow", "prefer"] {
            unsafe { env::set_var("POSTGRES_SSLMODE", weak) };
            let err = Settings::from_env().expect_err("a weak sslmode must be rejected");
            assert!(err.contains("POSTGRES_SSLMODE"), "{weak}: {err}");
        }
        for strong in ["require", "verify-ca", "verify-full"] {
            unsafe { env::set_var("POSTGRES_SSLMODE", strong) };
            assert!(Settings::from_env().is_ok(), "{strong}");
        }
        clear_all();
    }

    #[test]
    fn production_accepts_an_encrypting_sslmode_from_database_url() {
        let _guard = ENV_LOCK.lock().unwrap();
        clear_all();
        unsafe { env::set_var("MODE", "production") };
        unsafe { env::set_var("OIDC_AUDIENCE", "api") };
        unsafe { env::set_var("POSTGRES_PASSWORD", "a-real-secret") };
        unsafe { env::set_var("RUSTFS_ACCESS_KEY", "a-real-key") };
        unsafe { env::set_var("RUSTFS_SECRET_KEY", "a-real-secret-key") };
        unsafe { env::set_var("S3_ENDPOINT_URL", "https://s3.example.com") };
        unsafe { env::set_var("REDIS_URL", "rediss://redis.example.com:6379/0") };
        unsafe { env::set_var("OIDC_ISSUER_URL", "https://issuer.example.com/realms/api") };
        unsafe { env::set_var("PUBLIC_BASE_URL", "https://api.example.com") };
        unsafe {
            env::set_var(
                "DATABASE_URL",
                "postgres://u:p@h/d?application_name=x&sslmode=verify-full",
            )
        };
        assert!(Settings::from_env().is_ok());
        unsafe { env::set_var("DATABASE_URL", "postgres://u:p@h/d?sslmode=prefer") };
        assert!(Settings::from_env().is_err());
        unsafe { env::set_var("DATABASE_URL", "postgres://u:p@h/d") };
        assert!(Settings::from_env().is_err());
        clear_all();
    }

    #[test]
    fn db_settings_default_to_todays_behaviour() {
        let _guard = ENV_LOCK.lock().unwrap();
        clear_all();
        let settings = Settings::from_env().unwrap();
        assert_eq!(settings.db, DbSettings::default());
        assert_eq!(settings.db.sslmode, None);
        assert_eq!(settings.db.pool_size, None);
        assert_eq!(settings.db.connect_timeout, None);
        assert!(settings.db.run_migrations_on_startup);
    }

    #[test]
    fn db_settings_are_read_from_the_environment() {
        let _guard = ENV_LOCK.lock().unwrap();
        clear_all();
        unsafe { env::set_var("POSTGRES_SSLMODE", "verify-ca") };
        unsafe { env::set_var("DB_POOL_SIZE", "8") };
        unsafe { env::set_var("DB_POOL_MIN_SIZE", "2") };
        unsafe { env::set_var("DB_POOL_RECYCLE", "1800") };
        unsafe { env::set_var("DB_POOL_IDLE_TIMEOUT", "300") };
        unsafe { env::set_var("DB_CONNECT_TIMEOUT", "5") };
        unsafe { env::set_var("RUN_MIGRATIONS_ON_STARTUP", "false") };
        let db = Settings::from_env().unwrap().db;
        assert_eq!(db.sslmode, Some(PostgresSslMode::VerifyCa));
        assert_eq!(db.pool_size, Some(8));
        assert_eq!(db.pool_min_size, Some(2));
        assert_eq!(db.pool_recycle, Some(Duration::from_secs(1800)));
        assert_eq!(db.pool_idle_timeout, Some(Duration::from_secs(300)));
        assert_eq!(db.connect_timeout, Some(Duration::from_secs(5)));
        assert!(!db.run_migrations_on_startup);

        unsafe { env::set_var("RUN_MIGRATIONS_ON_STARTUP", "True") };
        assert!(Settings::from_env().unwrap().db.run_migrations_on_startup);
        unsafe { env::set_var("RUN_MIGRATIONS_ON_STARTUP", "0") };
        assert!(!Settings::from_env().unwrap().db.run_migrations_on_startup);
        clear_all();
    }

    #[test]
    fn every_sslmode_spelling_parses() {
        let _guard = ENV_LOCK.lock().unwrap();
        clear_all();
        for (raw, expected) in [
            ("disable", PostgresSslMode::Disable),
            ("allow", PostgresSslMode::Allow),
            ("prefer", PostgresSslMode::Prefer),
            ("require", PostgresSslMode::Require),
            ("verify-ca", PostgresSslMode::VerifyCa),
            ("verify-full", PostgresSslMode::VerifyFull),
        ] {
            unsafe { env::set_var("POSTGRES_SSLMODE", raw) };
            assert_eq!(Settings::from_env().unwrap().db.sslmode, Some(expected));
        }
        clear_all();
    }

    #[test]
    fn rejects_invalid_db_settings() {
        let _guard = ENV_LOCK.lock().unwrap();
        for (key, value) in [
            ("POSTGRES_SSLMODE", "yes"),
            ("DB_POOL_SIZE", "0"),
            ("DB_POOL_SIZE", "-1"),
            ("DB_POOL_SIZE", "many"),
            ("DB_POOL_MIN_SIZE", "0"),
            ("DB_POOL_RECYCLE", "0"),
            ("DB_POOL_IDLE_TIMEOUT", "1.5"),
            ("DB_CONNECT_TIMEOUT", "soon"),
            ("RUN_MIGRATIONS_ON_STARTUP", "maybe"),
        ] {
            clear_all();
            unsafe { env::set_var(key, value) };
            let err = Settings::from_env().expect_err(&format!("{key}={value} must be rejected"));
            assert!(err.contains(key), "{key}={value}: {err}");
        }
        clear_all();
    }

    #[test]
    fn rejects_a_min_pool_larger_than_the_max() {
        let _guard = ENV_LOCK.lock().unwrap();
        clear_all();
        unsafe { env::set_var("DB_POOL_SIZE", "4") };
        unsafe { env::set_var("DB_POOL_MIN_SIZE", "5") };
        let err = Settings::from_env().expect_err("min > max must be rejected");
        assert!(err.contains("DB_POOL_MIN_SIZE"));
        unsafe { env::set_var("DB_POOL_MIN_SIZE", "4") };
        assert!(Settings::from_env().is_ok());
        clear_all();
    }

    // -- ResourceServer: read through `from_lookup` over a map, so the
    // devcontainer's own OIDC_*/PUBLIC_BASE_URL exports never reach them.

    fn settings_in(mode: Mode) -> Settings {
        Settings {
            app_name: "template-axum".to_string(),
            mode,
            allow_mock_mode: true,
            postgres_user: "app".to_string(),
            postgres_password: "app".to_string(),
            postgres_db: "app".to_string(),
            postgres_host: "localhost".to_string(),
            postgres_port: 5432,
            db: Default::default(),
            s3_endpoint_url: "http://localhost:9000".to_string(),
            s3_access_key: "rustfsadmin".to_string(),
            s3_secret_key: "rustfsadmin".to_string(),
            redis_url: "redis://localhost:6379/0".to_string(),
            mqtt_host: "localhost".to_string(),
            mqtt_port: 1883,
            rate_limit_mock_token_per_minute: 10,
            rate_limit_hero_write_per_minute: 20,
            bulk_action_max_matched: 1000,
            oidc_issuer_url: "http://keycloak:8080/realms/r".to_string(),
            oidc_authorization_url: "http://localhost:8080/auth".to_string(),
            oidc_token_url: "http://localhost:8080/token".to_string(),
            oidc_client_id: "api".to_string(),
            oidc_audience: None,
        }
    }

    fn resource_server(
        mode: Mode,
        vars: &[(&str, &str)],
    ) -> Result<ResourceServer, String> {
        let vars: std::collections::HashMap<String, String> = vars
            .iter()
            .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
            .collect();
        ResourceServer::from_lookup(&settings_in(mode), |key| vars.get(key).cloned())
    }

    fn in_days(days: i64) -> String {
        (chrono::Utc::now() + chrono::Duration::days(days)).to_rfc3339()
    }

    #[test]
    fn resource_server_defaults_fall_back_to_the_legacy_issuer_url() {
        let rs = resource_server(Mode::Dev, &[]).unwrap();
        assert_eq!(rs.issuer, "http://keycloak:8080/realms/r");
        assert_eq!(
            rs.discovery_url,
            "http://keycloak:8080/realms/r/.well-known/openid-configuration"
        );
        assert_eq!(rs.audience, "api");
        assert_eq!(rs.scopes_supported, ["openid"]);
        assert_eq!(rs.public_base_url, "http://localhost:8000");
        assert_eq!(
            rs.resource_metadata_url(),
            "http://localhost:8000/.well-known/oauth-protected-resource"
        );
        assert!(rs.security_txt.is_none());
        assert_eq!(rs, ResourceServer::from_settings(&settings_in(Mode::Dev)));
    }

    #[test]
    fn oidc_issuer_and_discovery_url_are_independent_of_each_other() {
        let rs = resource_server(
            Mode::Dev,
            &[
                ("OIDC_ISSUER", "http://localhost:8080/realms/r/"),
                (
                    "OIDC_DISCOVERY_URL",
                    "http://keycloak:8080/realms/r/.well-known/openid-configuration",
                ),
                ("OIDC_AUDIENCE", "other"),
                ("OIDC_SCOPES_SUPPORTED", r#"["openid","profile"]"#),
                ("PUBLIC_BASE_URL", "https://api.example.com/"),
            ],
        )
        .unwrap();
        // Used verbatim: `iss` is an exact-string comparison.
        assert_eq!(rs.issuer, "http://localhost:8080/realms/r/");
        assert_eq!(
            rs.discovery_url,
            "http://keycloak:8080/realms/r/.well-known/openid-configuration"
        );
        assert_eq!(rs.audience, "other");
        assert_eq!(rs.scopes_supported, ["openid", "profile"]);
        assert_eq!(rs.public_base_url, "https://api.example.com");

        // OIDC_ISSUER alone also moves the derived discovery URL.
        let rs = resource_server(Mode::Dev, &[("OIDC_ISSUER", "http://i.example/r/")]).unwrap();
        assert_eq!(
            rs.discovery_url,
            "http://i.example/r/.well-known/openid-configuration"
        );
    }

    #[test]
    fn resource_server_rejects_bad_audience_scopes_and_public_base_url() {
        for (key, bad) in [
            ("OIDC_AUDIENCE", ""),
            ("OIDC_SCOPES_SUPPORTED", "openid"),
            ("PUBLIC_BASE_URL", "ftp://api.example.com"),
            ("PUBLIC_BASE_URL", "https://api.example.com/base"),
            ("PUBLIC_BASE_URL", "https://api.example.com?x=1"),
            ("PUBLIC_BASE_URL", "https://"),
            ("PUBLIC_BASE_URL", "api.example.com"),
        ] {
            let err = resource_server(Mode::Dev, &[(key, bad)])
                .expect_err(&format!("{key}={bad:?} must be rejected"));
            assert!(err.contains(key), "{key}={bad:?}: {err}");
        }
    }

    #[test]
    fn production_requires_a_public_https_origin_and_https_oidc_urls() {
        let err = resource_server(Mode::Production, &[]).unwrap_err();
        assert!(err.contains("PUBLIC_BASE_URL must be set"), "{err}");
        assert!(err.contains("OIDC_ISSUER_URL/OIDC_ISSUER must use https://"), "{err}");
        assert!(err.contains("OIDC_DISCOVERY_URL must use https://"), "{err}");

        let err = resource_server(
            Mode::Production,
            &[
                ("PUBLIC_BASE_URL", "http://api.example.com"),
                ("OIDC_ISSUER", "https://i.example/r"),
                ("OIDC_DISCOVERY_URL", "http://keycloak/x"),
            ],
        )
        .unwrap_err();
        assert!(err.contains("PUBLIC_BASE_URL must use https://"), "{err}");
        assert!(err.contains("OIDC_DISCOVERY_URL must use https://"), "{err}");
        assert!(!err.contains("OIDC_ISSUER"), "{err}");

        assert!(resource_server(
            Mode::Production,
            &[
                ("PUBLIC_BASE_URL", "https://api.example.com"),
                ("OIDC_ISSUER", "https://i.example/r"),
            ],
        )
        .is_ok());
    }

    #[test]
    fn security_txt_is_off_without_a_contact_and_renders_every_field() {
        assert!(resource_server(Mode::Dev, &[("SECURITY_TXT_POLICY", "https://p.example")])
            .unwrap()
            .security_txt
            .is_none());

        let expires = in_days(30);
        let rs = resource_server(
            Mode::Dev,
            &[
                ("SECURITY_TXT_CONTACT", "mailto:security@example.com"),
                ("SECURITY_TXT_EXPIRES", &expires),
                ("SECURITY_TXT_POLICY", "https://example.com/policy"),
                ("SECURITY_TXT_CANONICAL", "https://example.com/.well-known/security.txt"),
                ("SECURITY_TXT_PREFERRED_LANGUAGES", "en, de"),
            ],
        )
        .unwrap();
        let body = rs.security_txt.unwrap().render();
        let lines: Vec<&str> = body.lines().collect();
        assert_eq!(lines[0], "Contact: mailto:security@example.com");
        assert!(lines[1].starts_with("Expires: ") && lines[1].ends_with('Z'));
        assert_eq!(lines[2], "Preferred-Languages: en, de");
        assert_eq!(lines[3], "Policy: https://example.com/policy");
        assert_eq!(
            lines[4],
            "Canonical: https://example.com/.well-known/security.txt"
        );
        assert!(body.ends_with('\n'));

        // Only the required fields: nothing optional is invented.
        let rs = resource_server(
            Mode::Dev,
            &[
                ("SECURITY_TXT_CONTACT", "https://example.com/report"),
                ("SECURITY_TXT_EXPIRES", &expires),
            ],
        )
        .unwrap();
        assert_eq!(rs.security_txt.unwrap().render().lines().count(), 2);
    }

    #[test]
    fn security_txt_rejects_a_missing_unzoned_past_or_distant_expires() {
        let contact = ("SECURITY_TXT_CONTACT", "tel:+15555550100");
        let err = resource_server(Mode::Dev, &[contact]).unwrap_err();
        assert!(err.contains("SECURITY_TXT_EXPIRES must be set"), "{err}");

        for (bad, needle) in [
            ("2099-01-01T00:00:00", "RFC 3339"),
            ("tomorrow", "RFC 3339"),
            (in_days(-1).as_str(), "in the future"),
            (in_days(400).as_str(), "at most one year"),
        ] {
            let err = resource_server(Mode::Dev, &[contact, ("SECURITY_TXT_EXPIRES", bad)])
                .expect_err(bad);
            assert!(err.contains(needle), "{bad}: {err}");
        }
        // A bound just inside a year passes, in any zone.
        let near = (chrono::Utc::now() + chrono::Duration::days(364))
            .with_timezone(&chrono::FixedOffset::east_opt(7200).unwrap())
            .to_rfc3339();
        assert!(resource_server(Mode::Dev, &[contact, ("SECURITY_TXT_EXPIRES", &near)]).is_ok());
    }

    #[test]
    fn security_txt_rejects_a_bad_contact_url_or_multiline_values() {
        let expires = in_days(30);
        let expires = ("SECURITY_TXT_EXPIRES", expires.as_str());
        for (extra, needle) in [
            (("SECURITY_TXT_CONTACT", "http://example.com"), "https:, mailto: or tel:"),
            (("SECURITY_TXT_CONTACT", "   "), "single line"),
            (("SECURITY_TXT_CONTACT", "mailto:a@b\nContact: evil"), "single line"),
        ] {
            let err = resource_server(Mode::Dev, &[extra, expires]).unwrap_err();
            assert!(err.contains(needle), "{extra:?}: {err}");
        }
        for key in ["SECURITY_TXT_POLICY", "SECURITY_TXT_CANONICAL"] {
            let err = resource_server(
                Mode::Dev,
                &[("SECURITY_TXT_CONTACT", "mailto:a@b"), expires, (key, "http://x")],
            )
            .unwrap_err();
            assert!(err.contains("must be https:// URLs"), "{key}: {err}");
        }
        let err = resource_server(
            Mode::Dev,
            &[
                ("SECURITY_TXT_CONTACT", "mailto:a@b"),
                expires,
                ("SECURITY_TXT_PREFERRED_LANGUAGES", "en\r\nPolicy: x"),
            ],
        )
        .unwrap_err();
        assert!(err.contains("SECURITY_TXT_PREFERRED_LANGUAGES"), "{err}");
    }

    #[test]
    fn settings_from_env_defaults_the_audience_and_validates_the_resource_server() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        clear_all();
        let settings = Settings::from_env().unwrap();
        assert_eq!(settings.oidc_audience.as_deref(), Some("api"));
        assert_eq!(
            ResourceServer::from_env(&settings).unwrap().audience,
            "api"
        );
        unsafe { env::set_var("PUBLIC_BASE_URL", "not a url") };
        assert!(Settings::from_env().unwrap_err().contains("PUBLIC_BASE_URL"));
        clear_all();
    }

    #[test]
    fn production_checks_the_public_oidc_issuer_not_only_the_legacy_one() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        clear_all();
        unsafe { env::set_var("MODE", "production") };
        unsafe { env::set_var("POSTGRES_PASSWORD", "a-real-secret") };
        unsafe { env::set_var("RUSTFS_ACCESS_KEY", "a-real-key") };
        unsafe { env::set_var("RUSTFS_SECRET_KEY", "a-real-secret-key") };
        unsafe { env::set_var("S3_ENDPOINT_URL", "https://s3.example.com") };
        unsafe { env::set_var("REDIS_URL", "rediss://redis.example.com:6379/0") };
        unsafe { env::set_var("POSTGRES_SSLMODE", "verify-full") };
        unsafe { env::set_var("PUBLIC_BASE_URL", "https://api.example.com") };
        unsafe { env::set_var("OIDC_ISSUER_URL", "https://issuer.example.com/realms/api") };
        unsafe { env::set_var("OIDC_ISSUER", "http://localhost:8080/realms/api") };
        let err = Settings::from_env().expect_err("a plaintext public issuer must be rejected");
        assert!(err.contains("OIDC_ISSUER"), "{err}");
        unsafe { env::set_var("OIDC_ISSUER", "https://issuer.example.com/realms/api") };
        assert!(Settings::from_env().is_ok());
        clear_all();
    }
}
