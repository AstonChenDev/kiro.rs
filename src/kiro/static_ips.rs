//! Exclusive static proxy inventory. Bindings are derived from credentials, never copied
//! into a second on-disk map. Lock order: inventory -> credential persistence -> entries.
use super::*;
use std::collections::HashSet;
use std::io::Write;

#[derive(Debug, thiserror::Error)]
#[error("{0}")]
pub struct StaticIpError(pub String);
fn invalid(message: impl Into<String>) -> anyhow::Error { StaticIpError(message.into()).into() }

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StaticIp {
    pub id: u64,
    pub url: String,
    pub username: Option<String>,
    pub password: Option<String>,
    pub label: Option<String>,
    pub enabled: bool,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Inventory {
    pub auto_assign: bool,
    pub allowed_providers: Vec<String>,
    pub next_id: u64,
    pub entries: Vec<StaticIp>,
}
impl Default for Inventory {
    fn default() -> Self {
        Self { auto_assign: false, allowed_providers: vec!["github".into(), "google".into()], next_id: 0, entries: vec![] }
    }
}
/// Only explicitly identified personal identities are eligible. Corporate methods win
/// over inconsistent provider labels; unknown and API-key identities are excluded.
pub fn personal_provider(c: &KiroCredentials) -> Option<&'static str> {
    if c.is_api_key_credential() || c.is_external_idp_credential() { return None; }
    let provider = c.provider.as_deref().unwrap_or("").trim().to_ascii_lowercase();
    let method = crate::kiro::model::credentials::canonicalize_auth_method_value(c.auth_method.as_deref().unwrap_or_else(|| if c.client_id.is_some() && c.client_secret.is_some() { "idc" } else { "" }).trim()).to_ascii_lowercase();
    match (method.as_str(), provider.as_str()) {
        ("social" | "", "github") => Some("github"),
        ("social" | "", "google") => Some("google"),
        ("idc" | "", "builderid" | "builder-id" | "builder_id") => Some("builderid"),
        _ => None,
    }
}
fn eligible(inventory: &Inventory, c: &KiroCredentials) -> bool {
    personal_provider(c).is_some_and(|p| inventory.allowed_providers.iter().any(|v| v == p))
}
fn proxy_busy(e: &CredentialEntry) -> bool {
    e.active_requests.load(Ordering::Acquire) > 0 || e.proxy_operations.load(Ordering::Acquire) > 0
}
/// Caller holds inventory and entries, in that order. Legacy ordinary proxies
/// outside the personal policy must not be silently adopted or reconfigured.
pub(super) fn check_static_binding(store: &StaticIpStore, entries: &[CredentialEntry], entry: &CredentialEntry) -> anyhow::Result<()> {
    let c = &entry.credentials;
    if !store.contains(c.proxy_url.as_deref()) { return Ok(()); }
    if !c.static_ip_required && !eligible(&store.inventory, c) { return Ok(()); }
    if !eligible(&store.inventory, c) {
        return Err(invalid("该凭据类型不允许使用静态 IP，正在等待旧请求结束并释放"));
    }
    let key = c.proxy_url.as_deref().and_then(endpoint_key);
    if entries.iter().any(|other| other.id != entry.id && other.credentials.proxy_url.as_deref().and_then(endpoint_key) == key) {
        return Err(invalid("静态 IP 被多个账号占用，请先解除冲突绑定后重试"));
    }
    Ok(())
}
/// Pins an account's proxy across token refresh, context preparation and admin queries.
#[derive(Clone)]
pub struct StaticIpLease { _counter: Arc<LeaseCounter> }
/// Pins the actual proxy host until its network operation/body has finished.
/// Weak inventory entries do not keep requests alive; dropping never takes a lock.
pub(crate) struct ProxyUse {
    _hosts: Vec<Arc<()>>,
    _binding: Option<StaticIpLease>,
    proxy: Option<ProxyConfig>,
}
impl ProxyUse {
    pub(crate) fn proxy(&self) -> Option<&ProxyConfig> { self.proxy.as_ref() }
}
struct LeaseCounter(Arc<AtomicU32>);
impl Drop for LeaseCounter { fn drop(&mut self) { self.0.fetch_sub(1, Ordering::AcqRel); } }
/// Run after state transitions, outside all entry/persistence locks.
pub(super) struct ReconcileOnDrop<'a>(pub &'a MultiTokenManager);
impl Drop for ReconcileOnDrop<'_> {
    fn drop(&mut self) { self.0.reconcile_static_ips_logged(); }
}
pub(crate) struct StaticIpStore {
    pub(super) inventory: Inventory,
    pub(super) reserved: HashSet<String>,
    // Ordinary proxies remain shareable; each in-flight import pins its host
    // against being adopted into the exclusive inventory until it finishes.
    ordinary_imports: HashMap<String, usize>,
    active_proxy_hosts: HashMap<String, std::sync::Weak<()>>,
    path: Option<PathBuf>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct StaticIpInput {
    pub host: String,
    pub port: u16,
    pub protocol: String,
    pub username: Option<String>,
    pub password: Option<String>,
    pub label: Option<String>,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StaticIpView {
    pub id: u64,
    pub url: String,
    pub username: Option<String>,
    pub has_password: bool,
    pub label: Option<String>,
    pub enabled: bool,
    pub status: &'static str,
    pub credential_ids: Vec<u64>,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StaticIpResponse {
    pub auto_assign: bool,
    pub allowed_providers: Vec<String>,
    pub waiting_credential_ids: Vec<u64>,
    pub entries: Vec<StaticIpView>,
}

/// One IP is exclusive even across different ports/auth/protocols. Also recognize legacy
/// bare host:port account settings so an already-used endpoint cannot be reassigned.
pub(super) fn endpoint_key(value: &str) -> Option<String> {
    let value = value.trim();
    let normalized = if value.contains("://") { value.to_string() } else { format!("http://{value}") };
    let url = reqwest::Url::parse(&normalized).ok()?;
    // URL's built-in defaults omit SOCKS, while reqwest accepts these proxy
    // schemes without a port and connects to 1080. Keep host identity aligned.
    url.port_or_known_default().or_else(||
        matches!(url.scheme(), "socks4" | "socks4a" | "socks5" | "socks5h").then_some(1080)
    )?;
    let host = url.host_str()?;
    // IPv4-mapped IPv6 reaches the same IPv4 proxy. Normalize only its identity,
    // not the actual connection URL; native/compatible IPv6 stays distinct.
    if let Ok(std::net::IpAddr::V6(address)) = host.trim_start_matches('[').trim_end_matches(']').parse() {
        if let Some(ipv4) = address.to_ipv4_mapped() { return Some(ipv4.to_string()); }
    }
    // Non-special URLs (SOCKS) retain numeric IPv4 spellings that the socket
    // resolver accepts. Parse host identity with the URL IPv4 rules as HTTP does.
    // Keep domain names and the actual proxy URL unchanged.
    if !host.starts_with('[') {
        if let Ok(canonical) = reqwest::Url::parse(&format!("http://{host}/")) {
            if let Some(address) = canonical.host_str().and_then(|v| v.parse::<std::net::Ipv4Addr>().ok()) {
                return Some(address.to_string());
            }
        }
    }
    Some(host.to_ascii_lowercase())
}
impl StaticIpInput {
    fn into_entry(self, id: u64) -> anyhow::Result<StaticIp> {
        if !["http", "https", "socks5", "socks5h"].contains(&self.protocol.as_str()) {
            return Err(invalid("静态 IP 协议必须为 HTTP、HTTPS、SOCKS5 或 SOCKS5H"));
        }
        let host = self.host.trim().trim_start_matches('[').trim_end_matches(']');
        let address: std::net::IpAddr = host.parse().map_err(|_| invalid("请输入有效的静态 IP 地址，端口请填写在端口栏"))?;
        if self.port == 0 { return Err(invalid("端口必须为 1～65535")); }
        if self.username.as_deref().is_some_and(|v| v.len() > 1024)
            || self.password.as_deref().is_some_and(|v| v.len() > 4096) {
            return Err(invalid("代理认证信息过长"));
        }
        let host = match address { std::net::IpAddr::V6(_) => format!("[{address}]"), _ => address.to_string() };
        let username = self.username.filter(|s| !s.is_empty());
        let password = self.password.filter(|s| !s.is_empty());
        if password.is_some() && username.is_none() { return Err(invalid("填写代理密码时也需要填写用户名")); }
        Ok(StaticIp { id, url: format!("{}://{}:{}", self.protocol, host, self.port), username, password,
            label: self.label.map(|s| s.trim().to_string()).filter(|s| !s.is_empty()), enabled: true })
    }
}
impl StaticIpStore {
    pub(super) fn load(path: Option<PathBuf>) -> anyhow::Result<Self> {
        let inventory = match path.as_ref().map(std::fs::read).transpose() {
            Ok(Some(bytes)) => serde_json::from_slice::<Inventory>(&bytes)
                .map_err(|_| invalid("静态 IP 配置文件损坏，请恢复备份后再启动"))?,
            Ok(None) => Inventory::default(),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Inventory::default(),
            Err(_) => return Err(invalid("无法读取静态 IP 配置文件")),
        };
        validate_providers(&inventory.allowed_providers)?;
        let mut keys = HashSet::new();
        for entry in &inventory.entries {
            let key = endpoint_key(&entry.url).ok_or_else(|| invalid("静态 IP 配置包含无效地址"))?;
            if !keys.insert(key) { return Err(invalid("静态 IP 配置包含重复地址")); }
        }
        Ok(Self { inventory, reserved: HashSet::new(), ordinary_imports: HashMap::new(), active_proxy_hosts: HashMap::new(), path })
    }
    fn save(&mut self, next: Inventory) -> anyhow::Result<()> {
        if let Some(path) = &self.path {
            let tmp = path.with_extension(format!("{}.tmp", uuid::Uuid::new_v4()));
            let result = (|| -> anyhow::Result<()> {
                let mut opts = std::fs::OpenOptions::new(); opts.write(true).create_new(true);
                #[cfg(unix)] { use std::os::unix::fs::OpenOptionsExt; opts.mode(0o600); }
                let mut file = opts.open(&tmp)?;
                file.write_all(&serde_json::to_vec_pretty(&next)?)?; file.sync_all()?; drop(file);
                std::fs::rename(&tmp, path)?; Ok(())
            })();
            if result.is_err() { let _ = std::fs::remove_file(tmp); return Err(invalid("静态 IP 保存失败，请检查目录权限和磁盘空间")); }
        }
        self.inventory = next; Ok(())
    }
    pub(super) fn contains(&self, url: Option<&str>) -> bool {
        url.and_then(endpoint_key).is_some_and(|key| self.inventory.entries.iter().any(|e| endpoint_key(&e.url).as_ref() == Some(&key)))
    }
}
pub(super) struct Reservation {
    manager: Arc<MultiTokenManager>, key: Option<String>, exclusive: bool,
    pub(super) proxy: ProxyConfig,
}
impl Drop for Reservation {
    fn drop(&mut self) {
        let Some(key) = &self.key else { return; };
        let mut store = self.manager.static_ips.lock();
        if self.exclusive { store.reserved.remove(key); }
        else if let Some(count) = store.ordinary_imports.get_mut(key) {
            *count -= 1;
            if *count == 0 { store.ordinary_imports.remove(key); }
        }
    }
}

/// Called while holding inventory -> persistence (for static imports) -> entries.
pub(super) fn validate_import_commit(store: &StaticIpStore, entries: &[CredentialEntry], cred: &KiroCredentials, reservation: Option<&Reservation>) -> anyhow::Result<()> {
    if !store.contains(cred.proxy_url.as_deref()) {
        if cred.static_ip_required { return Err(invalid("静态 IP 绑定已变化，账号未导入，请重试")); }
        return Ok(());
    }
    let key = cred.proxy_url.as_deref().and_then(endpoint_key).unwrap();
    if !cred.static_ip_required || !eligible(&store.inventory, cred)
        || !reservation.is_some_and(|r| r.exclusive && r.key.as_ref() == Some(&key))
        || !store.reserved.contains(&key) || store.ordinary_imports.contains_key(&key)
        || entries.iter().any(|e| e.credentials.proxy_url.as_deref().and_then(endpoint_key).as_ref() == Some(&key)) {
        return Err(invalid("静态 IP 占用或类型策略已变化，账号未导入，请重试"));
    }
    Ok(())
}

fn validate_providers(providers: &[String]) -> anyhow::Result<()> {
    if providers.iter().any(|p| !["github", "google", "builderid"].contains(&p.as_str())) {
        return Err(invalid("仅支持个人类型 GitHub、Google、Builder ID；企业、API Key 和未知类型不可使用静态 IP"));
    }
    Ok(())
}
impl MultiTokenManager {
    pub(crate) fn pin_proxy_use(&self, credentials: &KiroCredentials, proxy: Option<&ProxyConfig>) -> anyhow::Result<ProxyUse> {
        let Some(key) = proxy.and_then(|p| endpoint_key(&p.url)) else {
            return Ok(ProxyUse { _hosts: Vec::new(), _binding: None, proxy: proxy.cloned() });
        };
        let mut store = self.static_ips.lock();
        // A snapshot can outlive an edit/deletion. If its old host has since
        // entered the inventory, only the current owner may start using it.
        let binding = if store.contains(proxy.map(|p| p.url.as_str())) {
            let entries = self.entries.lock();
            let entry = entries.iter().find(|e| Some(e.id) == credentials.id
                && e.credentials.proxy_url.as_deref().and_then(endpoint_key).as_ref() == Some(&key))
                .ok_or_else(|| invalid("代理 IP 已进入静态库且绑定已变化，请重试"))?;
            check_static_binding(&store, &entries, entry)?;
            // Login operations do not acquire a generation context. Pin the
            // binding atomically with ownership validation for every actual use.
            entry.proxy_operations.fetch_add(1, Ordering::AcqRel);
            Some(StaticIpLease { _counter: Arc::new(LeaseCounter(entry.proxy_operations.clone())) })
        } else { None };
        store.active_proxy_hosts.retain(|_, weak| weak.strong_count() > 0);
        let host = store.active_proxy_hosts.get(&key).and_then(std::sync::Weak::upgrade)
            .unwrap_or_else(|| Arc::new(()));
        store.active_proxy_hosts.insert(key, Arc::downgrade(&host));
        Ok(ProxyUse { _hosts: vec![host], _binding: binding, proxy: proxy.cloned() })
    }
    pub(crate) async fn prepare_proxy_use(&self, credentials: &KiroCredentials, proxy: Option<&ProxyConfig>) -> anyhow::Result<ProxyUse> {
        let Some(proxy) = proxy else { return self.pin_proxy_use(credentials, None); };
        let key = endpoint_key(&proxy.url).ok_or_else(|| invalid("代理地址无效"))?;
        if key.trim_matches(['[', ']']).parse::<std::net::IpAddr>().is_ok() {
            return self.pin_proxy_use(credentials, Some(proxy));
        }
        let normalized = if proxy.url.contains("://") { proxy.url.clone() } else { format!("http://{}", proxy.url) };
        let url = reqwest::Url::parse(&normalized)?;
        let port = url.port_or_known_default().unwrap_or(1080);
        // DNS never runs under inventory/credential locks. Pin its entire answer
        // set atomically, and use that exact snapshot for the ensuing connection.
        let addresses = tokio::time::timeout(std::time::Duration::from_secs(self.config.upstream_connect_timeout_secs.max(1)),
            tokio::net::lookup_host((url.host_str().unwrap(), port))).await
            .map_err(|_| anyhow::anyhow!("代理主机名解析超时"))??;
        let mut resolved = proxy.clone();
        resolved.url = normalized;
        resolved.resolved_addrs = addresses.collect();
        anyhow::ensure!(!resolved.resolved_addrs.is_empty(), "代理主机名未解析到地址");
        let mut keys = HashSet::from([key]);
        for address in &resolved.resolved_addrs {
            let ip = match address.ip() { std::net::IpAddr::V6(ip) => ip.to_ipv4_mapped().map(std::net::IpAddr::V4).unwrap_or(std::net::IpAddr::V6(ip)), ip => ip };
            keys.insert(match ip { std::net::IpAddr::V6(_) => format!("[{ip}]"), _ => ip.to_string() });
        }
        let mut store = self.static_ips.lock();
        if store.inventory.entries.iter().any(|ip| endpoint_key(&ip.url).is_some_and(|key| keys.contains(&key))) {
            return Err(invalid("代理主机名解析到已管理的静态 IP，请通过静态 IP 管理绑定，不能作为普通代理共用"));
        }
        store.active_proxy_hosts.retain(|_, weak| weak.strong_count() > 0);
        let hosts = keys.into_iter().map(|key| {
            let host = store.active_proxy_hosts.get(&key).and_then(std::sync::Weak::upgrade).unwrap_or_else(|| Arc::new(()));
            store.active_proxy_hosts.insert(key, Arc::downgrade(&host));
            host
        }).collect();
        Ok(ProxyUse { _hosts: hosts, _binding: None, proxy: Some(resolved) })
    }
    pub fn static_ip_config(&self, auto_assign: Option<bool>, providers: Option<Vec<String>>) -> anyhow::Result<()> {
        let mut store = self.static_ips.lock();
        let previous = store.inventory.clone();
        let mut next = previous.clone();
        if let Some(v) = auto_assign { next.auto_assign = v; }
        if let Some(mut v) = providers {
            validate_providers(&v)?; v.sort(); v.dedup(); next.allowed_providers = v;
            if !store.reserved.is_empty() { return Err(invalid("正在导入账号，请完成后再修改类型策略")); }
            if self.entries.lock().iter().any(|e| store.contains(e.credentials.proxy_url.as_deref()) && !eligible(&next, &e.credentials) && proxy_busy(e)) {
                return Err(invalid("受影响账号仍有请求进行中，请结束后再关闭该类型"));
            }
        }
        store.save(next)?;
        if let Err(error) = self.reconcile_static_ips_locked(&store, None) {
            store.save(previous)?;
            return Err(error);
        }
        Ok(())
    }
    pub(super) fn reconcile_static_ips_logged(&self) {
        if let Err(error) = self.reconcile_static_ips() { tracing::warn!(%error, "静态 IP 状态同步失败，保留占用并稍后重试"); }
    }
    pub fn reconcile_static_ips(&self) -> anyhow::Result<()> {
        let store = self.static_ips.lock();
        self.reconcile_static_ips_locked(&store, None)
    }
    fn reconcile_static_ips_locked(&self, store: &StaticIpStore, probe: Option<u64>) -> anyhow::Result<()> {
        let _persist = self.persist_lock.lock();
        let mut entries = self.entries.lock();
        let mut snapshot: Vec<_> = entries.iter().map(CredentialEntry::credentials_snapshot).collect();
        let mut changed = false;
        for (e, c) in entries.iter().zip(snapshot.iter_mut()) {
            let managed = store.contains(c.proxy_url.as_deref());
            let allowed = eligible(&store.inventory, c);
            if managed && (c.static_ip_required || allowed) && (e.disabled || !allowed) && !proxy_busy(e) {
                c.proxy_url=None; c.proxy_username=None; c.proxy_password=None;
                c.static_ip_required=allowed; changed=true;
            } else if managed && allowed && !c.static_ip_required {
                c.static_ip_required=true; changed=true;
            } else if !managed && c.static_ip_required && !allowed && !proxy_busy(e) {
                c.static_ip_required=false; changed=true;
            }
        }
        let mut used: HashSet<_> = snapshot.iter().filter_map(|c| c.proxy_url.as_deref().and_then(endpoint_key)).collect();
        for (e, c) in entries.iter().zip(snapshot.iter_mut()) {
            if c.static_ip_required && c.proxy_url.is_none() && (!e.disabled || probe==Some(e.id)) && eligible(&store.inventory, c) && !proxy_busy(e) {
                if let Some(ip) = store.inventory.entries.iter().find(|ip| {
                    let key=endpoint_key(&ip.url).unwrap();
                    ip.enabled && !used.contains(&key) && !store.reserved.contains(&key) && !self.static_is_global(&key)
                }) {
                    used.insert(endpoint_key(&ip.url).unwrap());
                    c.proxy_url=Some(ip.url.clone()); c.proxy_username=ip.username.clone(); c.proxy_password=ip.password.clone(); changed=true;
                }
            }
        }
        if changed {
            self.save_traffic_credentials(&snapshot)?;
            for (entry, c) in entries.iter_mut().zip(snapshot) { entry.credentials=c; }
        }
        Ok(())
    }
    /// Holds the proxy stable before any snapshot or network operation. A disabled
    /// account can borrow a free IP for an explicit quota/refresh probe, then release it.
    pub(super) fn static_ip_lease(&self, id: u64) -> anyhow::Result<StaticIpLease> {
        let store=self.static_ips.lock();
        self.reconcile_static_ips_locked(&store, Some(id))?;
        let entries=self.entries.lock();
        let e=entries.iter().find(|e| e.id==id).ok_or_else(|| invalid("凭据不存在"))?;
        if e.credentials.static_ip_required && e.credentials.proxy_url.is_none() {
            return Err(invalid("账号等待空闲静态 IP，请补充 IP 后重试"));
        }
        check_static_binding(&store, &entries, e)?;
        e.proxy_operations.fetch_add(1, Ordering::AcqRel);
        Ok(StaticIpLease { _counter: Arc::new(LeaseCounter(e.proxy_operations.clone())) })
    }
    pub fn start_static_ip_maintenance(self: &Arc<Self>) {
        let weak=Arc::downgrade(self);
        tokio::spawn(async move {
            let mut tick=tokio::time::interval(StdDuration::from_secs(1));
            loop {
                tick.tick().await;
                let Some(manager)=weak.upgrade() else { break; };
                manager.reconcile_static_ips_logged();
            }
        });
    }
    fn static_users(&self, key: &str) -> Vec<u64> {
        self.entries.lock().iter().filter(|e| e.credentials.proxy_url.as_deref().and_then(endpoint_key).as_deref() == Some(key))
            .map(|e| e.id).collect()
    }
    fn static_is_global(&self, key: &str) -> bool {
        self.proxy.lock().as_ref().and_then(|p| endpoint_key(&p.url)).as_deref() == Some(key)
    }
    pub fn static_ip_list(&self) -> StaticIpResponse {
        self.reconcile_static_ips_logged();
        let store = self.static_ips.lock();
        let entries = store.inventory.entries.iter().map(|e| {
            let key = endpoint_key(&e.url).expect("validated inventory"); let users = self.static_users(&key);
            let incompatible_legacy = self.entries.lock().iter().any(|c| users.contains(&c.id) && !c.credentials.static_ip_required && !eligible(&store.inventory, &c.credentials));
            let status = if self.static_is_global(&key) { "global" } else if users.len() > 1 || incompatible_legacy { "conflict" }
                else if !users.is_empty() && self.entries.lock().iter().any(|c| users.contains(&c.id) && (c.disabled || !eligible(&store.inventory, &c.credentials))) { "releasing" }
                else if !users.is_empty() { "occupied" } else if store.reserved.contains(&key) { "reserved" }
                else if !e.enabled { "disabled" } else { "free" };
            StaticIpView { id:e.id, url:e.url.clone(), username:e.username.clone(), has_password:e.password.is_some(),
                label:e.label.clone(), enabled:e.enabled, status, credential_ids:users }
        }).collect();
        StaticIpResponse { auto_assign: store.inventory.auto_assign, allowed_providers: store.inventory.allowed_providers.clone(),
            waiting_credential_ids: self.entries.lock().iter().filter(|e| !e.disabled && e.credentials.static_ip_required && e.credentials.proxy_url.is_none()).map(|e| e.id).collect(), entries }
    }
    pub fn static_ip_import(&self, inputs: Vec<StaticIpInput>) -> anyhow::Result<()> {
        if inputs.is_empty() || inputs.len() > 1000 { return Err(invalid("每次请导入 1～1000 条静态 IP")); }
        let mut store = self.static_ips.lock(); let mut next = store.inventory.clone();
        let credentials = self.entries.lock();
        next.next_id = next.next_id.max(next.entries.iter().map(|e| e.id).max().unwrap_or(0));
        for input in inputs {
            next.next_id = next.next_id.checked_add(1).ok_or_else(|| invalid("静态 IP ID 已耗尽"))?;
            let entry = input.into_entry(next.next_id)?; let key = endpoint_key(&entry.url).unwrap();
            if next.entries.iter().any(|e| endpoint_key(&e.url).as_ref() == Some(&key)) { return Err(invalid("存在重复的 IP 地址，本次未导入任何条目")); }
            if self.static_is_global(&key) { return Err(invalid("该 IP 正作为全局代理使用，请先清除全局绑定")); }
            if store.ordinary_imports.contains_key(&key) { return Err(invalid("该 IP 正被导入中的账号使用，请完成后再登记；本次未导入任何条目")); }
            if store.active_proxy_hosts.get(&key).is_some_and(|weak| weak.strong_count() > 0) {
                return Err(invalid("该 IP 仍有进行中的请求，请结束后再登记；本次未导入任何条目"));
            }
            let users: Vec<_> = credentials.iter().filter(|c| c.credentials.proxy_url.as_deref().and_then(endpoint_key).as_ref() == Some(&key)).collect();
            if users.len() > 1 { return Err(invalid("该 IP 已被多个账号使用，请先解除冲突；本次未导入任何条目")); }
            if users.iter().any(|c| !eligible(&next, &c.credentials)) {
                return Err(invalid("该 IP 已被不允许使用静态 IP 的账号作为普通代理使用；原代理保留，本次未导入任何条目"));
            }
            if users.iter().any(|c| proxy_busy(c)) {
                return Err(invalid("该 IP 的现有账号仍有请求进行中，请结束后再导入；本次未导入任何条目"));
            }
            next.entries.push(entry);
        }
        store.save(next)
    }
    #[cfg(test)]
    pub fn static_ip_auto_assign(&self, enabled: bool) -> anyhow::Result<()> {
        self.static_ip_config(Some(enabled), None)
    }
    pub fn static_ip_enabled(&self, id: u64, enabled: bool) -> anyhow::Result<()> {
        let mut store = self.static_ips.lock(); let mut next = store.inventory.clone();
        let e = next.entries.iter_mut().find(|e| e.id==id).ok_or_else(|| invalid("静态 IP 不存在"))?;
        // Availability only affects new allocation; existing bindings remain stable.
        e.enabled=enabled; store.save(next)
    }
    pub fn static_ip_delete(&self, id: u64) -> anyhow::Result<()> {
        let mut store = self.static_ips.lock(); let mut next = store.inventory.clone();
        let e = next.entries.iter().find(|e| e.id==id).ok_or_else(|| invalid("静态 IP 不存在"))?;
        let key = endpoint_key(&e.url).unwrap();
        if !self.static_users(&key).is_empty() || store.reserved.contains(&key) || self.static_is_global(&key) {
            return Err(invalid("静态 IP 已被占用或正在分配，不能删除；请先解除账号绑定"));
        }
        next.entries.retain(|e| e.id!=id); store.save(next)
    }
    /// Called before token validation so even the first refresh uses the assigned proxy.
    fn reserve_ordinary_import(self: &Arc<Self>, store: &mut StaticIpStore, cred: &KiroCredentials) -> anyhow::Result<Option<Reservation>> {
        let Some(proxy) = cred.effective_proxy(self.proxy.lock().as_ref()) else { return Ok(None); };
        if store.contains(Some(&proxy.url)) { return Err(invalid("普通代理与静态 IP 库冲突，账号未导入")); }
        let key = endpoint_key(&proxy.url);
        if let Some(key) = &key { *store.ordinary_imports.entry(key.clone()).or_default() += 1; }
        Ok(Some(Reservation { manager:self.clone(), key, exclusive:false, proxy }))
    }
    pub(super) fn prepare_static_ip(self: &Arc<Self>, cred: &mut KiroCredentials) -> anyhow::Result<Option<Reservation>> {
        self.reconcile_static_ips()?;
        let mut store = self.static_ips.lock();
        // Imported lifecycle flags are never authoritative.
        cred.static_ip_required = false;
        if !eligible(&store.inventory, cred) {
            if store.contains(cred.proxy_url.as_deref()) { return Err(invalid("该凭据类型不允许使用静态 IP")); }
            return self.reserve_ordinary_import(&mut store, cred);
        }
        let explicit = cred.proxy_url.as_deref().filter(|s| !s.trim().is_empty());
        let selected = if let Some(url) = explicit {
            let key = endpoint_key(url);
            store.inventory.entries.iter().find(|e| endpoint_key(&e.url)==key).cloned()
        } else if store.inventory.auto_assign {
            Some(store.inventory.entries.iter().find(|e| {
                let key=endpoint_key(&e.url).unwrap();
                e.enabled && !store.reserved.contains(&key) && self.static_users(&key).is_empty() && !self.static_is_global(&key)
            }).cloned().ok_or_else(|| invalid("没有空闲静态 IP，账号未导入；请补充 IP 后重试"))?)
        } else { None };
        let Some(ip) = selected else { return self.reserve_ordinary_import(&mut store, cred); };
        let key=endpoint_key(&ip.url).unwrap();
        if !ip.enabled || store.reserved.contains(&key) || !self.static_users(&key).is_empty() || self.static_is_global(&key) {
            return Err(invalid("指定静态 IP 已被占用、禁用或正在分配，请选择空闲 IP"));
        }
        cred.static_ip_required=true;
        cred.proxy_url=Some(ip.url); cred.proxy_username=ip.username; cred.proxy_password=ip.password;
        store.reserved.insert(key.clone());
        let proxy = cred.effective_proxy(None).expect("static inventory has a proxy URL");
        Ok(Some(Reservation { manager:self.clone(), key:Some(key), exclusive:true, proxy }))
    }
    pub(crate) fn guard_static_global_proxy(&self, url: Option<&str>) -> anyhow::Result<parking_lot::MutexGuard<'_, StaticIpStore>> {
        let store=self.static_ips.lock();
        if store.contains(url) { return Err(invalid("静态 IP 为账号独占资源，不能设置为全局代理")); }
        Ok(store)
    }
    pub fn static_ip_bind(&self, credential_id: u64, ip_id: Option<u64>) -> anyhow::Result<()> {
        let store=self.static_ips.lock();
        let before=self.entries.lock().iter().find(|e| e.id==credential_id).map(|e| e.credentials.clone())
            .ok_or_else(|| invalid("凭据不存在"))?;
        let mut next=before.clone();
        if let Some(id)=ip_id {
            let ip=store.inventory.entries.iter().find(|e| e.id==id).ok_or_else(|| invalid("静态 IP 不存在"))?;
            next.proxy_url=Some(ip.url.clone());
        } else {
            if !store.contains(before.proxy_url.as_deref()) { return Err(invalid("此账号没有绑定静态 IP")); }
            next.proxy_url=None;
        }
        self.validate_static_proxy_change(&store,credential_id,&before,&mut next)?;
        {
            let _persist=self.persist_lock.lock();
            let mut entries=self.entries.lock();
            let entry=entries.iter().find(|e| e.id==credential_id).ok_or_else(|| invalid("凭据不存在"))?;
            if proxy_busy(entry) { return Err(invalid("账号仍有进行中的请求，请结束后再更换静态 IP")); }
            let mut candidate=entry.credentials_snapshot();
            candidate.proxy_url=next.proxy_url; candidate.proxy_username=next.proxy_username; candidate.proxy_password=next.proxy_password;
            candidate.static_ip_required=store.contains(candidate.proxy_url.as_deref());
            let snapshot:Vec<_>=entries.iter().map(|e| if e.id==credential_id { candidate.clone() } else { e.credentials_snapshot() }).collect();
            self.save_traffic_credentials(&snapshot)?;
            entries.iter_mut().find(|e| e.id==credential_id).unwrap().credentials=candidate;
        }
        self.invalidate_model_cache(credential_id);
        Ok(())
    }
    pub(super) fn update_static_credential(
        &self,
        store: &StaticIpStore,
        id: u64,
        email: Option<Option<String>>,
        proxy_url: Option<Option<String>>,
        proxy_username: Option<Option<String>>,
        proxy_password: Option<Option<String>>,
        groups: Option<Vec<String>>,
        source_channel: Option<Option<String>>,
        metadata: Option<crate::kiro::model::credentials::CredentialMetadata>,
    ) -> anyhow::Result<()> {
        let before = self.entries.lock().iter().find(|e| e.id==id)
            .map(|e| e.credentials.clone()).ok_or_else(|| anyhow::anyhow!("凭据不存在: {id}"))?;
        #[cfg(test)] tests::run_edit_snapshot_hook();
        let mut next = before.clone();
        if let Some(v)=proxy_url { next.proxy_url=v.filter(|s| !s.is_empty()); }
        // Clear only inherited authentication, before applying explicit new values.
        if before.proxy_url != next.proxy_url { next.proxy_username=None; next.proxy_password=None; }
        if let Some(v)=proxy_username { next.proxy_username=v.filter(|s| !s.is_empty()); }
        if let Some(v)=proxy_password { next.proxy_password=v.filter(|s| !s.is_empty()); }
        // Forms submit unchanged proxy fields alongside metadata. Their presence
        // alone must not rebind an account or reject an in-flight metadata edit.
        let proxy_changed = before.proxy_url != next.proxy_url
            || before.proxy_username != next.proxy_username || before.proxy_password != next.proxy_password;
        let managed = if proxy_changed { self.validate_static_proxy_change(store,id,&before,&mut next)? }
            else { before.static_ip_required || store.contains(before.proxy_url.as_deref()) };
        {
            let _persist = managed.then(|| self.persist_lock.lock());
            let mut entries = self.entries.lock();
            let entry=entries.iter().find(|e| e.id==id).ok_or_else(|| anyhow::anyhow!("凭据不存在: {id}"))?;
            if managed && proxy_changed && proxy_busy(entry) {
                return Err(static_ips::StaticIpError("账号仍有进行中的请求，请结束后再更换静态 IP".into()).into());
            }
            // Apply only submitted metadata to the latest entry: balance queries
            // may have discovered an email since the proxy validation snapshot.
            let mut candidate=entry.credentials_snapshot();
            if let Some(v)=email { candidate.email=v.filter(|s| !s.is_empty()); }
            if let Some(g)=groups { candidate.groups=g.into_iter().map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect(); }
            if let Some(v)=source_channel { candidate.source_channel=v.map(|s| s.trim().to_string()).filter(|s| !s.is_empty()); }
            if let Some(v)=metadata { candidate.metadata=v; }
            candidate.proxy_url=next.proxy_url;
            candidate.proxy_username=next.proxy_username; candidate.proxy_password=next.proxy_password;
            if proxy_changed {
                // The editor submits an unchanged empty URL even for metadata-only
                // edits. Preserve waiting protection until an actual proxy change.
                candidate.static_ip_required = if before.proxy_url.is_none() && candidate.proxy_url.is_none() {
                    before.static_ip_required
                } else { store.contains(candidate.proxy_url.as_deref()) };
            }
            if managed {
                let snapshot: Vec<_> = entries.iter().map(|e| if e.id==id { candidate.clone() } else { e.credentials_snapshot() }).collect();
                self.save_traffic_credentials(&snapshot)?;
            }
            entries.iter_mut().find(|e| e.id==id).unwrap().credentials=candidate;
        }
        if proxy_changed { self.invalidate_model_cache(id); }
        if !managed { self.persist_credentials()?; }
        Ok(())
    }

    pub(super) fn validate_static_proxy_change(&self, store: &StaticIpStore, id: u64, before: &KiroCredentials, next: &mut KiroCredentials) -> anyhow::Result<bool> {
        let managed=before.static_ip_required || store.contains(before.proxy_url.as_deref()) || store.contains(next.proxy_url.as_deref());
        if !managed { return Ok(false); }
        if let Some(key)=next.proxy_url.as_deref().and_then(endpoint_key) {
            if let Some(ip)=store.inventory.entries.iter().find(|e| endpoint_key(&e.url).as_ref()==Some(&key)) {
                if !eligible(&store.inventory, next) { return Err(invalid("该凭据类型不允许使用静态 IP")); }
                if self.entries.lock().iter().any(|e| e.id==id && e.disabled) { return Err(invalid("请先启用账号，再绑定静态 IP")); }
                let unchanged=before.proxy_url.as_deref().and_then(endpoint_key).as_ref()==Some(&key);
                if (!ip.enabled && !unchanged) || store.reserved.contains(&key) || self.static_users(&key).iter().any(|other| *other!=id) || self.static_is_global(&key) {
                    return Err(invalid("静态 IP 已被占用、禁用或正在分配"));
                }
                next.proxy_url=Some(ip.url.clone()); next.proxy_username=ip.username.clone(); next.proxy_password=ip.password.clone();
            }
        }
        if next.proxy_url.is_none() || next.proxy_url.as_deref().is_some_and(|url| url.eq_ignore_ascii_case("direct")) {
            next.proxy_username=None; next.proxy_password=None;
        }
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    // Deterministically schedule a quota/refresh update between the edit's snapshot
    // and commit; thread-local isolation keeps parallel tests independent.
    thread_local! {
        static EDIT_SNAPSHOT_HOOK: std::cell::RefCell<Option<Box<dyn FnOnce()>>> = std::cell::RefCell::new(None);
    }
    pub(super) fn run_edit_snapshot_hook() {
        let hook = EDIT_SNAPSHOT_HOOK.with(|slot| slot.borrow_mut().take());
        if let Some(hook) = hook { hook(); }
    }
    fn input(host: &str) -> StaticIpInput {
        StaticIpInput { host:host.into(),port:3000,protocol:"http".into(),username:Some("vendor-user".into()),password:Some("test-only-password".into()),label:None }
    }
    fn account(name: &str) -> KiroCredentials {
        KiroCredentials { refresh_token:Some(name.repeat(120)), access_token:Some("test-access".into()), expires_at:Some("2099-01-01T00:00:00Z".into()), auth_method:Some("social".into()), provider:Some("Github".into()), ..Default::default() }
    }
    fn manager(path: Option<PathBuf>, creds: Vec<KiroCredentials>) -> Arc<MultiTokenManager> {
        Arc::new(MultiTokenManager::new(Config::default(),creds,None,path,true).unwrap())
    }
    impl MultiTokenManager {
        async fn insert_test_personal(self: &Arc<Self>, mut c: KiroCredentials) -> anyhow::Result<u64> {
            let _reservation=self.prepare_static_ip(&mut c)?;
            let id=self.next_id.fetch_add(1,Ordering::Relaxed); c.id=Some(id);
            let fixture=manager(None,vec![c]);
            let entry=fixture.entries.lock().pop().unwrap();
            let _persist=self.persist_lock.lock();
            let mut entries=self.entries.lock();
            let mut snapshot:Vec<_>=entries.iter().map(CredentialEntry::credentials_snapshot).collect();
            snapshot.push(entry.credentials_snapshot()); self.save_traffic_credentials(&snapshot)?;
            entries.push(entry); Ok(id)
        }
    }
    fn temp_dir() -> PathBuf {
        let p=std::env::temp_dir().join(format!("kiro-static-{}",uuid::Uuid::new_v4()));std::fs::create_dir(&p).unwrap();p
    }
    #[test]
    fn batch_is_atomic_and_same_ip_cannot_be_registered_with_another_port() {
        let m=manager(None,vec![]);
        assert!(m.static_ip_import(vec![input("192.0.2.10"),input("invalid-host")]).is_err());
        assert!(m.static_ip_list().entries.is_empty());
        m.static_ip_import(vec![input("192.0.2.10")]).unwrap();
        let mut duplicate=input("192.0.2.10");duplicate.port=4000;
        assert!(m.static_ip_import(vec![duplicate]).is_err());
        assert_eq!(m.static_ip_list().entries.len(),1);
    }
    #[test]
    fn reservation_precedes_network_and_drop_releases_on_failure_or_cancellation() {
        let m=manager(None,vec![]);m.static_ip_import(vec![input("192.0.2.10")]).unwrap();m.static_ip_auto_assign(true).unwrap();
        let mut c=account("first");let reservation=m.prepare_static_ip(&mut c).unwrap();
        assert_eq!(c.proxy_url.as_deref(),Some("http://192.0.2.10:3000"));
        assert_eq!(c.proxy_username.as_deref(),Some("vendor-user"));assert_eq!(c.proxy_password.as_deref(),Some("test-only-password"));
        assert_eq!(m.static_ip_list().entries[0].status,"reserved");
        assert!(m.prepare_static_ip(&mut account("second")).is_err());
        assert!(m.static_ip_delete(1).is_err());
        drop(reservation);assert_eq!(m.static_ip_list().entries[0].status,"free");
    }
    #[tokio::test(flavor="multi_thread",worker_threads=4)]
    async fn simultaneous_imports_are_exclusive_and_exhaustion_does_not_insert_accounts() {
        let m=manager(None,vec![]);m.static_ip_import(vec![input("192.0.2.10"),input("192.0.2.11")]).unwrap();m.static_ip_auto_assign(true).unwrap();
        let jobs=(0..12).map(|i| { let m=m.clone();tokio::spawn(async move {m.insert_test_personal(account(&format!("test-key-{i}"))).await}) });
        let results=futures::future::join_all(jobs).await;
        assert_eq!(results.into_iter().filter(|r|r.as_ref().unwrap().is_ok()).count(),2);
        assert_eq!(m.snapshot().total,2);
        let view=m.static_ip_list();assert!(view.entries.iter().all(|e|e.status=="occupied" && e.credential_ids.len()==1));
        assert_ne!(view.entries[0].credential_ids,view.entries[1].credential_ids);
    }
    #[tokio::test(flavor="multi_thread")]
    async fn disabled_account_releases_and_cannot_reuse_new_owners_ip() {
        let m=manager(None,vec![]); m.static_ip_import(vec![input("192.0.2.10")]).unwrap(); m.static_ip_auto_assign(true).unwrap();
        let first=m.insert_test_personal(account("first")).await.unwrap();
        m.set_disabled(first,true).unwrap();
        assert_eq!(m.static_ip_list().entries[0].status,"free");
        let second=m.insert_test_personal(account("second")).await.unwrap();
        m.set_disabled(first,false).unwrap();
        assert_eq!(m.static_ip_list().entries[0].credential_ids,vec![second]);
        assert_eq!(m.static_ip_list().waiting_credential_ids,vec![first]);
        assert!(m.try_acquire_permit(first).is_none());
        assert!(m.static_ip_lease(first).is_err());
        m.delete_credential(second).unwrap(); m.reconcile_static_ips().unwrap();
        assert_eq!(m.static_ip_list().entries[0].credential_ids,vec![first]);
        assert!(m.try_acquire_permit(first).is_some());
    }
    #[tokio::test(flavor="multi_thread")]
    async fn disabled_auto_assignment_and_explicit_non_pool_proxies_keep_legacy_behavior() {
        let m=manager(None,vec![]);m.static_ip_import(vec![input("192.0.2.10")]).unwrap();
        let first=m.insert_test_personal(account("first")).await.unwrap();assert!(m.snapshot().entries.iter().find(|e|e.id==first).unwrap().proxy_url.is_none());
        m.static_ip_auto_assign(true).unwrap();let mut c=account("second");c.proxy_url=Some("direct".into());m.insert_test_personal(c).await.unwrap();
        assert_eq!(m.static_ip_list().entries[0].status,"free");
    }
    #[tokio::test(flavor="multi_thread")]
    async fn restart_retains_assignments_and_does_not_reuse_occupied_ip() {
        let dir=temp_dir();let path=dir.join("credentials.json");
        let m=manager(Some(path.clone()),vec![]);m.static_ip_import(vec![input("192.0.2.10"),input("192.0.2.11")]).unwrap();m.static_ip_auto_assign(true).unwrap();
        let id=m.insert_test_personal(account("first")).await.unwrap();
        let creds=serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        let reopened=manager(Some(path),creds);assert!(reopened.static_ip_list().auto_assign);
        assert_eq!(reopened.static_ip_list().entries[0].credential_ids,vec![id]);
        reopened.insert_test_personal(account("second")).await.unwrap();
        assert!(reopened.static_ip_list().entries.iter().all(|e| e.status=="occupied"));
        std::fs::remove_dir_all(dir).unwrap();
    }
    #[tokio::test(flavor="multi_thread")]
    async fn account_edit_cannot_share_ip_and_move_clears_old_auth_on_unbind() {
        let m=manager(None,vec![]);m.static_ip_import(vec![input("192.0.2.10"),input("192.0.2.11")]).unwrap();
        let first=m.insert_test_personal(account("first")).await.unwrap();let second=m.insert_test_personal(account("second")).await.unwrap();
        m.static_ip_bind(first,Some(1)).unwrap();
        assert!(m.update_credential(second,None,Some(Some("socks5://x:y@192.0.2.10:4000".into())),None,None,None,None,None).is_err());
        m.static_ip_bind(first,Some(2)).unwrap();assert_eq!(m.static_ip_list().entries[0].status,"free");
        m.static_ip_bind(first,None).unwrap();let entries=m.entries.lock();let c=&entries.iter().find(|e|e.id==first).unwrap().credentials;
        assert!(c.proxy_url.is_none() && c.proxy_username.is_none() && c.proxy_password.is_none());
    }
    #[tokio::test(flavor="multi_thread")]
    async fn ongoing_generation_blocks_rebinding_and_deletion() {
        let m=manager(None,vec![]);m.static_ip_import(vec![input("192.0.2.10")]).unwrap();m.static_ip_auto_assign(true).unwrap();
        let id=m.insert_test_personal(account("first")).await.unwrap();let permit=m.try_acquire_permit(id).unwrap();
        assert!(m.static_ip_bind(id,None).is_err());assert!(m.delete_credential(id).is_err());
        assert_eq!(m.static_ip_list().entries[0].status,"occupied");drop(permit);m.static_ip_bind(id,None).unwrap();
    }
    #[test]
    fn importing_conflicting_manual_bindings_and_global_proxy_is_rejected() {
        let mut a=account("first");a.id=Some(1);a.proxy_url=Some("192.0.2.10:3000".into());
        let mut b=account("second");b.id=Some(2);b.proxy_url=Some("http://192.0.2.10:4000".into());
        let m=manager(None,vec![a,b]);
        assert!(m.static_ip_import(vec![input("192.0.2.11"),input("192.0.2.10")]).is_err());
        assert!(m.static_ip_list().entries.is_empty());
        assert_eq!(m.clone_credential(1).unwrap().proxy_url.as_deref(),Some("192.0.2.10:3000"));
        assert_eq!(m.clone_credential(2).unwrap().proxy_url.as_deref(),Some("http://192.0.2.10:4000"));
        m.set_global_proxy(Some(ProxyConfig::new("http://192.0.2.12:3000")));
        assert!(m.static_ip_import(vec![input("192.0.2.12")]).is_err());
    }
    #[tokio::test(flavor="multi_thread")]
    async fn persistence_failure_does_not_insert_account_or_leak_reservation() {
        let dir=temp_dir();let path=dir.join("credentials.json");let m=manager(Some(path.clone()),vec![]);
        m.static_ip_import(vec![input("192.0.2.10")]).unwrap();m.static_ip_auto_assign(true).unwrap();
        std::fs::create_dir(&path).unwrap();assert!(m.insert_test_personal(account("first")).await.is_err());
        assert_eq!(m.snapshot().total,0);assert_eq!(m.static_ip_list().entries[0].status,"free");std::fs::remove_dir_all(dir).unwrap();
    }
    #[tokio::test(flavor="multi_thread")]
    async fn failed_unbind_or_delete_preserves_occupancy_and_credentials() {
        let dir=temp_dir();let path=dir.join("credentials.json");let m=manager(Some(path.clone()),vec![]);
        m.static_ip_import(vec![input("192.0.2.10")]).unwrap();m.static_ip_auto_assign(true).unwrap();let id=m.insert_test_personal(account("first")).await.unwrap();
        std::fs::remove_file(&path).unwrap();std::fs::create_dir(&path).unwrap();
        assert!(m.static_ip_bind(id,None).is_err());assert!(m.delete_credential(id).is_err());
        assert_eq!(m.static_ip_list().entries[0].credential_ids,vec![id]);assert_eq!(m.snapshot().total,1);std::fs::remove_dir_all(dir).unwrap();
    }
    #[tokio::test(flavor="multi_thread")]
    async fn delayed_token_refresh_cannot_restore_a_released_static_binding() {
        let m=manager(None,vec![]);m.static_ip_import(vec![input("192.0.2.10")]).unwrap();
        let id=m.insert_test_personal(account("first")).await.unwrap();m.static_ip_bind(id,Some(1)).unwrap();
        let source=m.entries.lock().iter().find(|e|e.id==id).unwrap().credentials.clone();
        m.static_ip_bind(id,None).unwrap();
        {
            let mut entries=m.entries.lock();let entry=entries.iter_mut().find(|e|e.id==id).unwrap();
            entry.replace_refreshed_credentials(&source,source.clone());
            assert!(entry.credentials.proxy_url.is_none());assert!(entry.credentials.proxy_password.is_none());
        }
        assert_eq!(m.static_ip_list().entries[0].status,"free");
    }
    #[test]
    fn failed_inventory_write_keeps_memory_unchanged_and_listing_omits_password() {
        let dir=temp_dir();let m=manager(Some(dir.join("credentials.json")),vec![]);
        m.static_ip_import(vec![input("192.0.2.10")]).unwrap();
        let text=serde_json::to_string(&m.static_ip_list()).unwrap();assert!(!text.contains("test-only-password"));
        let path=dir.join("static_ips.json");std::fs::remove_file(&path).unwrap();std::fs::create_dir(&path).unwrap();
        assert!(m.static_ip_auto_assign(true).is_err());assert!(!m.static_ip_list().auto_assign);
        assert!(m.static_ip_delete(1).is_err());assert_eq!(m.static_ip_list().entries.len(),1);std::fs::remove_dir_all(dir).unwrap();
    }

#[test]
fn provider_policy_defaults_and_enterprise_cannot_bypass() {
    let m=manager(None,vec![]); m.static_ip_import(vec![input("192.0.2.10")]).unwrap(); m.static_ip_auto_assign(true).unwrap();
    assert_eq!(m.static_ip_list().allowed_providers,vec!["github","google"]);
    for (method, provider) in [("idc","Enterprise"),("idc","iam_sso"),("external_idp","Google"),("social","Enterprise"),("social",""),("api_key","Github")] {
        let mut c=account("x"); c.auth_method=Some(method.into()); c.provider=Some(provider.into());
        assert!(m.prepare_static_ip(&mut c).unwrap().is_none()); assert!(c.proxy_url.is_none());
        c.proxy_url=Some("http://192.0.2.10:3000".into()); assert!(m.prepare_static_ip(&mut c).is_err());
    }
    assert!(m.static_ip_config(None,Some(vec!["enterprise".into()])).is_err());
    m.static_ip_config(None,Some(vec!["google".into(),"builderid".into()])).unwrap();
    assert!(m.prepare_static_ip(&mut account("github")).unwrap().is_none());
    let mut c=account("google"); c.provider=Some("Google".into()); assert!(m.prepare_static_ip(&mut c).unwrap().is_some());
    let mut c=account("builder"); c.provider=Some("BuilderID".into()); c.auth_method=Some("idc".into()); assert!(m.prepare_static_ip(&mut c).unwrap().is_some());
}
#[tokio::test]
async fn all_disable_causes_release_but_temporary_throttle_does_not() {
    for reason in 0..5 {
        let m=manager(None,vec![]);m.static_ip_import(vec![input("192.0.2.10")]).unwrap();m.static_ip_auto_assign(true).unwrap();
        let id=m.insert_test_personal(account("account")).await.unwrap();
        m.report_account_throttled_for_request(id,StdDuration::from_secs(30),None,None);
        assert_eq!(m.static_ip_list().entries[0].status,"occupied");
        match reason {
            0 => {m.report_suspended(id);},
            1 => {m.report_refresh_token_invalid(id);},
            2 => {m.report_quota_exhausted(id);},
            3 => {m.disable_quota_exceeded(id).unwrap();},
            _ => {let identity=m.balance_identity(id).unwrap();m.apply_traffic_quota(id,&identity,|_| true).unwrap();},
        }
        assert_eq!(m.static_ip_list().entries[0].status,"free");
        let c=m.clone_credential(id).unwrap();assert!(c.proxy_url.is_none() && c.proxy_username.is_none() && c.proxy_password.is_none());
        assert!(c.static_ip_required);
    }
}
#[tokio::test]
async fn in_flight_generation_and_admin_probe_delay_release() {
    let m=manager(None,vec![]);m.static_ip_import(vec![input("192.0.2.10")]).unwrap();m.static_ip_auto_assign(true).unwrap();
    let id=m.insert_test_personal(account("account")).await.unwrap();
    let lease=m.static_ip_lease(id).unwrap();let permit=m.try_acquire_permit(id).unwrap();
    m.report_suspended(id);assert_eq!(m.static_ip_list().entries[0].status,"releasing");
    assert!(m.prepare_static_ip(&mut account("new")).is_err());
    drop(lease);assert_eq!(m.static_ip_list().entries[0].status,"releasing");
    drop(permit);assert_eq!(m.static_ip_list().entries[0].status,"free");
    // Explicit disabled-account quota probe may borrow a FREE address, but not another owner's.
    let probe=m.static_ip_lease(id).unwrap();assert_eq!(m.static_ip_list().entries[0].status,"releasing");
    drop(probe);assert_eq!(m.static_ip_list().entries[0].status,"free");
}
#[tokio::test]
async fn request_context_pins_ip_until_response_permit_takes_over() {
    let m=manager(None,vec![]);m.static_ip_import(vec![input("192.0.2.10")]).unwrap();m.static_ip_auto_assign(true).unwrap();
    let id=m.insert_test_personal(account("account")).await.unwrap();
    let c=m.clone_credential(id).unwrap();let ctx=m.try_ensure_token(id,&c).await.unwrap();
    assert!(m.static_ip_bind(id,None).is_err());
    let permit=m.try_acquire_permit(id).unwrap();m.set_disabled(id,true).unwrap();
    drop(ctx);assert_eq!(m.static_ip_list().entries[0].status,"releasing");
    drop(permit);assert_eq!(m.static_ip_list().entries[0].status,"free");
}
#[tokio::test]
async fn closing_type_releases_binding_and_blocks_manual_editor_and_import() {
    let m=manager(None,vec![]);m.static_ip_import(vec![input("192.0.2.10")]).unwrap();m.static_ip_auto_assign(true).unwrap();
    let id=m.insert_test_personal(account("account")).await.unwrap();
    let lease=m.static_ip_lease(id).unwrap();assert!(m.static_ip_config(None,Some(vec![])).is_err());drop(lease);
    m.static_ip_config(None,Some(vec![])).unwrap();
    assert_eq!(m.static_ip_list().entries[0].status,"free");assert!(!m.clone_credential(id).unwrap().static_ip_required);
    assert!(m.static_ip_bind(id,Some(1)).is_err());
    assert!(m.update_credential(id,None,Some(Some("192.0.2.10:3000".into())),None,None,None,None,None).is_err());
    assert!(m.prepare_static_ip(&mut account("new")).unwrap().is_none());
}
#[tokio::test(flavor="multi_thread")]
async fn release_failure_keeps_ownership_until_retry_and_restart_keeps_waiting() {
    let dir=temp_dir();let path=dir.join("credentials.json");let m=manager(Some(path.clone()),vec![]);
    m.static_ip_import(vec![input("192.0.2.10")]).unwrap();m.static_ip_auto_assign(true).unwrap();
    let first=m.insert_test_personal(account("first")).await.unwrap();
    std::fs::remove_file(&path).unwrap();std::fs::create_dir(&path).unwrap();
    m.report_suspended(first);
    assert_eq!(m.static_ip_list().entries[0].credential_ids,vec![first]);
    assert!(m.prepare_static_ip(&mut account("new")).is_err());
    std::fs::remove_dir(&path).unwrap();m.reconcile_static_ips().unwrap();
    let second=m.insert_test_personal(account("second")).await.unwrap();m.set_disabled(first,false).unwrap();
    let creds=serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    let reopened=manager(Some(path),creds);
    assert_eq!(reopened.static_ip_list().waiting_credential_ids,vec![first]);
    assert_eq!(reopened.static_ip_list().entries[0].credential_ids,vec![second]);
    assert!(reopened.try_acquire_permit(first).is_none());std::fs::remove_dir_all(dir).unwrap();
}
#[tokio::test]
async fn api_key_import_never_consumes_personal_ip() {
    let m=manager(None,vec![]);m.static_ip_import(vec![input("192.0.2.10")]).unwrap();m.static_ip_auto_assign(true).unwrap();
    let id=m.add_credential(KiroCredentials { auth_method:Some("api_key".into()),kiro_api_key:Some("test-api-key".into()), ..Default::default() }).await.unwrap();
    assert!(m.clone_credential(id).unwrap().proxy_url.is_none());assert_eq!(m.static_ip_list().entries[0].status,"free");
}

#[tokio::test(flavor="multi_thread")]
async fn provider_change_rolls_back_if_binding_cannot_be_persisted() {
    let dir=temp_dir();let path=dir.join("credentials.json");let m=manager(Some(path.clone()),vec![]);
    m.static_ip_import(vec![input("192.0.2.10")]).unwrap();m.static_ip_auto_assign(true).unwrap();
    let id=m.insert_test_personal(account("first")).await.unwrap();
    std::fs::remove_file(&path).unwrap();std::fs::create_dir(&path).unwrap();
    assert!(m.static_ip_config(None,Some(vec!["google".into()])).is_err());
    assert!(m.static_ip_list().allowed_providers.contains(&"github".into()));
    assert_eq!(m.static_ip_list().entries[0].credential_ids,vec![id]);
    let config:Inventory=serde_json::from_slice(&std::fs::read(dir.join("static_ips.json")).unwrap()).unwrap();
    assert!(config.allowed_providers.contains(&"github".into()));std::fs::remove_dir_all(dir).unwrap();
}
#[test]
fn type_policy_cannot_change_during_an_import_reservation() {
    let m=manager(None,vec![]);m.static_ip_import(vec![input("192.0.2.10")]).unwrap();m.static_ip_auto_assign(true).unwrap();
    let reservation=m.prepare_static_ip(&mut account("new")).unwrap();
    assert!(m.static_ip_config(None,Some(vec![])).is_err());
    drop(reservation);m.static_ip_config(None,Some(vec![])).unwrap();
    assert!(m.static_ip_list().allowed_providers.is_empty());
}


    #[test]
    fn regression_import_should_not_silently_clear_enterprise_proxy() {
        let mut c=account("enterprise");c.id=Some(1);c.auth_method=Some("idc".into());c.provider=Some("Enterprise".into());
        c.proxy_url=Some("http://192.0.2.10:4000".into());c.proxy_username=Some("original-user".into());c.proxy_password=Some("original-password".into());
        let m=manager(None,vec![c]);
        let result=m.static_ip_import(vec![input("192.0.2.10")]);
        if result.is_ok(){m.reconcile_static_ips().unwrap();}
        assert_eq!(m.clone_credential(1).unwrap().proxy_url.as_deref(),Some("http://192.0.2.10:4000"),"Adding inventory must reject incompatible legacy use or preserve the enterprise ordinary proxy; it must not silently switch it to direct/global");
    }
    #[test]
    fn regression_switch_to_ordinary_proxy_preserves_new_auth() {
        let mut c=account("personal");c.id=Some(1);let m=manager(None,vec![c]);
        m.static_ip_import(vec![input("192.0.2.10")]).unwrap();m.static_ip_bind(1,Some(1)).unwrap();
        m.update_credential(1,None,Some(Some("http://192.0.2.20:3000".into())),Some(Some("new-user".into())),Some(Some("new-password".into())),None,None,None).unwrap();
        let c=m.clone_credential(1).unwrap();
        assert_eq!(c.proxy_url.as_deref(),Some("http://192.0.2.20:3000"));
        assert_eq!(c.proxy_username.as_deref(),Some("new-user"),"Explicit NEW proxy authentication must survive switching away from static inventory");
        assert_eq!(c.proxy_password.as_deref(),Some("new-password"));
    }
    #[tokio::test]
    async fn regression_conflicting_legacy_accounts_must_not_both_acquire_static_ip() {
        let mut a=account("first");a.id=Some(1);a.proxy_url=Some("http://192.0.2.10:3000".into());
        let mut b=account("second");b.id=Some(2);b.proxy_url=Some("http://192.0.2.10:4000".into());
        let m=manager(None,vec![a,b]);
        if m.static_ip_import(vec![input("192.0.2.10")]).is_err(){return;}
        let _=m.static_ip_list();
        let first=m.static_ip_lease(1);let second=m.static_ip_lease(2);
        let p1=m.try_acquire_permit(1);let p2=m.try_acquire_permit(2);
        assert!(!(first.is_ok() && second.is_ok() && p1.is_some() && p2.is_some()),"Inventory reports conflict but still admits two accounts on the same supposedly exclusive IP");
    }

#[test]
fn legacy_enterprise_overlap_keeps_proxy_and_cannot_be_allocated() {
    let mut c=account("enterprise");c.id=Some(1);c.auth_method=Some("idc".into());c.provider=Some("Enterprise".into());
    c.proxy_url=Some("http://192.0.2.10:4000".into());c.proxy_username=Some("ordinary-user".into());c.proxy_password=Some("ordinary-password".into());
    let m=manager(None,vec![c]);
    // Simulate inventory written by the previous version, before import checks existed.
    m.static_ips.lock().inventory.entries.push(input("192.0.2.10").into_entry(1).unwrap());
    m.static_ip_auto_assign(true).unwrap();
    assert_eq!(m.static_ip_list().entries[0].status,"conflict");
    let c=m.clone_credential(1).unwrap();
    assert_eq!(c.proxy_url.as_deref(),Some("http://192.0.2.10:4000"));
    assert_eq!(c.proxy_username.as_deref(),Some("ordinary-user"));
    assert_eq!(c.proxy_password.as_deref(),Some("ordinary-password"));
    assert!(!c.static_ip_required);
    assert!(m.static_ip_lease(1).is_ok());assert!(m.try_acquire_permit(1).is_some());
    assert!(m.prepare_static_ip(&mut account("new-personal")).is_err());
    m.set_disabled(1,true).unwrap();
    assert_eq!(m.clone_credential(1).unwrap().proxy_url.as_deref(),Some("http://192.0.2.10:4000"));
}
#[tokio::test]
async fn legacy_conflict_blocks_requests_without_disabling_accounts_and_unbind_recovers() {
    let mut a=account("first");a.id=Some(1);a.proxy_url=Some("http://192.0.2.10:3000".into());
    let mut b=account("second");b.id=Some(2);b.proxy_url=Some("http://192.0.2.10:4000".into());
    let m=manager(None,vec![a,b]);
    // Shared ordinary proxies remain supported until explicitly managed as static.
    assert!(m.try_acquire_permit(1).is_some());assert!(m.try_acquire_permit(2).is_some());
    m.static_ips.lock().inventory.entries.push(input("192.0.2.10").into_entry(1).unwrap());
    assert_eq!(m.static_ip_list().entries[0].status,"conflict");
    for id in [1,2] { assert!(m.static_ip_lease(id).is_err());assert!(m.try_acquire_permit(id).is_none()); }
    for _ in 0..4 {
        let err=m.acquire_context(None,None).await.err().expect("conflict must block network preparation");
        assert!(err.downcast_ref::<StaticIpError>().is_some());
    }
    for e in m.entries.lock().iter() {assert!(!e.disabled);assert_eq!(e.refresh_failure_count,0);}
    m.static_ip_bind(1,None).unwrap();
    assert_eq!(m.static_ip_list().entries[0].credential_ids,vec![2]);
    assert_eq!(m.clone_credential(2).unwrap().proxy_url.as_deref(),Some("http://192.0.2.10:4000"));
    assert!(m.static_ip_lease(2).is_ok());assert!(m.try_acquire_permit(2).is_some());
}
#[test]
fn import_rejects_incompatible_types_atomically_but_adopts_one_idle_personal_account() {
    for (method,provider) in [("idc","Enterprise"),("social","Unknown"),("idc","BuilderID")] {
        let mut c=account("legacy");c.id=Some(1);c.auth_method=Some(method.into());c.provider=Some(provider.into());c.proxy_url=Some("http://192.0.2.10:4000".into());
        let m=manager(None,vec![c]);
        assert!(m.static_ip_import(vec![input("192.0.2.11"),input("192.0.2.10")]).is_err());
        assert!(m.static_ip_list().entries.is_empty());
        assert_eq!(m.clone_credential(1).unwrap().proxy_url.as_deref(),Some("http://192.0.2.10:4000"));
    }
    let mut c=account("personal");c.id=Some(1);c.proxy_url=Some("http://192.0.2.10:3000".into());
    let m=manager(None,vec![c]);let lease=m.static_ip_lease(1).unwrap();
    assert!(m.static_ip_import(vec![input("192.0.2.10")]).is_err());drop(lease);
    m.static_ip_import(vec![input("192.0.2.10")]).unwrap();
    assert_eq!(m.static_ip_list().entries[0].credential_ids,vec![1]);
    assert!(m.clone_credential(1).unwrap().static_ip_required);
    assert!(m.try_acquire_permit(1).is_some());
}
#[test]
fn moving_to_ordinary_proxy_clears_only_old_auth_and_persists_explicit_values() {
    let dir=temp_dir();let path=dir.join("credentials.json");
    let mut c=account("personal");c.id=Some(1);let m=manager(Some(path.clone()),vec![c]);
    m.static_ip_import(vec![input("192.0.2.10")]).unwrap();
    for (username,password) in [(None,None),(Some(Some("new-user".into())),None),(None,Some(Some("new-password".into()))),(Some(None),Some(None)),(Some(Some("new-user".into())),Some(Some("new-password".into())))] {
        m.static_ip_bind(1,Some(1)).unwrap();
        m.update_credential(1,None,Some(Some("http://192.0.2.20:3000".into())),username.clone(),password.clone(),None,None,None).unwrap();
        let saved:Vec<KiroCredentials>=serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(saved[0].proxy_username,username.flatten());assert_eq!(saved[0].proxy_password,password.flatten());
        assert!(!saved[0].static_ip_required);assert_eq!(m.static_ip_list().entries[0].status,"free");
    }
    for url in [None,Some("direct".into())] {
        m.static_ip_bind(1,Some(1)).unwrap();
        m.update_credential(1,None,Some(url),Some(Some("unused-user".into())),Some(Some("unused-password".into())),None,None,None).unwrap();
        let c=m.clone_credential(1).unwrap();assert!(c.proxy_username.is_none() && c.proxy_password.is_none());
    }
    std::fs::remove_dir_all(dir).unwrap();
}


#[tokio::test]
async fn acceptance_conflicting_accounts_do_not_block_healthy_account() {
    let mut a=account("first");a.id=Some(1);a.priority=0;a.proxy_url=Some("http://192.0.2.10:3000".into());
    let mut b=account("second");b.id=Some(2);b.priority=1;b.proxy_url=Some("http://192.0.2.10:4000".into());
    let mut c=account("healthy");c.id=Some(3);c.priority=2;c.proxy_url=Some("http://192.0.2.11:3000".into());
    let m=manager(None,vec![a,b,c]);
    m.static_ips.lock().inventory.entries.push(input("192.0.2.10").into_entry(1).unwrap());
    m.static_ips.lock().inventory.entries.push(input("192.0.2.11").into_entry(2).unwrap());
    assert!(m.static_ip_lease(3).is_ok());assert!(m.try_acquire_permit(3).is_some());
    let result=m.acquire_context(None,None).await;
    assert_eq!(result.unwrap().id,3);
    for mode in ["priority", "balanced"] {
        m.set_load_balancing_mode(mode.into()).unwrap();
        m.session_affinity().set_enabled(true);m.bind_session("conflict-session",1);
        for _ in 0..32 {
            let (ctx,route)=m.acquire_context_routed(None,None,Some("conflict-session")).await.unwrap();
            assert_eq!(ctx.id,3);assert_eq!(route.sticky_outcome,StickyOutcome::MissUnavailable);
            assert_eq!(m.acquire_context(None,None).await.unwrap().id,3);
        }
    }
}
#[tokio::test]
async fn acceptance_metadata_edit_preserves_waiting_ip_guard() {
    let m=manager(None,vec![]);m.static_ip_import(vec![input("192.0.2.10")]).unwrap();m.static_ip_auto_assign(true).unwrap();
    let first=m.insert_test_personal(account("first")).await.unwrap();
    m.set_disabled(first,true).unwrap();
    let _second=m.insert_test_personal(account("second")).await.unwrap();
    m.set_disabled(first,false).unwrap();
    assert!(m.clone_credential(first).unwrap().static_ip_required);assert!(m.try_acquire_permit(first).is_none());
    // The real edit dialog always submits its unchanged, empty proxyUrl, even for a remark-only edit.
    m.update_credential(first,None,Some(Some("".into())),None,None,None,Some(Some("edited remark".into())),None).unwrap();
    m.reconcile_static_ips().unwrap();
    let c=m.clone_credential(first).unwrap();
    assert!(c.static_ip_required && m.try_acquire_permit(first).is_none(),"Saving a remark cleared the waiting guard and allowed direct/global traffic");
}

#[tokio::test]
async fn conflict_selection_keeps_group_boundaries_and_returns_typed_error() {
    let mut a=account("first");a.id=Some(1);a.groups=vec!["conflicted".into()];a.proxy_url=Some("http://192.0.2.10:3000".into());
    let mut b=account("second");b.id=Some(2);b.groups=a.groups.clone();b.proxy_url=a.proxy_url.clone();
    let mut c=account("healthy");c.id=Some(3);c.groups=vec!["healthy".into()];
    let m=manager(None,vec![a,b,c]);m.static_ips.lock().inventory.entries.push(input("192.0.2.10").into_entry(1).unwrap());
    for mode in ["priority","balanced"] {
        m.set_load_balancing_mode(mode.into()).unwrap();
        for _ in 0..4 {
            let error=m.acquire_context(None,Some("conflicted")).await.err().unwrap();
            assert!(error.downcast_ref::<StaticIpError>().is_some());
        }
        assert_eq!(m.acquire_context(None,Some("healthy")).await.unwrap().id,3);
    }
    m.set_disabled(3,true).unwrap();
    assert!(m.acquire_context(None,None).await.err().unwrap().downcast_ref::<StaticIpError>().is_some());
    for e in m.entries.lock().iter().filter(|e|e.id!=3) {assert!(!e.disabled);assert_eq!(e.refresh_failure_count,0);}
}
#[tokio::test]
async fn conflict_selection_preserves_healthy_account_rate_limits() {
    let mut a=account("first");a.id=Some(1);a.proxy_url=Some("http://192.0.2.10:3000".into());
    let mut b=account("second");b.id=Some(2);b.proxy_url=a.proxy_url.clone();
    let mut c=account("healthy");c.id=Some(3);
    let m=manager(None,vec![a,b,c]);m.static_ips.lock().inventory.entries.push(input("192.0.2.10").into_entry(1).unwrap());
    m.set_credential_traffic(3,CredentialTraffic {concurrency:Some(1),..Default::default()}).unwrap();
    let permit=m.try_acquire_permit(3).unwrap();
    assert!(m.acquire_context(None,None).await.err().unwrap().downcast_ref::<UpstreamRateLimitError>().is_some());
    drop(permit);assert_eq!(m.acquire_context(None,None).await.unwrap().id,3);
    m.set_credential_traffic(3,CredentialTraffic {rpm:Some(1),..Default::default()}).unwrap();
    assert!(m.acquire_context(None,None).await.err().unwrap().downcast_ref::<UpstreamRateLimitError>().is_some());
}
#[tokio::test(flavor="multi_thread")]
async fn waiting_ip_metadata_edits_persist_and_failed_writes_leave_state_unchanged() {
    let dir=temp_dir();let path=dir.join("credentials.json");
    let m=manager(Some(path.clone()),vec![]);m.static_ip_import(vec![input("192.0.2.10")]).unwrap();m.static_ip_auto_assign(true).unwrap();
    let first=m.insert_test_personal(account("first")).await.unwrap();m.set_disabled(first,true).unwrap();
    m.insert_test_personal(account("second")).await.unwrap();m.set_disabled(first,false).unwrap();
    for proxy_url in [None,Some(None),Some(Some("".into()))] {
        m.update_credential(first,Some(Some("edited@example.invalid".into())),proxy_url,None,None,None,Some(Some("saved".into())),None).unwrap();
        let saved:Vec<KiroCredentials>=serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        let reopened=manager(Some(path.clone()),saved);
        assert_eq!(reopened.static_ip_list().waiting_credential_ids,vec![first]);
        assert!(reopened.static_ip_lease(first).is_err());assert!(reopened.try_acquire_permit(first).is_none());
    }
    std::fs::remove_file(&path).unwrap();std::fs::create_dir(&path).unwrap();
    for proxy_url in [None,Some(None),Some(Some("".into()))] {
        assert!(m.update_credential(first,Some(Some("not-saved@example.invalid".into())),proxy_url,None,None,None,None,None).is_err());
        let c=m.clone_credential(first).unwrap();assert!(c.static_ip_required);assert_eq!(c.email.as_deref(),Some("edited@example.invalid"));
        assert!(m.try_acquire_permit(first).is_none());
    }
    std::fs::remove_dir_all(dir).unwrap();
}
#[tokio::test]
async fn waiting_ip_explicit_proxy_changes_still_work() {
    for url in ["direct","http://192.0.2.20:3000"] {
        let m=manager(None,vec![]);m.static_ip_import(vec![input("192.0.2.10")]).unwrap();m.static_ip_auto_assign(true).unwrap();
        let first=m.insert_test_personal(account("first")).await.unwrap();m.set_disabled(first,true).unwrap();
        m.insert_test_personal(account("second")).await.unwrap();m.set_disabled(first,false).unwrap();
        m.update_credential(first,None,Some(Some(url.into())),Some(Some("new-user".into())),Some(Some("new-password".into())),None,None,None).unwrap();
        let c=m.clone_credential(first).unwrap();assert!(!c.static_ip_required);assert_eq!(c.proxy_url.as_deref(),Some(url));
        if url=="direct" {assert!(c.proxy_username.is_none() && c.proxy_password.is_none());}
        else {assert_eq!(c.proxy_username.as_deref(),Some("new-user"));assert_eq!(c.proxy_password.as_deref(),Some("new-password"));}
        assert!(m.try_acquire_permit(first).is_some());
    }
}

#[tokio::test]
async fn regression_unchanged_proxy_metadata_edit_during_request_should_succeed() {
    let m=manager(None,vec![]);m.static_ip_import(vec![input("192.0.2.10")]).unwrap();m.static_ip_auto_assign(true).unwrap();
    let id=m.insert_test_personal(account("first")).await.unwrap();
    let lease=m.static_ip_lease(id).unwrap();let permit=m.try_acquire_permit(id).unwrap();
    let c=m.clone_credential(id).unwrap();
    let result=m.update_credential(id,Some(Some("edited@example.invalid".into())),Some(c.proxy_url.clone()),None,None,None,Some(Some("new remark".into())),None);
    assert!(result.is_ok(),"No proxy change was requested, but metadata edit was rejected: {:?}",result.err());
    assert_eq!(m.clone_credential(id).unwrap().proxy_url,c.proxy_url);drop(permit);drop(lease);
}
#[test]
fn regression_legacy_enterprise_ordinary_proxy_metadata_edit_should_succeed() {
    let mut c=account("enterprise");c.id=Some(1);c.provider=Some("Enterprise".into());c.auth_method=Some("idc".into());
    c.proxy_url=Some("http://192.0.2.10:4000".into());c.proxy_username=Some("ordinary-user".into());c.proxy_password=Some("ordinary-pass".into());
    let m=manager(None,vec![c.clone()]);m.static_ips.lock().inventory.entries.push(input("192.0.2.10").into_entry(1).unwrap());
    let result=m.update_credential(1,None,Some(c.proxy_url.clone()),None,None,None,Some(Some("new remark".into())),None);
    assert!(result.is_ok(),"Preserved ordinary enterprise proxy must not block unrelated metadata edit: {:?}",result.err());
}

#[test]
fn ordinary_imports_are_shared_but_prevent_atomic_inventory_adoption() {
    let m=manager(None,vec![]);let mut a=account("first");a.proxy_url=Some("http://192.0.2.10:3000".into());
    let first=m.prepare_static_ip(&mut a).unwrap();let mut b=a.clone();b.proxy_url=Some("socks5://192.0.2.10:4000".into());
    let second=m.prepare_static_ip(&mut b).unwrap();assert!(!a.static_ip_required && !b.static_ip_required);
    assert!(m.static_ip_import(vec![input("192.0.2.11"),input("192.0.2.10")]).is_err());assert!(m.static_ip_list().entries.is_empty());
    drop(first);assert!(m.static_ip_import(vec![input("192.0.2.10")]).is_err());
    drop(second);assert!(m.static_ips.lock().ordinary_imports.is_empty());m.static_ip_import(vec![input("192.0.2.10")]).unwrap();
}
#[tokio::test]
async fn cancelling_ordinary_import_releases_its_guard() {
    let m=manager(None,vec![]);let worker=m.clone();let (ready,wait)=tokio::sync::oneshot::channel();
    let task=tokio::spawn(async move {
        let mut c=account("cancelled");c.proxy_url=Some("http://192.0.2.10:3000".into());
        let _guard=worker.prepare_static_ip(&mut c).unwrap();ready.send(()).unwrap();std::future::pending::<()>().await;
    });
    wait.await.unwrap();assert!(m.static_ip_import(vec![input("192.0.2.10")]).is_err());
    task.abort();assert!(task.await.unwrap_err().is_cancelled());
    m.static_ip_import(vec![input("192.0.2.10")]).unwrap();assert_eq!(m.static_ip_list().entries[0].status,"free");
}
#[test]
fn ordinary_import_pins_global_snapshot_without_changing_account_settings() {
    let m=manager(None,vec![]);m.set_global_proxy(Some(ProxyConfig::new("http://192.0.2.10:3000")));
    let mut c=account("global");let guard=m.prepare_static_ip(&mut c).unwrap().unwrap();
    assert!(c.proxy_url.is_none() && !c.static_ip_required);assert_eq!(guard.proxy.url,"http://192.0.2.10:3000");
    m.set_global_proxy(Some(ProxyConfig::new("http://192.0.2.20:3000")));
    assert!(m.static_ip_import(vec![input("192.0.2.10")]).is_err());
    drop(guard);m.static_ip_import(vec![input("192.0.2.10")]).unwrap();
}
#[test]
fn import_commit_rechecks_ownership_type_and_reservation() {
    let m=manager(None,vec![]);m.static_ip_import(vec![input("192.0.2.10")]).unwrap();m.static_ip_auto_assign(true).unwrap();
    let mut c=account("personal");let guard=m.prepare_static_ip(&mut c).unwrap();
    let store=m.static_ips.lock();
    assert!(validate_import_commit(&store,&[],&c,guard.as_ref()).is_ok());
    assert!(validate_import_commit(&store,&[],&c,None).is_err());
    let mut other=c.clone();other.id=Some(99);let fixture=manager(None,vec![other]);
    assert!(validate_import_commit(&store,&fixture.entries.lock(),&c,guard.as_ref()).is_err());
    let mut invalid=c.clone();invalid.provider=Some("Enterprise".into());invalid.auth_method=Some("idc".into());
    assert!(validate_import_commit(&store,&[],&invalid,guard.as_ref()).is_err());
    invalid=c.clone();invalid.static_ip_required=false;assert!(validate_import_commit(&store,&[],&invalid,guard.as_ref()).is_err());
    drop(store);drop(guard);
}
#[tokio::test]
async fn ordinary_api_import_preserves_shared_proxy_and_cleans_registration() {
    let m=manager(None,vec![]);
    for key in ["fixture-one","fixture-two"] {
        m.add_credential(KiroCredentials {auth_method:Some("api_key".into()),kiro_api_key:Some(key.into()),proxy_url:Some("http://192.0.2.10:3000".into()),..Default::default()}).await.unwrap();
    }
    assert_eq!(m.snapshot().total,2);assert!(m.static_ips.lock().ordinary_imports.is_empty());
    for c in m.clone_all_credentials() {assert!(!c.static_ip_required);assert_eq!(c.proxy_url.as_deref(),Some("http://192.0.2.10:3000"));}
    assert!(m.static_ip_import(vec![input("192.0.2.10")]).is_err());
}
#[tokio::test(flavor="multi_thread")]
async fn metadata_only_edit_is_atomic_and_real_proxy_change_stays_blocked() {
    let dir=temp_dir();let path=dir.join("credentials.json");let m=manager(Some(path.clone()),vec![]);
    m.static_ip_import(vec![input("192.0.2.10"),input("192.0.2.11")]).unwrap();m.static_ip_auto_assign(true).unwrap();
    let id=m.insert_test_personal(account("first")).await.unwrap();let lease=m.static_ip_lease(id).unwrap();let before=m.clone_credential(id).unwrap();
    m.update_credential(id,None,Some(before.proxy_url.clone()),before.proxy_username.clone().map(Some),before.proxy_password.clone().map(Some),None,Some(Some("saved".into())),None).unwrap();
    assert!(m.update_credential(id,None,Some(Some("http://192.0.2.11:3000".into())),None,None,None,None,None).is_err());
    assert!(m.update_credential(id,None,Some(Some("direct".into())),None,None,None,None,None).is_err());
    let saved=m.clone_credential(id).unwrap();assert_eq!(saved.proxy_url,before.proxy_url);assert_eq!(saved.proxy_password,before.proxy_password);assert!(saved.static_ip_required);
    std::fs::remove_file(&path).unwrap();std::fs::create_dir(&path).unwrap();
    assert!(m.update_credential(id,None,Some(before.proxy_url),None,None,None,Some(Some("not-saved".into())),None).is_err());
    assert_eq!(m.clone_credential(id).unwrap().source_channel.as_deref(),Some("saved"));drop(lease);std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn partial_static_edit_preserves_concurrent_email_discovery() {
    let dir=temp_dir();let path=dir.join("credentials.json");
    let mut c=account("personal");c.id=Some(1);
    let m=manager(Some(path.clone()),vec![c]);m.static_ip_import(vec![input("192.0.2.10")]).unwrap();m.static_ip_bind(1,Some(1)).unwrap();
    let url=m.clone_credential(1).unwrap().proxy_url;
    let refresh=m.clone();
    EDIT_SNAPSHOT_HOOK.with(|slot| *slot.borrow_mut()=Some(Box::new(move || {
        let mut entries=refresh.entries.lock();let c=&mut entries[0].credentials;
        c.email=Some("discovered@example.invalid".into());
        c.access_token=Some("rotated-test-token".into());c.subscription_title=Some("KIRO PRO MAX".into());
    })));
    m.update_credential(1,None,Some(url),None,None,None,Some(Some("new remark".into())),None).unwrap();
    let updated=m.clone_credential(1).unwrap();
    assert_eq!(updated.email.as_deref(),Some("discovered@example.invalid"),"Partial edit overwrote the email filled by an in-flight balance query");
    assert_eq!(updated.source_channel.as_deref(),Some("new remark"));
    assert_eq!(updated.access_token.as_deref(),Some("rotated-test-token"));assert_eq!(updated.subscription_title.as_deref(),Some("KIRO PRO MAX"));
    let saved:Vec<KiroCredentials>=serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
    assert_eq!(saved[0].email,updated.email);assert!(saved[0].static_ip_required);
    std::fs::remove_dir_all(dir).unwrap();
}
#[test]
fn partial_static_edit_honors_explicit_values_and_clears() {
    for email in [None, Some(String::new()), Some("edited@example.invalid".into())] {
        let mut c=account("personal");c.id=Some(1);
        let m=manager(None,vec![c]);m.static_ip_import(vec![input("192.0.2.10")]).unwrap();m.static_ip_bind(1,Some(1)).unwrap();
        let refresh=m.clone();
        EDIT_SNAPSHOT_HOOK.with(|slot| *slot.borrow_mut()=Some(Box::new(move || {
            let mut entries=refresh.entries.lock();let c=&mut entries[0].credentials;
            c.email=Some("discovered@example.invalid".into());
            c.groups=vec!["previous".into()];c.source_channel=Some("previous".into());
            c.metadata.extra.insert("note".into(),serde_json::json!("previous"));
        })));
        m.update_credential(1,Some(email.clone()),None,None,None,Some(vec![" new ".into()," ".into()]),Some(Some(" ".into())),Some(Default::default())).unwrap();
        let updated=m.clone_credential(1).unwrap();
        assert_eq!(updated.email,email.filter(|s| !s.is_empty()));
        assert_eq!(updated.groups,vec!["new"]);assert!(updated.source_channel.is_none());
        assert!(updated.metadata.extra.is_empty());assert!(updated.static_ip_required);
    }
}
#[test]
fn failed_partial_static_edit_preserves_concurrent_discovery() {
    let dir=temp_dir();let path=dir.join("credentials.json");
    let mut c=account("personal");c.id=Some(1);c.source_channel=Some("original".into());
    let m=manager(Some(path.clone()),vec![c]);m.static_ip_import(vec![input("192.0.2.10")]).unwrap();m.static_ip_bind(1,Some(1)).unwrap();
    std::fs::remove_file(&path).unwrap();std::fs::create_dir(&path).unwrap();
    let refresh=m.clone();
    EDIT_SNAPSHOT_HOOK.with(|slot| *slot.borrow_mut()=Some(Box::new(move || {
        refresh.entries.lock()[0].credentials.email=Some("discovered@example.invalid".into());
    })));
    assert!(m.update_credential(1,Some(Some("edited@example.invalid".into())),None,None,None,None,Some(Some("not saved".into())),None).is_err());
    let updated=m.clone_credential(1).unwrap();
    assert_eq!(updated.email.as_deref(),Some("discovered@example.invalid"));
    assert_eq!(updated.source_channel.as_deref(),Some("original"));assert!(updated.static_ip_required);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn ordinary_host_stays_pinned_after_edit_delete_or_global_change() {
    for mode in ["edit", "delete", "global"] {
        let mut c=account("ordinary");c.id=Some(1);
        let proxy=ProxyConfig::new("http://192.0.2.10:3000");
        if mode!="global" { c.proxy_url=Some(proxy.url.clone()); }
        let m=manager(None,vec![c.clone()]);
        if mode=="global" { m.set_global_proxy(Some(proxy.clone())); }
        let guard=m.pin_proxy_use(&c,Some(&proxy)).unwrap();
        match mode {
            "edit" => m.update_credential(1,None,Some(Some("direct".into())),None,None,None,None,None).unwrap(),
            "delete" => m.delete_credential(1).unwrap(),
            _ => m.set_global_proxy(None),
        }
        assert!(m.static_ip_import(vec![input("192.0.2.11"),input("192.0.2.10")]).is_err(),"{mode}");
        assert!(m.static_ip_list().entries.is_empty());
        drop(guard);
        m.static_ip_import(vec![input("192.0.2.10")]).unwrap();
        assert_eq!(m.static_ip_list().entries[0].status,"free");
    }
}
#[test]
fn ordinary_shared_hosts_release_only_after_last_user_and_drop_needs_no_lock() {
    let m=manager(None,vec![]);let c=account("ordinary");
    let first=m.pin_proxy_use(&c,Some(&ProxyConfig::new("http://192.0.2.10:3000"))).unwrap();
    let last=m.pin_proxy_use(&c,Some(&ProxyConfig::new("socks5://192.0.2.10:4000"))).unwrap();
    drop(first);assert!(m.static_ip_import(vec![input("192.0.2.10")]).is_err());
    { let _store=m.static_ips.lock();drop(last); }
    m.static_ip_import(vec![input("192.0.2.10")]).unwrap();
}
#[test]
fn stale_proxy_snapshot_cannot_use_new_static_owner() {
    let mut old=account("old");old.id=Some(1);old.proxy_url=Some("http://192.0.2.10:3000".into());
    let mut other=account("new");other.id=Some(2);let m=manager(None,vec![old.clone(),other]);
    m.update_credential(1,None,Some(Some("direct".into())),None,None,None,None,None).unwrap();
    m.static_ip_import(vec![input("192.0.2.10")]).unwrap();m.static_ip_bind(2,Some(1)).unwrap();
    let proxy=ProxyConfig::new("http://192.0.2.10:3000");
    assert!(m.pin_proxy_use(&old,Some(&proxy)).is_err());
    assert!(m.pin_proxy_use(&m.clone_credential(2).unwrap(),Some(&proxy)).is_ok());
    m.delete_credential(1).unwrap();assert!(m.pin_proxy_use(&old,Some(&proxy)).is_err());
}
#[tokio::test]
async fn cancelled_proxy_operation_releases_host_registration() {
    let m=manager(None,vec![]);let worker=m.clone();let (ready,wait)=tokio::sync::oneshot::channel();
    let task=tokio::spawn(async move {
        let _guard=worker.pin_proxy_use(&account("ordinary"),Some(&ProxyConfig::new("http://192.0.2.10:3000"))).unwrap();
        ready.send(()).unwrap();std::future::pending::<()>().await;
    });
    wait.await.unwrap();assert!(m.static_ip_import(vec![input("192.0.2.10")]).is_err());
    task.abort();assert!(task.await.unwrap_err().is_cancelled());
    m.static_ip_import(vec![input("192.0.2.10")]).unwrap();
}
#[tokio::test]
async fn response_body_holds_actual_proxy_until_read_error_cancel_or_drop() {
    use futures::StreamExt;
    use crate::kiro::traffic::LimitedResponse;
    for mode in ["text","bytes","stream","error","cancel","drop"] {
        let m=manager(None,vec![]);
        let guard=m.pin_proxy_use(&account("ordinary"),Some(&ProxyConfig::new("http://192.0.2.10:3000"))).unwrap();
        let (release,wait)=tokio::sync::oneshot::channel();
        let body=futures::stream::once(async move {
            let _=wait.await;
            if mode=="error" { Err(std::io::Error::other("fixture body failure")) }
            else { Ok(bytes::Bytes::from_static(b"body")) }
        });
        let response=reqwest::Response::from(axum::http::Response::new(reqwest::Body::wrap_stream(body)));
        let counter=Arc::new(AtomicU32::new(1));
        let response=LimitedResponse::new(response,TrafficPermit(counter.clone())).with_proxy_use(guard);
        assert!(m.static_ip_import(vec![input("192.0.2.10")]).is_err());
        if mode=="drop" { drop(response); }
        else {
            let task=tokio::spawn(async move {
                match mode {
                    "text" => assert_eq!(response.text().await.unwrap(),"body"),
                    "bytes" => assert_eq!(response.bytes().await.unwrap(),"body"),
                    "error" => assert!(response.text().await.is_err()),
                    _ => {
                        let mut stream=Box::pin(response.bytes_stream());
                        assert_eq!(stream.next().await.unwrap().unwrap(),"body");
                        assert!(stream.next().await.is_none());
                    }
                }
            });
            tokio::task::yield_now().await;
            assert!(m.static_ip_import(vec![input("192.0.2.10")]).is_err());
            if mode=="cancel" { task.abort();assert!(task.await.unwrap_err().is_cancelled()); }
            else { release.send(()).unwrap();task.await.unwrap(); }
        }
        assert_eq!(counter.load(Ordering::Acquire),0);
        m.static_ip_import(vec![input("192.0.2.10")]).unwrap();
    }
}

#[test]
fn socks_default_ports_have_the_same_exclusive_host_identity() {
    for scheme in ["socks4","socks4a","socks5","socks5h"] {
        for host in ["192.0.2.10","[2001:db8::1]"] {
            let implicit=format!("{scheme}://user:pass@{host}");
            let explicit=format!("{scheme}://{host}:1080");
            assert!(endpoint_key(&implicit).is_some(),"{implicit}");
            assert_eq!(endpoint_key(&implicit),endpoint_key(&explicit));
            assert_eq!(endpoint_key(&implicit),endpoint_key(&format!("http://{host}:3000")));
        }
    }
    assert_eq!(endpoint_key("192.0.2.10:3000"),endpoint_key("http://192.0.2.10"));
    assert_eq!(endpoint_key("https://192.0.2.10"),Some("192.0.2.10".into()));
    for invalid in ["", "socks5://", "socks5://192.0.2.10:bad", "socks5://192.0.2.10:65536"] {
        assert!(endpoint_key(invalid).is_none(),"{invalid}");
    }
}
#[test]
fn omitted_socks_ports_cannot_bypass_ownership_type_or_global_checks() {
    for scheme in ["socks4","socks4a","socks5","socks5h"] {
        let mut a=account("owner");a.id=Some(1);let mut b=account("other");b.id=Some(2);
        let m=manager(None,vec![a,b]);m.static_ip_import(vec![input("192.0.2.10")]).unwrap();m.static_ip_bind(1,Some(1)).unwrap();
        let url=format!("{scheme}://192.0.2.10");
        assert!(m.update_credential(2,None,Some(Some(url.clone())),None,None,None,None,None).is_err());
        assert!(m.guard_static_global_proxy(Some(&url)).is_err());
        let mut imported=account("new");imported.proxy_url=Some(url.clone());
        assert!(m.prepare_static_ip(&mut imported).is_err());
        imported.provider=Some("Enterprise".into());imported.auth_method=Some("idc".into());
        assert!(m.prepare_static_ip(&mut imported).is_err());
        assert_eq!(m.static_ip_list().entries[0].credential_ids,vec![1]);
        assert!(m.clone_credential(2).unwrap().proxy_url.is_none());
    }
}
#[test]
fn omitted_socks_ports_preserve_inflight_and_ordinary_import_guards() {
    for scheme in ["socks4","socks4a","socks5","socks5h"] {
        let m=manager(None,vec![]);let mut c=account("ordinary");
        c.proxy_url=Some(format!("{scheme}://192.0.2.10"));
        let reservation=m.prepare_static_ip(&mut c).unwrap();
        assert!(!c.static_ip_required);assert!(m.static_ip_import(vec![input("192.0.2.10")]).is_err());
        drop(reservation);
        let proxy=c.effective_proxy(None).unwrap();let guard=m.pin_proxy_use(&c,Some(&proxy)).unwrap();
        assert!(m.static_ip_import(vec![input("192.0.2.10")]).is_err());drop(guard);
        m.static_ip_import(vec![input("192.0.2.10")]).unwrap();assert_eq!(m.static_ip_list().entries[0].status,"free");
    }
}
#[test]
fn omitted_socks_ports_detect_existing_shared_and_single_owners() {
    for scheme in ["socks4","socks4a","socks5","socks5h"] {
        let mut a=account("first");a.id=Some(1);a.proxy_url=Some(format!("{scheme}://192.0.2.10"));
        let mut b=account("second");b.id=Some(2);b.proxy_url=Some("http://192.0.2.10:3000".into());
        let m=manager(None,vec![a.clone(),b]);assert!(m.static_ip_import(vec![input("192.0.2.10")]).is_err());
        let m=manager(None,vec![a]);m.static_ip_import(vec![input("192.0.2.10")]).unwrap();
        assert_eq!(m.static_ip_list().entries[0].credential_ids,vec![1]);
        assert!(m.clone_credential(1).unwrap().static_ip_required);
    }
}

#[test]
fn mapped_ipv4_has_one_identity_without_merging_native_ipv6() {
    for scheme in ["http", "https", "socks4", "socks4a", "socks5", "socks5h"] {
        for host in ["192.0.2.10", "[::ffff:192.0.2.10]", "[0:0:0:0:0:ffff:c000:20a]"] {
            assert_eq!(endpoint_key(&format!("{scheme}://user:pass@{host}:3000")), Some("192.0.2.10".into()));
        }
    }
    assert_eq!(endpoint_key("[::ffff:192.0.2.10]:3000"), endpoint_key("192.0.2.10:80"));
    assert_ne!(endpoint_key("http://[::192.0.2.10]"), endpoint_key("http://192.0.2.10"));
    assert_ne!(endpoint_key("http://[64:ff9b::c000:20a]"), endpoint_key("http://192.0.2.10"));
    assert_eq!(endpoint_key("http://[2001:0db8:0:0:0:0:0:1]"), endpoint_key("http://[2001:db8::1]"));
    assert_eq!(endpoint_key("http://PROXY.EXAMPLE.invalid:3000"),Some("proxy.example.invalid".into()));
}

#[test]
fn mapped_ipv4_duplicate_inventory_is_rejected_atomically() {
    for (first, alias) in [("192.0.2.10", "::ffff:192.0.2.10"), ("::ffff:192.0.2.10", "192.0.2.10")] {
        let m=manager(None,vec![]);
        assert!(m.static_ip_import(vec![input(first),input(alias)]).is_err());
        assert!(m.static_ip_list().entries.is_empty());
        m.static_ip_import(vec![input(first)]).unwrap();
        let url=m.static_ip_list().entries[0].url.clone();
        let mut duplicate=input(alias);duplicate.port=4000;duplicate.protocol="socks5".into();
        assert!(m.static_ip_import(vec![input("192.0.2.11"),duplicate]).is_err());
        let pool=m.static_ip_list();assert_eq!(pool.entries.len(),1);assert_eq!(pool.entries[0].url,url);
    }
}

#[test]
fn mapped_ipv4_alias_cannot_bypass_binding_import_type_or_global_checks() {
    for (host, alias) in [("192.0.2.10", "http://[::ffff:192.0.2.10]:3000"), ("::ffff:192.0.2.10", "http://192.0.2.10:3000")] {
        let mut a=account("owner");a.id=Some(1);let mut b=account("other");b.id=Some(2);
        let m=manager(None,vec![a,b]);m.static_ip_import(vec![input(host)]).unwrap();m.static_ip_bind(1,Some(1)).unwrap();
        assert!(m.update_credential(2,None,Some(Some(alias.into())),None,None,None,None,None).is_err());
        assert!(m.guard_static_global_proxy(Some(alias)).is_err());
        let mut imported=account("new");imported.proxy_url=Some(alias.into());
        assert!(m.prepare_static_ip(&mut imported).is_err());
        assert!(m.pin_proxy_use(&m.clone_credential(2).unwrap(),Some(&ProxyConfig::new(alias))).is_err());
        let owner=m.clone_credential(1).unwrap();
        let guard=m.pin_proxy_use(&owner,Some(&ProxyConfig::new(alias))).unwrap();
        m.set_disabled(1,true).unwrap();assert_eq!(m.static_ip_list().entries[0].status,"releasing");
        assert!(m.static_ip_bind(2,Some(1)).is_err());drop(guard);
        assert_eq!(m.static_ip_list().entries[0].status,"free");
        imported.provider=Some("Enterprise".into());imported.auth_method=Some("idc".into());
        assert!(m.prepare_static_ip(&mut imported).is_err());
        m.static_ip_bind(2,Some(1)).unwrap();
        assert_eq!(m.clone_credential(2).unwrap().proxy_url,owner.proxy_url);
    }
}

#[test]
fn mapped_ipv4_inflight_and_import_reservations_block_alias_adoption() {
    for (host, alias) in [("192.0.2.10", "http://[::ffff:192.0.2.10]:3000"), ("::ffff:192.0.2.10", "http://192.0.2.10:3000")] {
        let m=manager(None,vec![]);let mut c=account("ordinary");c.proxy_url=Some(alias.into());
        let reservation=m.prepare_static_ip(&mut c).unwrap();
        assert!(!c.static_ip_required);assert!(m.static_ip_import(vec![input(host)]).is_err());drop(reservation);
        let proxy=c.effective_proxy(None).unwrap();
        // Ordinary shared proxies remain allowed, and the last operation owns the pin.
        let first=m.pin_proxy_use(&c,Some(&proxy)).unwrap();let last=m.pin_proxy_use(&c,Some(&proxy)).unwrap();
        drop(first);assert!(m.static_ip_import(vec![input(host)]).is_err());drop(last);
        m.static_ip_import(vec![input(host)]).unwrap();
        let mut new=account("new");new.proxy_url=Some(alias.into());let reserved=m.prepare_static_ip(&mut new).unwrap();
        assert!(new.static_ip_required);assert_eq!(m.static_ip_list().entries[0].status,"reserved");
        let mut other=account("other");other.proxy_url=Some(format!("http://{}:3000",if host.contains(':') {format!("[{host}]")} else {host.into()}));
        assert!(m.prepare_static_ip(&mut other).is_err());drop(reserved);
        assert_eq!(m.static_ip_list().entries[0].status,"free");
    }
}

#[test]
fn mapped_ipv4_detects_existing_owners_and_global_proxy() {
    let mut a=account("first");a.id=Some(1);a.proxy_url=Some("http://[::ffff:192.0.2.10]:3000".into());
    let mut b=account("second");b.id=Some(2);b.proxy_url=Some("http://192.0.2.10:4000".into());
    let m=manager(None,vec![a.clone(),b]);assert!(m.static_ip_import(vec![input("192.0.2.10")]).is_err());
    let m=manager(None,vec![a.clone()]);m.static_ip_import(vec![input("192.0.2.10")]).unwrap();
    assert_eq!(m.static_ip_list().entries[0].credential_ids,vec![1]);
    assert_eq!(m.clone_credential(1).unwrap().proxy_url,a.proxy_url);
    assert!(m.clone_credential(1).unwrap().static_ip_required);
    let m=manager(None,vec![]);m.set_global_proxy(Some(ProxyConfig::new("http://[::ffff:192.0.2.10]:3000")));
    assert!(m.static_ip_import(vec![input("192.0.2.10")]).is_err());
}

#[test]
fn mapped_ipv4_inventory_load_rejects_alias_duplicates_without_rewriting() {
    let dir=temp_dir();let path=dir.join("static_ips.json");
    let mut inventory=Inventory::default();
    inventory.entries=vec![input("192.0.2.10").into_entry(1).unwrap(),input("::ffff:192.0.2.10").into_entry(2).unwrap()];
    let bytes=serde_json::to_vec(&inventory).unwrap();std::fs::write(&path,&bytes).unwrap();
    let error=StaticIpStore::load(Some(path.clone())).err().expect("duplicate aliases must fail load");
    assert!(error.to_string().contains("重复地址"));assert_eq!(std::fs::read(&path).unwrap(),bytes);
    inventory.entries.remove(0);let url=inventory.entries[0].url.clone();
    std::fs::write(&path,serde_json::to_vec(&inventory).unwrap()).unwrap();
    let loaded=StaticIpStore::load(Some(path.clone())).unwrap();
    assert_eq!(loaded.inventory.entries[0].url,url);assert!(loaded.contains(Some("http://192.0.2.10:9000")));
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn numeric_ipv4_aliases_cannot_bypass_exclusivity() {
    for alias in ["127.1", "2130706433", "0x7f000001"] {
        let url = format!("socks5://{alias}:3000");
        assert_eq!(endpoint_key(&url), Some("127.0.0.1".into()));
        let mut a=account("owner");a.id=Some(1);let mut b=account("other");b.id=Some(2);
        let m=manager(None,vec![a,b]);m.static_ip_import(vec![input("127.0.0.1")]).unwrap();m.static_ip_bind(1,Some(1)).unwrap();
        assert!(m.update_credential(2,None,Some(Some(url)),None,None,None,None,None).is_err());
    }
    assert_eq!(endpoint_key("socks5://proxy.example:3000"),Some("proxy.example".into()));
}

#[tokio::test]
async fn hostname_proxy_cannot_use_managed_ip_and_pins_all_addresses() {
    let m=manager(None,vec![]);
    let c=account("ordinary");
    let proxy=ProxyConfig::new("http://localhost:3000");
    let first=m.prepare_proxy_use(&c,Some(&proxy)).await.unwrap();
    let second=m.prepare_proxy_use(&c,Some(&proxy)).await.unwrap();
    assert_eq!(first.proxy().unwrap().url,proxy.url);
    assert!(!first.proxy().unwrap().resolved_addrs.is_empty());
    for address in &first.proxy().unwrap().resolved_addrs {
        assert!(m.static_ip_import(vec![input(&address.ip().to_string())]).is_err());
    }
    drop(first);
    assert!(m.static_ip_import(vec![input("127.0.0.1")]).is_err());
    drop(second);
    m.static_ip_import(vec![input("127.0.0.1")]).unwrap();
    for scheme in ["http","https","socks5","socks5h"] {
        let proxy=ProxyConfig::new(format!("{scheme}://localhost:3000"));
        let result=m.prepare_proxy_use(&c,Some(&proxy)).await;
        assert!(result.err().unwrap().downcast_ref::<StaticIpError>().is_some());
    }
}

}
