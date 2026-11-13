use axum::Json;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde::Serialize;
use tflip_engine::ace_error::codes::AE_INVALID_DATA;
use ts_rs::TS;

use crate::vault::VaultError;

/// Every failure has this one shape, the studio's own (an unknown cursor)
/// with the nearest ADS code, so every view shows errors alike.
#[derive(Debug, Serialize, TS)]
#[ts(export)]
pub struct ApiError {
    /// The ADS error code, as ACE and ARC report it (5004, 7200, ...).
    pub code: u32,
    /// The whole text, as `AdsGetLastError` gives it: `Error 5004:  ...`.
    pub message: String,
}

impl From<tflip_engine::Error> for ApiError {
    fn from(error: tflip_engine::Error) -> Self {
        let ace = error.to_ace();
        ApiError {
            code: ace.code,
            message: ace.message(),
        }
    }
}

/// One status for every failure: the frontend acts on the ADS code, so
/// mapping codes to HTTP statuses would only be guesswork it never reads.
impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (StatusCode::BAD_REQUEST, Json(self)).into_response()
    }
}

/// What a handler that needs the vault fails with: an error to show, or a
/// locked vault, which the frontend answers by asking for the master
/// password and trying again.
pub enum Failure {
    Api(ApiError),
    Locked,
}

impl From<ApiError> for Failure {
    fn from(error: ApiError) -> Self {
        Failure::Api(error)
    }
}

impl From<tflip_engine::Error> for Failure {
    fn from(error: tflip_engine::Error) -> Self {
        Failure::Api(error.into())
    }
}

impl From<VaultError> for Failure {
    fn from(error: VaultError) -> Self {
        match error {
            VaultError::Locked => Failure::Locked,
            VaultError::WrongPassword => tflip_engine::Error::InvalidPassword.into(),
            VaultError::Unreadable => Failure::Api(ApiError {
                code: AE_INVALID_DATA,
                message: "The stored password cannot be read. Enter it again.".to_owned(),
            }),
        }
    }
}

/// 423 without a body, like the 401 of a wrong token: told apart from every
/// ADS error by its status alone.
impl IntoResponse for Failure {
    fn into_response(self) -> Response {
        match self {
            Failure::Api(error) => error.into_response(),
            Failure::Locked => StatusCode::LOCKED.into_response(),
        }
    }
}

#[cfg(test)]
mod tests {
    use axum::body::to_bytes;

    use super::*;

    #[tokio::test]
    async fn answers_an_engine_error_with_its_ads_code_and_text() {
        let response = ApiError::from(tflip_engine::Error::FileNotFound).into_response();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let expected = r#"{"code":5004,"message":"Error 5004:  Either ACE could not find the specified file, or you do not have sufficient rights to access the file."}"#;
        assert_eq!(&body[..], expected.as_bytes());
    }
}
