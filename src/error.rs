use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Serialize;
use std::path::PathBuf;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum ResqlError {
    #[error("Saved query '{0}' does not exist")]
    QueryNotFound(String),

    #[error("Specified dataSourceName name: '{0}' is unknown to the service")]
    UnknownDataSource(String),

    #[error("No value supplied for the SQL parameter '{0}': No value registered for key '{0}'")]
    MissingParameter(String),

    #[error("Unexpected parameter '{0}' not declared for this endpoint")]
    UnknownParameter(String),

    #[error("Parameter '{name}' has the wrong type: expected {expected}, got {actual}")]
    InvalidParameterType {
        name: String,
        expected: &'static str,
        actual: String,
    },

    #[error("Parameter '{name}' value {value} is not in the allowed set {allowed:?}")]
    InvalidParameterValue {
        name: String,
        value: String,
        allowed: Vec<String>,
    },

    #[error("Invalid query file '{path}': {reason}")]
    InvalidQuery { path: PathBuf, reason: String },

    #[error("Invalid declaration in '{path}': {reason}")]
    InvalidDeclaration { path: PathBuf, reason: String },

    #[error("Invalid SQL directory '{path}': {reason}")]
    InvalidDirectory { path: PathBuf, reason: String },

    #[error("SQL execution failed: {0}")]
    SqlExecution(String),

    #[error("Request body too large")]
    BodyTooLarge,

    #[error("Malformed request body: {0}")]
    MalformedRequest(String),

    #[error("Internal error: {0}")]
    Internal(String),

    #[error(
        "X-Datasource '{requested}' is not permitted for project '{project}' (not in allowlist)"
    )]
    ForbiddenDatasourceOverride { project: String, requested: String },
}

impl ResqlError {
    fn kind(&self) -> &'static str {
        match self {
            ResqlError::QueryNotFound(_) => "ResqlRuntimeException",
            ResqlError::UnknownDataSource(_) => "UnknownDataSourceNameException",
            ResqlError::MissingParameter(_) => "InvalidDataAccessApiUsageException",
            ResqlError::UnknownParameter(_) => "UnknownParameterException",
            ResqlError::InvalidParameterType { .. } => "InvalidParameterTypeException",
            ResqlError::InvalidParameterValue { .. } => "InvalidParameterValueException",
            ResqlError::InvalidQuery { .. } => "InvalidQueryException",
            ResqlError::InvalidDeclaration { .. } => "InvalidDeclarationException",
            ResqlError::InvalidDirectory { .. } => "InvalidDirectoryException",
            ResqlError::SqlExecution(_) => "BadSqlGrammarException",
            ResqlError::BodyTooLarge => "PayloadTooLargeException",
            ResqlError::MalformedRequest(_) => "MalformedRequestException",
            ResqlError::Internal(_) => "InternalError",
            ResqlError::ForbiddenDatasourceOverride { .. } => {
                "ForbiddenDatasourceOverrideException"
            }
        }
    }

    fn status(&self) -> StatusCode {
        match self {
            ResqlError::BodyTooLarge => StatusCode::PAYLOAD_TOO_LARGE,
            ResqlError::Internal(_) => StatusCode::INTERNAL_SERVER_ERROR,
            ResqlError::ForbiddenDatasourceOverride { .. } => StatusCode::FORBIDDEN,
            _ => StatusCode::BAD_REQUEST,
        }
    }
}

#[derive(Serialize)]
struct ErrorBody<'a> {
    error: &'a str,
    message: String,
}

impl IntoResponse for ResqlError {
    fn into_response(self) -> Response {
        // Emit a structured log line for every error we return so
        // operators see the failure alongside the response. Field
        // names align with the OpenTelemetry HTTP semantic-conventions
        // used by the request middleware; the source-chain toggle is
        // handled by the middleware / handler that has config access
        // (see `logging.print_stack_trace`).
        let status_code = self.status().as_u16();
        let kind = self.kind();
        let message = self.to_string();
        if status_code >= 500 {
            tracing::error!(
                error.kind = %kind,
                http.response.status_code = status_code,
                "{message}"
            );
        } else {
            tracing::warn!(
                error.kind = %kind,
                http.response.status_code = status_code,
                "{message}"
            );
        }
        let body = ErrorBody {
            error: kind,
            message,
        };
        (self.status(), Json(body)).into_response()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn query_not_found_matches_java_message() {
        let e = ResqlError::QueryNotFound("/foo/bar".into());
        assert_eq!(e.to_string(), "Saved query '/foo/bar' does not exist");
        assert_eq!(e.kind(), "ResqlRuntimeException");
        assert_eq!(e.status(), StatusCode::BAD_REQUEST);
    }

    #[test]
    fn unknown_datasource_matches_java_message() {
        let e = ResqlError::UnknownDataSource("orders".into());
        assert_eq!(
            e.to_string(),
            "Specified dataSourceName name: 'orders' is unknown to the service"
        );
        assert_eq!(e.kind(), "UnknownDataSourceNameException");
    }

    #[test]
    fn missing_parameter_message_shape() {
        let e = ResqlError::MissingParameter("login".into());
        assert!(e.to_string().contains("'login'"));
        assert_eq!(e.kind(), "InvalidDataAccessApiUsageException");
    }

    #[test]
    fn body_too_large_is_413() {
        assert_eq!(
            ResqlError::BodyTooLarge.status(),
            StatusCode::PAYLOAD_TOO_LARGE
        );
    }
}
