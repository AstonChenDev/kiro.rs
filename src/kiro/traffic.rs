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
            "PRO+" | "PRO PLUS" => Self::ProPlus, "MAX" => Self::Max,
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
pub struct TrafficPolicyPatch {
    pub tiers: BTreeMap<AccountTier, Limits>,
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
pub struct LimitedResponse { response: reqwest::Response, permit: TrafficPermit }
impl LimitedResponse {
    pub fn new(response: reqwest::Response, permit: TrafficPermit) -> Self { Self { response, permit } }
    pub async fn text(self) -> Result<String, reqwest::Error> {
        let Self { response, permit: _permit } = self;
        response.text().await
    }
    pub async fn bytes(self) -> Result<bytes::Bytes, reqwest::Error> {
        let Self { response, permit: _permit } = self;
        response.bytes().await
    }
    pub fn status(&self) -> reqwest::StatusCode { self.response.status() }
    pub fn bytes_stream(self) -> impl Stream<Item = Result<bytes::Bytes, reqwest::Error>> {
        let Self { response, permit } = self;
        futures::stream::unfold((Box::pin(response.bytes_stream()), permit), |(mut stream, permit)| async move {
            stream.next().await.map(|chunk| (chunk, (stream, permit)))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
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
