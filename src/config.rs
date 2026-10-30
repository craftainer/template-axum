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
            oidc_audience: env::var("OIDC_AUDIENCE").ok(),
        };

        settings.validate()?;
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

        if self.oidc_audience.is_none() {
            return Err("OIDC_AUDIENCE must be set when MODE=production".to_string());
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
        if !self.oidc_issuer_url.starts_with("https://") {
            insecure.push("OIDC_ISSUER_URL must use https://");
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
            "OIDC_AUDIENCE",
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
    fn production_rejects_default_credentials_and_missing_audience() {
        let _guard = ENV_LOCK.lock().unwrap();
        clear_all();
        unsafe { env::set_var("MODE", "production") };
        let err = Settings::from_env().expect_err("defaults must be rejected in production");
        assert!(err.contains("OIDC_AUDIENCE"));

        unsafe { env::set_var("OIDC_AUDIENCE", "api") };
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
}
