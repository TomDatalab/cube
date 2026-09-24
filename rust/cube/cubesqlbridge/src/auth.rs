//! `checkSqlAuth` without JavaScript.
//!
//! [`RustSqlAuthService`] reproduces `SQLServer.createDefaultCheckSqlAuthFn`
//! (`CUBEJS_SQL_USER` / `CUBEJS_SQL_PASSWORD`) and adds the one thing the JS
//! default could not do on its own: accept a Cube JWT as the password and take
//! the session's security context from it, through [`cubeauth`].

use std::any::Any;
use std::env;
use std::sync::Arc;

use async_trait::async_trait;
use cubeauth::Authenticator;
use cubesql::sql::{
    AuthContext, AuthenticateResponse, SqlAuthService, SqlAuthServiceAuthenticateRequest,
};
use cubesql::{di_service, CubeError};
use serde_json::{Map, Value};

/// The session of one SQL API connection.
#[derive(Debug, Clone)]
pub struct RustSqlAuthContext {
    pub user: Option<String>,
    pub superuser: bool,
    pub security_context: Option<Value>,
}

impl AuthContext for RustSqlAuthContext {
    fn as_any(&self) -> &dyn Any {
        self
    }

    fn user(&self) -> Option<&String> {
        self.user.as_ref()
    }

    fn security_context(&self) -> Option<&Value> {
        self.security_context.as_ref()
    }
}

/// The static half of the SQL API's credentials.
#[derive(Debug, Clone, Default)]
pub struct SqlAuthConfig {
    /// `CUBEJS_SQL_USER`. `None` accepts any user name.
    pub sql_user: Option<String>,
    /// `CUBEJS_SQL_PASSWORD`.
    pub sql_password: Option<String>,
    /// `CUBEJS_SQL_SUPER_USER`: the one user allowed to become another user.
    pub sql_super_user: Option<String>,
    /// `CUBEJS_DEV_MODE`. Outside dev mode a missing user or password is a
    /// misconfiguration rather than an open door.
    pub dev_mode: bool,
}

impl SqlAuthConfig {
    /// Reads the configuration from the environment, applying the production
    /// defaults `createDefaultCheckSqlAuthFn` applies.
    ///
    /// Unlike Node this does not invent a random password when one is missing
    /// in production: a generated password nobody is told is the same as
    /// refusing every connection, so this refuses them with a message that
    /// names the variable instead.
    pub fn from_env() -> Self {
        Self {
            sql_user: non_empty_env("CUBEJS_SQL_USER"),
            sql_password: non_empty_env("CUBEJS_SQL_PASSWORD"),
            sql_super_user: non_empty_env("CUBEJS_SQL_SUPER_USER"),
            dev_mode: matches!(
                non_empty_env("CUBEJS_DEV_MODE").as_deref(),
                Some("true") | Some("1")
            ),
        }
    }

    /// The user name accepted when none is configured: `cube`, the same
    /// fallback the Node default warns about and uses.
    pub fn effective_user(&self) -> Option<String> {
        match (&self.sql_user, self.dev_mode) {
            (Some(user), _) => Some(user.clone()),
            (None, true) => None,
            (None, false) => Some("cube".to_string()),
        }
    }

    /// `true` when `user` is the configured superuser.
    pub fn is_super_user(&self, user: Option<&str>) -> bool {
        match (&self.sql_super_user, user) {
            (Some(super_user), Some(user)) => super_user == user,
            _ => false,
        }
    }

    /// `createDefaultCanSwitchSqlUserFn`: switching to yourself is always
    /// allowed, and otherwise only the superuser may switch.
    pub fn can_switch_user(&self, current: Option<&str>, to_user: &str) -> bool {
        if current == Some(to_user) {
            return true;
        }

        match &self.sql_super_user {
            Some(super_user) => current == Some(super_user.as_str()),
            None => false,
        }
    }
}

fn non_empty_env(name: &str) -> Option<String> {
    env::var(name).ok().filter(|value| !value.is_empty())
}

/// `checkSqlAuth`, natively.
pub struct RustSqlAuthService {
    config: SqlAuthConfig,
    /// When set, a password that is a Cube JWT is verified against it and its
    /// claims become the session's security context.
    authenticator: Option<Arc<Authenticator>>,
}

