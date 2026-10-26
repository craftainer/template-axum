//! The app's Postgres connection pool, built from `config::DbSettings`
//! (`docs/adrs/0022`, `docs/nfrs/NFR-0031`).
//!
//! sea-orm wraps an sqlx `PgPool`; `connect_options` turns the settings into
//! a sea-orm `ConnectOptions`, passing a value only when its env var is set
//! so an unconfigured process keeps sea-orm/sqlx's own defaults. The libpq
//! `sslmode`, which sea-orm has no setter for, goes through
//! `map_sqlx_postgres_opts` onto sqlx's `PgConnectOptions`.
//!
//! There is deliberately no PgBouncer mode: sqlx names every prepared
//! statement even with `statement_cache_capacity(0)`, and sea-orm exposes no
//! `persistent(false)`; see `docs/adrs/0022`.
//!
//! Layering: this module imports only `config` (`src/README.md`).

use sea_orm::sqlx::postgres::{PgConnectOptions, PgSslMode};
use sea_orm::{ConnectOptions, Database, DatabaseConnection, DbErr};

use crate::config::{PostgresSslMode, Settings};

fn sqlx_ssl_mode(mode: PostgresSslMode) -> PgSslMode {
    match mode {
        PostgresSslMode::Disable => PgSslMode::Disable,
        PostgresSslMode::Allow => PgSslMode::Allow,
        PostgresSslMode::Prefer => PgSslMode::Prefer,
        PostgresSslMode::Require => PgSslMode::Require,
        PostgresSslMode::VerifyCa => PgSslMode::VerifyCa,
        PostgresSslMode::VerifyFull => PgSslMode::VerifyFull,
    }
}

/// Apply the settings sea-orm's `ConnectOptions` cannot express onto sqlx's
/// per-connection options: an explicit `POSTGRES_SSLMODE` overrides any
/// `sslmode` in `DATABASE_URL`.
fn apply_pg_options(
    options: PgConnectOptions,
    sslmode: Option<PostgresSslMode>,
) -> PgConnectOptions {
    match sslmode {
        Some(mode) => options.ssl_mode(sqlx_ssl_mode(mode)),
        None => options,
    }
}

/// The sea-orm `ConnectOptions` for `settings`.
pub fn connect_options(settings: &Settings) -> ConnectOptions {
    let db = &settings.db;
    let mut options = ConnectOptions::new(settings.database_url());
    if let Some(size) = db.pool_size {
        options.max_connections(size);
    }
    if let Some(size) = db.pool_min_size {
        options.min_connections(size);
    }
    if let Some(recycle) = db.pool_recycle {
        options.max_lifetime(recycle);
    }
    if let Some(idle) = db.pool_idle_timeout {
        options.idle_timeout(idle);
    }
    if let Some(acquire) = db.connect_timeout {
        options.connect_timeout(acquire);
    }
    let sslmode = db.sslmode;
    options.map_sqlx_postgres_opts(move |pg| apply_pg_options(pg, sslmode));
    options
}

/// Open the pool described by `settings`.
pub async fn connect(settings: &Settings) -> Result<DatabaseConnection, DbErr> {
    Database::connect(connect_options(settings)).await
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;
    use crate::config::{DbSettings, Mode};

    fn settings(db: DbSettings) -> Settings {
        Settings {
            app_name: "test".into(),
            mode: Mode::Dev,
            allow_mock_mode: false,
            postgres_user: "u".into(),
            postgres_password: "p".into(),
            postgres_db: "d".into(),
            postgres_host: "h".into(),
            postgres_port: 5432,
            db,
            s3_endpoint_url: String::new(),
            s3_access_key: String::new(),
            s3_secret_key: String::new(),
            redis_url: String::new(),
            mqtt_host: String::new(),
            mqtt_port: 1883,
            rate_limit_mock_token_per_minute: 10,
            rate_limit_hero_write_per_minute: 20,
            bulk_action_max_matched: 1000,
            oidc_issuer_url: String::new(),
            oidc_authorization_url: String::new(),
            oidc_token_url: String::new(),
            oidc_client_id: String::new(),
            oidc_audience: None,
        }
    }

    #[test]
    fn unset_settings_leave_every_pool_default_alone() {
        let options = connect_options(&settings(DbSettings::default()));
        assert_eq!(options.get_max_connections(), None);
        assert_eq!(options.get_min_connections(), None);
        assert_eq!(options.get_connect_timeout(), None);
        assert_eq!(options.get_acquire_timeout(), None);
        assert_eq!(options.get_idle_timeout(), None);
        assert_eq!(options.get_max_lifetime(), None);
    }

    #[test]
    fn set_settings_reach_the_connect_options() {
        let options = connect_options(&settings(DbSettings {
            pool_size: Some(8),
            pool_min_size: Some(2),
            pool_recycle: Some(Duration::from_secs(1800)),
            pool_idle_timeout: Some(Duration::from_secs(300)),
            connect_timeout: Some(Duration::from_secs(5)),
            ..DbSettings::default()
        }));
        assert_eq!(options.get_url(), "postgres://u:p@h:5432/d");
        assert_eq!(options.get_max_connections(), Some(8));
        assert_eq!(options.get_min_connections(), Some(2));
        assert_eq!(
            options.get_max_lifetime(),
            Some(Some(Duration::from_secs(1800)))
        );
        assert_eq!(
            options.get_idle_timeout(),
            Some(Some(Duration::from_secs(300)))
        );
        assert_eq!(options.get_connect_timeout(), Some(Duration::from_secs(5)));
    }

    #[test]
    fn sslmode_overrides_the_url_and_unset_keeps_it() {
        let url: PgConnectOptions = "postgres://u:p@h/d?sslmode=disable".parse().unwrap();
        let kept = apply_pg_options(url.clone(), None);
        assert!(matches!(kept.get_ssl_mode(), PgSslMode::Disable));

        for (mode, expected) in [
            (PostgresSslMode::Disable, PgSslMode::Disable),
            (PostgresSslMode::Allow, PgSslMode::Allow),
            (PostgresSslMode::Prefer, PgSslMode::Prefer),
            (PostgresSslMode::Require, PgSslMode::Require),
            (PostgresSslMode::VerifyCa, PgSslMode::VerifyCa),
            (PostgresSslMode::VerifyFull, PgSslMode::VerifyFull),
        ] {
            let applied = apply_pg_options(url.clone(), Some(mode));
            assert_eq!(
                std::mem::discriminant(&applied.get_ssl_mode()),
                std::mem::discriminant(&expected)
            );
        }
    }
}
