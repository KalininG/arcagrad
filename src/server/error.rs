//! HTTP error responses.

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Serialize;
use utoipa::ToSchema;

#[derive(Serialize, ToSchema)]
pub struct ErrorResponse {
    pub error: String,
}

pub enum AppError {
    NotFound,
    BadRequest(String),
    Unauthorized,
    Forbidden,
    Conflict(String),
    TooManyRequests,
    /// An external-source error safe to show to the client.
    Upstream(String),
    Internal(anyhow::Error),
}

impl From<anyhow::Error> for AppError {
    fn from(e: anyhow::Error) -> Self {
        AppError::Internal(e)
    }
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let (status, message) = match self {
            AppError::NotFound => (StatusCode::NOT_FOUND, "not found".to_string()),
            AppError::BadRequest(m) => (StatusCode::BAD_REQUEST, m),
            AppError::Unauthorized => (StatusCode::UNAUTHORIZED, "unauthorized".to_string()),
            AppError::Forbidden => (StatusCode::FORBIDDEN, "forbidden".to_string()),
            AppError::Conflict(m) => (StatusCode::CONFLICT, m),
            AppError::TooManyRequests => (
                StatusCode::TOO_MANY_REQUESTS,
                "too many attempts — try again later".to_string(),
            ),
            AppError::Upstream(m) => (StatusCode::BAD_GATEWAY, m),
            AppError::Internal(e) => {
                // Keep internal details out of the response.
                tracing::error!("internal error: {e:?}");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "internal server error".to_string(),
                )
            }
        };
        (status, Json(ErrorResponse { error: message })).into_response()
    }
}