impl std::fmt::Debug for RustSqlAuthService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // `Authenticator` holds signing secrets, so only its presence is shown.
        f.debug_struct("RustSqlAuthService")
            .field("config", &self.config)
            .field("authenticator", &self.authenticator.is_some())
            .finish()
    }
}

di_service!(RustSqlAuthService, [SqlAuthService]);

impl RustSqlAuthService {
    pub fn new(config: SqlAuthConfig) -> Self {
        Self {
            config,
            authenticator: None,
        }
    }

    /// Also accept a Cube JWT as the password.
    pub fn with_authenticator(mut self, authenticator: Arc<Authenticator>) -> Self {
        self.authenticator = Some(authenticator);
        self
    }

    pub fn config(&self) -> &SqlAuthConfig {
        &self.config
    }

    fn incorrect_credentials(user: Option<&str>) -> CubeError {
        CubeError::user(format!(
            "Incorrect user name \"{}\" or password",
            user.unwrap_or_default()
        ))
    }

    /// A password that carries two dots is worth trying as a JWT; anything
    /// else cannot be one, and trying it would only produce a confusing error.
    fn looks_like_jwt(password: &str) -> bool {
        password.split('.').count() == 3
    }

    async fn authenticate_with_token(
        &self,
        user: Option<&str>,
        password: &str,
    ) -> Option<AuthenticateResponse> {
        let authenticator = self.authenticator.as_ref()?;
        if !Self::looks_like_jwt(password) {
            return None;
        }

        let result = match authenticator.authenticate(Some(password)).await {
            Ok(result) => result,
            Err(e) => {
                tracing::debug!(error = %e, "SQL API token authentication failed");
                return None;
            }
        };

        let security_context = authenticator.extract_security_context(&result.security_context);

        Some(AuthenticateResponse {
            context: Arc::new(RustSqlAuthContext {
                user: user.map(str::to_string),
                superuser: self.config.is_super_user(user),
                security_context: Some(security_context),
            }),
            password: Some(password.to_string()),
            // The signature was already checked, so there is nothing left to
            // compare the password against.
            skip_password_check: true,
        })
    }
}

