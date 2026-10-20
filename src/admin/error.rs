//! Admin API 错误类型定义

use std::fmt;

use axum::{
    Json,
    http::{StatusCode, header},
    response::{IntoResponse, Response},
};

use super::types::AdminErrorResponse;

/// Admin 服务错误类型
#[derive(Debug)]
pub enum AdminServiceError {
    /// Safe diagnostic for transport failures during credential initialization.
    ImportNetwork { stage: &'static str, kind: &'static str, attempts: usize },
    /// 凭据不存在
    NotFound { id: u64 },

    /// 上游服务调用失败（网络、API 错误等）
    UpstreamError(String),

    /// 上游明确返回限流，可选携带合法 Retry-After。
    RateLimited { retry_after: Option<String> },

    /// 内部状态错误
    InternalError(String),

    /// 普通参数或操作条件不满足。
    InvalidRequest(String),

    /// 凭据无效（验证失败）
    InvalidCredential(String),
}

impl fmt::Display for AdminServiceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AdminServiceError::ImportNetwork { stage, kind, attempts } =>
                write!(f, "{}网络请求失败：{}（已尝试 {} 次），请检查网络或代理后重试", stage, kind, attempts),
            AdminServiceError::NotFound { id } => {
                write!(f, "凭据不存在: {}", id)
            }
            AdminServiceError::UpstreamError(_) => write!(f, "上游服务请求失败"),
            AdminServiceError::RateLimited { .. } => write!(f, "上游请求过于频繁，请稍后重试"),
            AdminServiceError::InternalError(msg) => write!(f, "内部错误: {}", msg),
            AdminServiceError::InvalidRequest(msg) => write!(f, "请求无效: {}", msg),
            AdminServiceError::InvalidCredential(msg) => write!(f, "凭据无效: {}", msg),
        }
    }
}

impl std::error::Error for AdminServiceError {}

impl AdminServiceError {
    /// 获取对应的 HTTP 状态码
    pub fn status_code(&self) -> StatusCode {
        match self {
            AdminServiceError::ImportNetwork { .. } => StatusCode::BAD_GATEWAY,
            AdminServiceError::NotFound { .. } => StatusCode::NOT_FOUND,
            AdminServiceError::UpstreamError(_) => StatusCode::BAD_GATEWAY,
            AdminServiceError::RateLimited { .. } => StatusCode::TOO_MANY_REQUESTS,
            AdminServiceError::InternalError(_) => StatusCode::INTERNAL_SERVER_ERROR,
            AdminServiceError::InvalidRequest(_) => StatusCode::BAD_REQUEST,
            AdminServiceError::InvalidCredential(_) => StatusCode::FORBIDDEN,
        }
    }

    /// 转换为 API 错误响应
    pub fn into_response(self) -> AdminErrorResponse {
        match &self {
            AdminServiceError::ImportNetwork { .. } => AdminErrorResponse::api_error(self.to_string()),
            AdminServiceError::NotFound { .. } => AdminErrorResponse::not_found(self.to_string()),
            AdminServiceError::UpstreamError(_) => AdminErrorResponse::api_error(self.to_string()),
            AdminServiceError::RateLimited { .. } => {
                AdminErrorResponse::rate_limit(self.to_string())
            }
            AdminServiceError::InternalError(_) => {
                AdminErrorResponse::internal_error(self.to_string())
            }
            AdminServiceError::InvalidRequest(_) => {
                AdminErrorResponse::invalid_request(self.to_string())
            }
            AdminServiceError::InvalidCredential(_) => {
                AdminErrorResponse::new("permission_error", self.to_string())
            }
        }
    }

    pub fn into_http_response(self) -> Response {
        if let AdminServiceError::UpstreamError(message) = &self {
            tracing::warn!(error = %message, "Admin 上游服务请求失败");
        }
        let retry_after = match &self {
            AdminServiceError::RateLimited { retry_after } => retry_after.clone(),
            _ => None,
        };
        let status = self.status_code();
        let mut response = (status, Json(self.into_response())).into_response();
        if let Some(value) = retry_after.and_then(|value| value.parse().ok()) {
            response.headers_mut().insert(header::RETRY_AFTER, value);
        }
        response
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn rate_limit_response_has_status_header_and_stable_body() {
        let response = AdminServiceError::RateLimited {
            retry_after: Some("120".to_string()),
        }
        .into_http_response();

        assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
        assert_eq!(response.headers().get(header::RETRY_AFTER).unwrap(), "120");
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let body: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(body["error"]["type"], "rate_limit_error");
        assert!(!body["error"]["message"].as_str().unwrap().is_empty());
    }

    #[tokio::test]
    async fn upstream_error_response_does_not_expose_raw_body() {
        let secret = "aws-account=123456789012 request-id=private-request";
        let response = AdminServiceError::UpstreamError(secret.to_string()).into_http_response();

        assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let body = String::from_utf8(body.to_vec()).unwrap();
        assert!(!body.contains(secret));
        assert!(body.contains("上游服务请求失败"));
    }
}

#[cfg(test)]
mod credential_status_regressions {
    use super::*;
    #[tokio::test]
    async fn invalid_credentials_return_403_with_reason_and_other_statuses_stay_distinct() {
        let response = AdminServiceError::InvalidCredential("kiroApiKey 为空".into()).into_http_response();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let body: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(body["error"]["type"], "permission_error");
        assert_eq!(body["error"]["message"], "凭据无效: kiroApiKey 为空");
        assert_eq!(AdminServiceError::InvalidRequest("rpm 越界".into()).status_code(), StatusCode::BAD_REQUEST);
        assert_eq!(AdminServiceError::NotFound { id: 1 }.status_code(), StatusCode::NOT_FOUND);
        assert_eq!(AdminServiceError::UpstreamError("network".into()).status_code(), StatusCode::BAD_GATEWAY);
    }
}

#[cfg(test)]
mod import_network_diagnostics_tests {
    use super::*;
    #[test]
    fn import_network_failures_are_502_with_safe_actionable_details() {
        let error=AdminServiceError::ImportNetwork {stage:"刷新 Token",kind:"连接中断",attempts:3};
        assert_eq!(error.status_code(),StatusCode::BAD_GATEWAY);
        let text=error.to_string();
        assert!(text.contains("刷新 Token") && text.contains("连接中断") && text.contains("3 次"));
        assert!(!text.contains("内部错误"));
    }
}
