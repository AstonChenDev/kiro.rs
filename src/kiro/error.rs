//! Shared typed errors for Kiro upstream calls.

/// The upstream still returned HTTP 429 after any applicable failover/retry.
#[derive(Debug, Clone, thiserror::Error)]
#[error("upstream rate limited")]
pub struct UpstreamRateLimitError {
    retry_after: Option<String>,
}

/// Kiro rejected the serialized conversation because it exceeded the model's
/// context limit. Keeping this typed at the provider boundary lets compact
/// retry once without reverse-engineering an HTTP error response.
#[derive(Debug, Clone, thiserror::Error)]
#[error("upstream context window exceeded")]
pub struct UpstreamContextOverflowError;

/// Model capacity is temporarily unavailable, independently of account health.
#[derive(Debug, Clone, thiserror::Error)]
#[error("upstream model temporarily unavailable")]
pub struct UpstreamModelUnavailableError {
    pub retry_after: String,
}

/// A transient token-refresh failure. The account stays enabled and retains its proxy.
#[derive(Debug, Clone, thiserror::Error)]
#[error("账号网络或刷新暂时失败，冷却 {retry_after_secs} 秒后自动重试")]
pub struct CredentialRefreshCooldownError {
    pub retry_after_secs: u64,
}

#[derive(Debug, thiserror::Error)]
#[error("upstream response deadline exceeded")]
pub struct UpstreamTimeoutError {
    #[source]
    pub source: reqwest::Error,
}

impl UpstreamRateLimitError {
    pub(crate) fn new(retry_after: Option<String>) -> Self {
        Self {
            retry_after: retry_after.and_then(normalize_retry_after),
        }
    }

    pub(crate) fn from_headers(headers: &http::HeaderMap) -> Self {
        let retry_after = headers
            .get(http::header::RETRY_AFTER)
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned);
        Self::new(retry_after)
    }

    pub fn retry_after(&self) -> Option<&str> {
        self.retry_after.as_deref()
    }

    /// Without an explicit upstream delay, a short local retry is still useful.
    pub(crate) fn should_retry_locally(&self) -> bool {
        self.retry_after.is_none()
    }
}

fn normalize_retry_after(value: String) -> Option<String> {
    let value = value.trim();
    if value.is_empty() {
        return None;
    }

    if value.parse::<u64>().is_ok() || httpdate::parse_http_date(value).is_ok() {
        Some(value.to_string())
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_delta_seconds_and_http_date() {
        let seconds = UpstreamRateLimitError::new(Some(" 1800 ".to_string()));
        assert_eq!(seconds.retry_after(), Some("1800"));
        assert!(!seconds.should_retry_locally());

        let date = "Sun, 12 Jul 2026 02:30:00 GMT";
        let http_date = UpstreamRateLimitError::new(Some(date.to_string()));
        assert_eq!(http_date.retry_after(), Some(date));
        assert!(!http_date.should_retry_locally());
    }

    #[test]
    fn rejects_invalid_retry_after() {
        let error = UpstreamRateLimitError::new(Some("not-a-retry-delay".to_string()));
        assert_eq!(error.retry_after(), None);
        assert!(error.should_retry_locally());
    }
}

/// 明确的上游认证拒绝。原因采用受控文案，不能把原始上游响应回传客户端。
#[derive(Debug, Clone, Copy, thiserror::Error)]
pub enum UpstreamCredentialError {
    #[error("上游拒绝认证：Token 无效或已过期，请重新登录并更新凭据")]
    InvalidToken,
    #[error("上游拒绝访问：当前凭据权限不足，请检查账号权限及订阅状态")]
    Forbidden,
    #[error("上游账号已被封禁或停用，请更换凭据或联系上游恢复账号")]
    Suspended,
    #[error("凭据的 profileArn 无效，请重新登录或导入包含正确 profileArn 的完整凭据")]
    InvalidProfileArn,
}

impl UpstreamCredentialError {
    pub(crate) fn from_response(status: u16, body: &str) -> Option<Self> {
        match status {
            401 => Some(Self::InvalidToken),
            403 if crate::kiro::endpoint::default_is_account_suspended(body) => Some(Self::Suspended),
            403 if crate::kiro::endpoint::default_is_bearer_token_invalid(body) => Some(Self::InvalidToken),
            403 if body.contains("Invalid profileArn") => Some(Self::InvalidProfileArn),
            403 => Some(Self::Forbidden),
            _ => None,
        }
    }
}

/// 可安全返回给客户端的凭据原因，不包含原始上游响应体。
pub(crate) fn credential_error_reason(error: &anyhow::Error) -> Option<String> {
    if let Some(config) = error.downcast_ref::<crate::kiro::model::credentials::InvalidCredentialConfig>() {
        Some(config.to_string())
    } else if error.downcast_ref::<crate::kiro::token_manager::RefreshTokenInvalidError>().is_some() {
        Some("refreshToken 已失效或被撤销，请重新登录并更新凭据".to_string())
    } else {
        error.downcast_ref::<UpstreamCredentialError>().map(ToString::to_string)
    }
}

#[cfg(test)]
mod credential_status_regressions {
    use super::*;
    #[test]
    fn auth_reasons_are_specific_without_echoing_upstream_body() {
        for (status, body, reason) in [
            (401, "secret", "Token 无效或已过期"),
            (403, "secret", "权限不足"),
            (403, "The bearer token included in the request is invalid; secret", "Token 无效或已过期"),
            (403, "suspended; locked your account; secret", "封禁或停用"),
            (403, "Invalid profileArn: secret", "profileArn 无效"),
        ] {
            let error = UpstreamCredentialError::from_response(status, body).unwrap();
            assert!(error.to_string().contains(reason));
            assert!(!error.to_string().contains("secret"));
        }
        for status in [400, 429, 500, 502] {
            assert!(UpstreamCredentialError::from_response(status, "secret").is_none());
        }
    }
}