#[async_trait]
impl SqlAuthService for RustSqlAuthService {
    async fn authenticate(
        &self,
        _request: SqlAuthServiceAuthenticateRequest,
        user: Option<String>,
        password: Option<String>,
    ) -> Result<AuthenticateResponse, CubeError> {
        if let Some(password) = password.as_deref() {
            if let Some(response) = self
                .authenticate_with_token(user.as_deref(), password)
                .await
            {
                return Ok(response);
            }
        }

        let superuser = self.config.is_super_user(user.as_deref());

        if let Some(allowed) = self.config.effective_user() {
            // The superuser is a user in its own right, so it is let through
            // even when it is not the one `CUBEJS_SQL_USER` names.
            if user.as_deref() != Some(allowed.as_str()) && !superuser {
                return Err(Self::incorrect_credentials(user.as_deref()));
            }
        }

        if self.config.sql_password.is_none() && !self.config.dev_mode {
            return Err(CubeError::user(
                "The SQL API has no password configured. Set CUBEJS_SQL_PASSWORD, or run in \
                 development mode (CUBEJS_DEV_MODE=true) to connect without one."
                    .to_string(),
            ));
        }

        Ok(AuthenticateResponse {
            context: Arc::new(RustSqlAuthContext {
                user,
                superuser,
                security_context: Some(Value::Object(Map::new())),
            }),
            password: self.config.sql_password.clone(),
            skip_password_check: self.config.dev_mode && self.config.sql_password.is_none(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config() -> SqlAuthConfig {
        SqlAuthConfig {
            sql_user: Some("cube".to_string()),
            sql_password: Some("secret".to_string()),
            sql_super_user: Some("admin".to_string()),
            dev_mode: false,
        }
    }

    fn request() -> SqlAuthServiceAuthenticateRequest {
        SqlAuthServiceAuthenticateRequest {
            protocol: "postgres".to_string(),
            method: "password".to_string(),
        }
    }

    async fn authenticate(
        config: SqlAuthConfig,
        user: &str,
    ) -> Result<AuthenticateResponse, CubeError> {
        RustSqlAuthService::new(config)
            .authenticate(
                request(),
                Some(user.to_string()),
                Some("secret".to_string()),
            )
            .await
    }

    #[tokio::test]
    async fn the_configured_user_is_accepted_and_hands_back_its_password() {
        let response = authenticate(config(), "cube")
            .await
            .expect("the configured user should be accepted");

        assert_eq!(response.password.as_deref(), Some("secret"));
        assert!(!response.skip_password_check);

        let context = response
            .context
            .as_any()
            .downcast_ref::<RustSqlAuthContext>()
            .expect("the context should be a RustSqlAuthContext");
        assert_eq!(context.user.as_deref(), Some("cube"));
        assert!(!context.superuser);
        assert_eq!(context.security_context, Some(Value::Object(Map::new())));
    }

    #[tokio::test]
    async fn another_user_is_refused() {
        let error = authenticate(config(), "someone-else")
            .await
            .expect_err("an unknown user should be refused");
        assert!(
            error.message.contains("Incorrect user name"),
            "unexpected message: {}",
            error.message
        );
    }

    #[tokio::test]
    async fn a_wrong_password_is_refused_by_the_protocol_layer() {
        // `checkSqlAuth` hands the expected password back rather than
        // comparing it; the comparison is the pg auth service's job, and this
        // pins the value it compares against.
        let response = authenticate(config(), "cube")
            .await
            .expect("the user is valid");
        assert_ne!(response.password.as_deref(), Some("not-the-password"));
        assert!(!response.skip_password_check);
    }

    #[tokio::test]
    async fn the_superuser_is_accepted_even_though_it_is_not_the_sql_user() {
        let response = authenticate(config(), "admin")
            .await
            .expect("the superuser should be accepted");

        let context = response
            .context
            .as_any()
            .downcast_ref::<RustSqlAuthContext>()
            .expect("the context should be a RustSqlAuthContext");
        assert!(context.superuser);
    }

    #[tokio::test]
    async fn without_a_configured_user_any_name_is_accepted_in_dev_mode() {
        let config = SqlAuthConfig {
            sql_user: None,
            sql_password: None,
            sql_super_user: None,
            dev_mode: true,
        };

        let response = RustSqlAuthService::new(config)
            .authenticate(request(), Some("anybody".to_string()), None)
            .await
            .expect("dev mode accepts any user");
        assert!(response.skip_password_check);
    }

    #[tokio::test]
    async fn a_missing_password_outside_dev_mode_is_a_misconfiguration() {
        let config = SqlAuthConfig {
            sql_user: Some("cube".to_string()),
            sql_password: None,
            sql_super_user: None,
            dev_mode: false,
        };

        let error = RustSqlAuthService::new(config)
            .authenticate(request(), Some("cube".to_string()), Some("x".to_string()))
            .await
            .expect_err("a password is required outside dev mode");
        assert!(
            error.message.contains("CUBEJS_SQL_PASSWORD"),
            "unexpected message: {}",
            error.message
        );
    }

    #[test]
    fn only_the_superuser_may_switch_to_another_user() {
        let config = config();

        // Switching to yourself is always allowed.
        assert!(config.can_switch_user(Some("cube"), "cube"));
        // A regular user may not become someone else.
        assert!(!config.can_switch_user(Some("cube"), "admin"));
        assert!(!config.can_switch_user(Some("cube"), "other"));
        // The superuser may.
        assert!(config.can_switch_user(Some("admin"), "cube"));
        assert!(config.can_switch_user(Some("admin"), "other"));
    }

    #[test]
    fn nobody_may_switch_when_no_superuser_is_configured() {
        let config = SqlAuthConfig {
            sql_super_user: None,
            ..config()
        };

        assert!(config.can_switch_user(Some("cube"), "cube"));
        assert!(!config.can_switch_user(Some("cube"), "other"));
        assert!(!config.can_switch_user(None, "other"));
    }

    #[test]
    fn the_production_default_user_is_cube() {
        let config = SqlAuthConfig {
            sql_user: None,
            sql_password: Some("secret".to_string()),
            sql_super_user: None,
            dev_mode: false,
        };
        assert_eq!(config.effective_user().as_deref(), Some("cube"));

        let dev = SqlAuthConfig {
            dev_mode: true,
            ..config
        };
        assert_eq!(dev.effective_user(), None);
    }
}
