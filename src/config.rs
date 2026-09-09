//! Application settings, sourced entirely from the process environment --
//! see docs/adrs/0001 (layering) and the port of template-fastapi's
//! `app/config.py`. No `.env` file is read here, matching `docs/TEMPLATE.md`'s
//! "Don't" section: every value below comes from the process environment,
//! which the compose files set directly.

use std::env;

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

    pub s3_endpoint_url: String,
    pub s3_access_key: String,
    pub s3_secret_key: String,

    pub redis_url: String,

    /// The MQTT broker backing the CRUD event stream (`src/events.rs`,
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

fn env_or(key: &str, default: &str) -> String {
    env::var(key).unwrap_or_else(|_| default.to_string())
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
        ] {
            unsafe { env::remove_var(key) };
        }
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
        assert!(Settings::from_env().is_ok());
        clear_all();
    }
}
