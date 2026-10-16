//! Per-credential traffic policies. Zero means unlimited; None means inherit.
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, sync::{Arc, atomic::{AtomicU32, Ordering}}};
use futures::{Stream, StreamExt};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AccountTier { Free, Pro, ProPlus, Max, Power, Student, Unknown }

impl AccountTier {
    pub fn detect(title: Option<&str>) -> Self {
        let title = title.unwrap_or_default().to_ascii_uppercase();
        // Match whole normalized names, not arbitrary substrings (e.g. PROMO).
        match title.trim().strip_prefix("KIRO ").unwrap_or(title.trim()).trim() {
            "FREE" => Self::Free, "PRO" => Self::Pro,
            "PRO+" | "PRO PLUS" => Self::ProPlus, "MAX" | "PRO MAX" => Self::Max,
            "POWER" => Self::Power, "STUDENT" => Self::Student, _ => Self::Unknown,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
pub struct Limits { pub concurrency: Option<u32>, pub rpm: Option<u32> }
impl Limits {
    pub fn validate(&self) -> anyhow::Result<()> {
        anyhow::ensure!(self.concurrency.unwrap_or(0) <= 100_000 && self.rpm.unwrap_or(0) <= 100_000,
            "并发和 RPM 必须是 0～100000 的整数；0 表示不限");
        Ok(())
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
pub struct CredentialTraffic {
    pub tier: Option<AccountTier>,
    pub concurrency: Option<u32>,
    pub rpm: Option<u32>,
}
impl CredentialTraffic {
    pub fn validate(&self) -> anyhow::Result<()> {
        Limits { concurrency: self.concurrency, rpm: self.rpm }.validate()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
pub struct TrafficPolicies {
    pub tiers: BTreeMap<AccountTier, Limits>,
    pub default_concurrency: u32,
    pub auto_disable_quota: bool,
    pub disable_at_base_quota: bool,
}
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
pub struct LimitPatch {
    #[serde(deserialize_with = "present_limit")]
    pub concurrency: Option<Option<u32>>,
    #[serde(deserialize_with = "present_limit")]
    pub rpm: Option<Option<u32>>,
}

// Missing means unchanged; explicit null restores inheritance; zero is unlimited.
fn present_limit<'de, D: serde::Deserializer<'de>>(deserializer: D) -> Result<Option<Option<u32>>, D::Error> {
    Option::<u32>::deserialize(deserializer).map(Some)
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
pub struct TrafficPolicyPatch {
    pub tiers: BTreeMap<AccountTier, LimitPatch>,
    pub default_concurrency: Option<u32>,
    pub auto_disable_quota: Option<bool>,
    pub disable_at_base_quota: Option<bool>,
}

impl Default for TrafficPolicies {
    fn default() -> Self {
        Self { tiers: [AccountTier::Free, AccountTier::Pro, AccountTier::ProPlus,
            AccountTier::Max, AccountTier::Power, AccountTier::Student].into_iter()
            .map(|tier| (tier, Limits::default())).collect(),
            default_concurrency: 0, auto_disable_quota: true, disable_at_base_quota: true }
    }
}
impl TrafficPolicies {
    pub fn validate(&self) -> anyhow::Result<()> {
        anyhow::ensure!(self.default_concurrency <= 100_000, "默认并发不能超过 100000");
        for limits in self.tiers.values() { limits.validate()?; }
        Ok(())
    }
    pub fn resolve(&self, credential: &CredentialTraffic, title: Option<&str>, default_rpm: u32) -> EffectiveLimits {
        let tier = credential.tier.unwrap_or_else(|| AccountTier::detect(title));
        let defaults = self.tiers.get(&tier).cloned().unwrap_or_default();
        let source = |own: Option<u32>, category: Option<u32>| {
            if own.is_some() { "credential" } else if category.is_some() { "tier" } else { "global" }.to_string()
        };
        EffectiveLimits {
            tier,
            concurrency: credential.concurrency.or(defaults.concurrency).unwrap_or(self.default_concurrency),
            rpm: credential.rpm.or(defaults.rpm).unwrap_or(default_rpm),
            concurrency_source: source(credential.concurrency, defaults.concurrency),
            rpm_source: source(credential.rpm, defaults.rpm),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EffectiveLimits {
    pub tier: AccountTier, pub concurrency: u32, pub rpm: u32,
    pub concurrency_source: String, pub rpm_source: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TrafficStatus {
    pub id: u64, pub settings: CredentialTraffic, pub effective: EffectiveLimits,
    pub concurrency: u32, pub rpm: usize, pub limited: bool,
}

/// The counter outlives deleted credentials; every permit is released exactly once.
pub struct TrafficPermit(pub(super) Arc<AtomicU32>);
impl Drop for TrafficPermit {
    fn drop(&mut self) { self.0.fetch_sub(1, Ordering::AcqRel); }
}

/// Keep the permit with the response body, including while a stream is suspended.
pub struct LimitedResponse {
    response: reqwest::Response, permit: TrafficPermit,
    proxy_use: Option<super::token_manager::static_ips::ProxyUse>,
}
impl LimitedResponse {
    pub fn new(response: reqwest::Response, permit: TrafficPermit) -> Self { Self { response, permit, proxy_use: None } }
    pub(crate) fn with_proxy_use(mut self, proxy_use: super::token_manager::static_ips::ProxyUse) -> Self {
        self.proxy_use=Some(proxy_use); self
    }
    pub async fn text(self) -> Result<String, reqwest::Error> {
        let Self { response, permit: _permit, proxy_use: _proxy_use } = self;
        response.text().await
    }
    pub async fn bytes(self) -> Result<bytes::Bytes, reqwest::Error> {
        let Self { response, permit: _permit, proxy_use: _proxy_use } = self;
        response.bytes().await
    }
    pub fn status(&self) -> reqwest::StatusCode { self.response.status() }
    pub fn bytes_stream(self) -> impl Stream<Item = Result<bytes::Bytes, reqwest::Error>> {
        let Self { response, permit, proxy_use } = self;
        futures::stream::unfold((Box::pin(response.bytes_stream()), permit, proxy_use), |(mut stream, permit, proxy_use)| async move {
            stream.next().await.map(|chunk| (chunk, (stream, permit, proxy_use)))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn limit_patch_preserves_omission_null_and_zero() {
        let empty: LimitPatch = serde_json::from_str("{}").unwrap();
        assert!(empty.concurrency.is_none() && empty.rpm.is_none());
        let patch: LimitPatch = serde_json::from_str(r#"{"concurrency":null,"rpm":0}"#).unwrap();
        assert_eq!(patch.concurrency, Some(None));
        assert_eq!(patch.rpm, Some(Some(0)));
        for invalid in [r#"{"rpm":-1}"#, r#"{"rpm":1.5}"#, r#"{"rpm":"20"}"#, r#"{"other":5}"#] {
            assert!(serde_json::from_str::<LimitPatch>(invalid).is_err());
        }
    }
    #[test]
    fn inheritance_is_dynamic_and_independent() {
        let mut p = TrafficPolicies::default();
        p.tiers.insert(AccountTier::Power, Limits { concurrency: Some(300), rpm: Some(200) });
        let inherited = CredentialTraffic::default();
        let own = CredentialTraffic { concurrency: Some(150), rpm: Some(100), ..Default::default() };
        assert_eq!(p.resolve(&inherited, Some("KIRO POWER"), 60).concurrency, 300);
        p.tiers.insert(AccountTier::Power, Limits { concurrency: Some(200), rpm: Some(80) });
        assert_eq!(p.resolve(&inherited, Some("KIRO POWER"), 60).rpm, 80);
        let effective = p.resolve(&own, Some("KIRO POWER"), 60);
        assert_eq!((effective.concurrency, effective.rpm), (150, 100));
        let partial = CredentialTraffic { concurrency: Some(0), ..Default::default() };
        let effective = p.resolve(&partial, Some("KIRO POWER"), 60);
        assert_eq!((effective.concurrency, effective.rpm), (0, 80));
        assert_eq!(effective.concurrency_source, "credential");
        assert_eq!(effective.rpm_source, "tier");
    }
    #[test]
    fn pro_max_subscription_uses_max_policy_and_preserves_account_overrides() {
        for title in ["KIRO PRO MAX", "PRO MAX", " kiro pro max ", "KIRO MAX", "MAX"] {
            assert_eq!(AccountTier::detect(Some(title)), AccountTier::Max);
        }
        for title in ["KIRO PRO MAXIMUM", "PROMO MAX", "MAX TRIAL"] {
            assert_eq!(AccountTier::detect(Some(title)), AccountTier::Unknown);
        }
        let mut policies = TrafficPolicies::default();
        policies.tiers.insert(AccountTier::Max, Limits { concurrency: Some(100), rpm: Some(50) });
        policies.tiers.insert(AccountTier::Power, Limits { concurrency: Some(200), rpm: Some(100) });
        let inherited = policies.resolve(&CredentialTraffic::default(), Some("KIRO PRO MAX"), 60);
        assert_eq!(inherited.tier, AccountTier::Max);
        assert_eq!((inherited.concurrency, inherited.rpm), (100, 50));
        assert_eq!((inherited.concurrency_source.as_str(), inherited.rpm_source.as_str()), ("tier", "tier"));
        let own = CredentialTraffic { concurrency: Some(15), rpm: Some(10), ..Default::default() };
        let effective = policies.resolve(&own, Some("KIRO PRO MAX"), 60);
        assert_eq!((effective.concurrency, effective.rpm), (15, 10));
        let manual = CredentialTraffic { tier: Some(AccountTier::Power), ..Default::default() };
        let effective = policies.resolve(&manual, Some("KIRO PRO MAX"), 60);
        assert_eq!(effective.tier, AccountTier::Power);
        assert_eq!((effective.concurrency, effective.rpm), (200, 100));
    }

    #[test]
    fn all_six_tiers_and_unknown_are_distinct() {
        for (title, tier) in [("FREE", AccountTier::Free), ("PRO", AccountTier::Pro),
            ("PRO+", AccountTier::ProPlus), ("MAX", AccountTier::Max),
            ("POWER", AccountTier::Power), ("STUDENT", AccountTier::Student)] {
            assert_eq!(AccountTier::detect(Some(&format!("KIRO {title}"))), tier);
        }
        assert_eq!(AccountTier::detect(Some("PROMO")), AccountTier::Unknown);
        let manual = CredentialTraffic { tier: Some(AccountTier::Max), ..Default::default() };
        assert_eq!(TrafficPolicies::default().resolve(&manual, Some("KIRO POWER"), 0).tier, AccountTier::Max);
    }
}
