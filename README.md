# kiro-rs

**该项目基于 [hank9999/kiro.rs](https://github.com/hank9999/kiro.rs) 进行的二次开发**

Telegram 讨论群组：[kiro.rs](https://t.me/+SXAjVkZDWFUyMWVl)

`kiro-rs` 是一个用 Rust 编写的 Anthropic Messages API 与 OpenAI Chat Completions / Responses API 兼容代理。它把 `/v1/messages`、`/v1/chat/completions`、`/v1/responses` 等请求转换为 Kiro / Amazon Q 后端请求，并提供一个可选的 Web Admin 面板来管理凭据、客户端 Key、用量、代理池、请求日志和在线更新。

项目当前的核心目标是：让 Claude Code、Codex CLI、Anthropic / OpenAI SDK 或其它兼容客户端，通过统一的本地 / 自托管服务访问 Kiro 账号能力，同时在服务端集中处理多凭据、token 刷新、故障转移、用量统计和可观测性。

## 🔎 快速引导

- [声明](#notice)
- [功能](#features)
- [快速开始](#quick-start)
- [调用 API](#api-usage)
- [API 路由](#api-routes)
- [配置](#configuration)
- [凭据](#credentials)
- [模型](#models)
- [Thinking、工具与 WebSearch](#thinking-tools-websearch)
- [图片处理](#images)
- [用量、缓存与日志](#usage-cache-logs)
- [Admin UI](#admin-ui)
- [代理和 Region](#proxy-region)
- [负载均衡与故障转移](#load-balancing-failover)
- [在线更新和发布](#updates-release)
- [开发](#development)
- [目录结构](#project-structure)
- [License](#license)
- [社区支持](#community)
- [致谢](#acknowledgements)

<a id="notice"></a>
## 📚 声明

本项目仅供研究和自用。使用本项目产生的任何后果由使用者自行承担。本项目与 AWS、Kiro、Amazon Q、Anthropic、Claude 等官方实体无关，不代表任何官方立场。

<a id="features"></a>
## ✨ 功能

- **Anthropic Messages API 兼容**：`/v1/messages`、`/v1/models`、`/v1/messages/count_tokens`。
- **OpenAI API 兼容**：`/v1/chat/completions` 和 `/v1/responses`，支持非流式响应与合成 SSE，可供 OpenAI SDK 和新版 Codex CLI 使用。
- **Claude Code 兼容端点**：`/cc/v1/messages`、`/cc/v1/messages/count_tokens`。
- **GPT-5.6 模型族**：`gpt-5.6-sol`、`gpt-5.6-terra`、`gpt-5.6-luna`。
- 流式和非流式响应：支持 Anthropic SSE 与 OpenAI SSE 事件格式。
- **多凭据管理**：OAuth、Builder ID、Social、Enterprise / IdC、企业 SSO（Microsoft Entra ID / Azure AD）、Kiro API Key。
- 自动 token 刷新：支持刷新后回写 `credentials.json`。
- **多凭据调度**：`priority` 固定优先级和 `balanced` 均衡分配。
- **故障转移**：凭据失败、额度用尽、账号级 429 限流冷却、token 失效强制刷新。临时网络／代理刷新失败或业务请求发送失败进入 30、60、120、240、300 秒递增冷却，之后请求自动重试；冷却不禁用账号、不释放静态 IP。没有可用候选且存在此类冷却时返回 HTTP 503 和 `Retry-After`。
- **profileArn 策略**：流式端点与 MCP（WebSearch）按账号类型注入真实 ARN 或 Builder ID 占位 ARN；用量类调用跳过占位 ARN。
- **可切换上游域名**：对话 / MCP 可发往 `q.{region}.amazonaws.com`（默认）或新版 Kiro 客户端使用的 `runtime.{region}.kiro.dev`，Admin 面板热切换。
- **端点抽象**：按凭据选择 `ide` 或 `cli` endpoint。
- **工具调用**：支持 `tool_use` / `tool_result` 配对、工具名缩短与反向映射。
- **Thinking / Reasoning 兼容**：支持 `thinking.type=enabled` / `adaptive`、Claude Code 默认 thinking 请求、Kiro 原生 `reasoningContentEvent` 到 Anthropic thinking / signature / redacted thinking 事件的转换。
- **WebSearch**：支持纯 `web_search` 请求和混合工具场景下的本地 agentic web_search loop。
- **图像处理**：入站图片按环境变量自动缩放 / 重编码，降低 AWS Q 单字段大小限制导致的 400 风险。
- **Prompt cache 计量**：模拟 Anthropic cache_control 的 `cache_creation` / `cache_read` token 统计。
- **用量统计**：按客户端 Key、模型、凭据、日期聚合 input/output/cache token 和 credits。
- **请求链路追踪**：SQLite `traces.db`，记录成功 / 失败请求、尝试链路和错误类型。
- 客户端 Key 分发：Admin 面板生成 `sk-...` Key，支持独立启停、轮换、分组和统计；鉴权不强制 Key 前缀。
- **Admin UI**：概览、凭据管理、客户端 Key、请求日志四个主视图。
- 代理能力：全局代理、凭据级代理、代理池、健康检查、轮询分配。
- **在线更新**：从 GitHub Release / Docker Hub 拉取新版本，支持镜像定时自动更新与手动回退。
- **多平台发布**：GitHub Release 构建 Windows、Linux、macOS 和 Docker Hub 多架构镜像。

<a id="quick-start"></a>
## 🚀 快速开始

### Docker

推荐生产部署使用 Docker。仓库提供的 `docker-compose.yml` 默认使用 Docker Hub 镜像：

```yaml
image: ${KIRO_RS_IMAGE:-zyphrzero/kiro-rs:latest}
ports:
  - "8990:8990"
volumes:
  - ./data/:/app/config/
```

部署：

```bash
mkdir -p /opt/kiro-rs/data
cd /opt/kiro-rs
curl -O https://raw.githubusercontent.com/ZyphrZero/kiro.rs/master/docker-compose.yml
docker compose up -d
```

首次启动时，程序会在挂载目录中自动生成：

```text
data/
├── config.json
└── credentials.json
```

`config.json` 会包含随机生成的 `apiKey` 和 `adminApiKey`。查看日志：

```bash
docker compose logs --tail=200 kiro-rs
```

也可以直接打开 `data/config.json` 查看：

```json
{
  "host": "0.0.0.0",
  "port": 8990,
  "apiKey": "sk-kiro-rs-...",
  "adminApiKey": "sk-admin-...",
  "region": "us-east-1",
  "tlsBackend": "rustls",
  "defaultEndpoint": "ide"
}
```

访问：

- API: `http://<host>:8990/v1/messages`
- Admin UI: `http://<host>:8990/admin`

指定镜像版本：

```bash
KIRO_RS_IMAGE=zyphrzero/kiro-rs:0.7.3 docker compose up -d
```

### 下载二进制

正式版本会在 [GitHub Release](https://github.com/ZyphrZero/kiro.rs/releases/latest) 中发布以下平台产物：

- Windows x64
- Linux x64 / arm64
- Linux musl x64 / arm64
- macOS x64 / arm64

下载后把二进制放到工作目录，首次启动会自动生成 `config.json` 和 `credentials.json`。

```bash
./kiro-rs
```

Windows:

```powershell
.\kiro-rs.exe
```

指定配置文件：

```bash
./kiro-rs --config /path/to/config.json --credentials /path/to/credentials.json
```

### 从源码构建

前端 Admin UI 会通过 `rust-embed` 嵌入到最终二进制。构建后端前先构建前端：

```bash
cd admin-ui
bun install
bun run build
cd ..
cargo build --release --locked
```

测试：

```bash
cargo test
```

<a id="api-usage"></a>
## 调用 API

`/v1` 路由支持 `x-api-key` 和 `Authorization: Bearer` 两种鉴权方式。Key 可以是 `config.json` 中用户自定义的 `apiKey`，也可以是 Admin 面板生成的 `sk-...` 客户端 Key。鉴权只比较完整 Key，不限制自定义 `apiKey` 的前缀或格式。

### Anthropic Messages

```bash
curl http://127.0.0.1:8990/v1/messages \
  -H "Content-Type: application/json" \
  -H "x-api-key: sk-kiro-rs-..." \
  -d '{
    "model": "claude-sonnet-4-5-20250929",
    "max_tokens": 1024,
    "stream": true,
    "messages": [
      { "role": "user", "content": "Hello" }
    ]
  }'
```

Claude Code 兼容端点：

```bash
curl http://127.0.0.1:8990/cc/v1/messages \
  -H "Content-Type: application/json" \
  -H "Authorization: Bearer sk-kiro-rs-..." \
  -d '{
    "model": "claude-sonnet-4-8",
    "max_tokens": 1024,
    "stream": true,
    "messages": [
      { "role": "user", "content": "Hello from Claude Code style endpoint" }
    ]
  }'
```

列出模型：

```bash
curl http://127.0.0.1:8990/v1/models \
  -H "Authorization: Bearer sk-kiro-rs-..."
```

估算 token：

```bash
curl http://127.0.0.1:8990/v1/messages/count_tokens \
  -H "Content-Type: application/json" \
  -H "x-api-key: sk-kiro-rs-..." \
  -d '{
    "model": "claude-sonnet-4-5-20250929",
    "messages": [
      { "role": "user", "content": "Count this." }
    ]
  }'
```

### OpenAI Chat Completions

`POST /v1/chat/completions` 接受 OpenAI 消息、函数工具、`tool_choice`、`reasoning_effort`、`max_tokens` / `max_completion_tokens`：

```bash
curl http://127.0.0.1:8990/v1/chat/completions \
  -H "Content-Type: application/json" \
  -H "Authorization: Bearer sk-kiro-rs-..." \
  -d '{
    "model": "gpt-5.6-sol",
    "reasoning_effort": "high",
    "stream": false,
    "messages": [
      { "role": "user", "content": "Hello from an OpenAI client" }
    ]
  }'
```

### OpenAI Responses / Codex CLI

`POST /v1/responses` 接受字符串或 input item 数组形式的 `input`，并支持 `instructions`、`reasoning.effort`、`max_output_tokens` 和非流式 / SSE 响应：

```bash
curl http://127.0.0.1:8990/v1/responses \
  -H "Content-Type: application/json" \
  -H "Authorization: Bearer sk-kiro-rs-..." \
  -d '{
    "model": "gpt-5.6-sol",
    "instructions": "Answer concisely.",
    "input": "What can you do?",
    "reasoning": { "effort": "high" },
    "stream": false
  }'
```

新版 Codex CLI 使用 Responses API。可在 `~/.codex/config.toml` 中添加：

```toml
model = "gpt-5.6-sol"
model_provider = "kiro-rs"

[model_providers.kiro-rs]
name = "kiro-rs"
base_url = "http://127.0.0.1:8990/v1"
env_key = "KIRO_RS_API_KEY"
wire_api = "responses"
```

启动 Codex 前设置与 `config.apiKey` 或客户端 Key 相同的环境变量：

```bash
export KIRO_RS_API_KEY='sk-kiro-rs-...'
codex
```

两个 OpenAI 端点都会复用现有的模型映射、凭据故障转移和用量计量链路。`/v1/responses` 在 `stream: true` 时会复用 Anthropic 流式链路，将上游 SSE 事件增量转换为 Responses SSE，而不是等待完整响应后再合成；WebSearch 多轮调用期间也会持续发送搜索进度和保活事件。Responses 端点会将 Codex 声明的 function、custom 和 namespace 工具（包括 `additional_tools`）转换后转发给 Kiro，并将工具调用还原为对应的 Responses 事件；hosted `web_search` 则由服务端的 Kiro MCP WebSearch 处理。

<a id="api-routes"></a>
## API 路由

### Anthropic 兼容

| 方法 | 路径 | 说明 |
|---|---|---|
| `GET` | `/v1/models` | 返回本服务声明支持的模型列表 |
| `POST` | `/v1/messages` | Anthropic Messages API 兼容入口 |
| `POST` | `/v1/messages/count_tokens` | Anthropic count_tokens 兼容入口 |
| `POST` | `/cc/v1/messages` | Claude Code 兼容入口，流式事件顺序针对 Claude Code 调整 |
| `POST` | `/cc/v1/messages/count_tokens` | Claude Code 兼容 count_tokens |

### OpenAI 兼容

| 方法 | 路径 | 说明 |
|---|---|---|
| `POST` | `/v1/chat/completions` | OpenAI Chat Completions 兼容入口，支持消息、函数工具和 reasoning effort |
| `POST` | `/v1/responses` | OpenAI Responses 兼容入口，适用于新版 Codex CLI |

### Admin

启用 `adminApiKey` 后会挂载：

| 路径 | 说明 |
|---|---|
| `/admin` | 嵌入式 Web 管理界面 |
| `/api/admin/credentials` | 凭据列表、新增、编辑、删除 |
| `/api/admin/credentials/{id}/balance` | 查询单个凭据订阅 / 用量 |
| `/api/admin/credentials/{id}/models` | 查询该凭据上游实际可用模型 |
| `/api/admin/models` | 使用账号池当前选中的可用凭据实时查询模型 |
| `/api/admin/models/test` | 对指定模型发送真实的最小化请求并返回响应、耗时和 credit |
| `/api/admin/client-keys` | 客户端 Key 管理 |
| `/api/admin/stats/*` | 用量统计 |
| `/api/admin/traces` | 请求链路追踪查询 |
| `/api/admin/proxy-pool` | 代理池 |
| `/api/admin/config/*` | 运行时配置（含 `GET/PUT /api/admin/config/api-domain` 上游域名） |
| `/api/admin/auth/*` | Social / IdC 登录流程 |
| `/api/admin/system/update/*` | 在线更新、回退、版本检查 |

Admin API 鉴权同样支持：

- `x-api-key: <adminApiKey>`
- `Authorization: Bearer <adminApiKey>`

<a id="configuration"></a>
## ⚙️ 配置

默认配置文件名是 `config.json`。首次启动如果文件不存在，会自动生成最小配置。

### 最小配置

```json
{
  "host": "0.0.0.0",
  "port": 8990,
  "apiKey": "sk-kiro-rs-change-me",
  "adminApiKey": "sk-admin-change-me",
  "region": "us-east-1",
  "tlsBackend": "rustls",
  "defaultEndpoint": "ide"
}
```

### 常用字段

| 字段 | 默认值 | 说明 |
|---|---:|---|
| `host` | `127.0.0.1` | 监听地址。自动生成配置时为 `0.0.0.0` |
| `port` | `8080` | 监听端口。自动生成配置时为 `8990` |
| `apiKey` | 无 | 配置的 `id=0` 系统 Key；可使用任意非空自定义值，不限制前缀。`/v1` 和 `/cc/v1` 也可使用 Admin 面板创建的客户端 Key |
| `adminApiKey` | 无 | 设置后启用 `/admin` 和 `/api/admin` |
| `region` | `us-east-1` | 全局默认 Region |
| `authRegion` | 无 | token 刷新用 Region，未配置时回退 `region` |
| `apiRegion` | 无 | Kiro API 请求用 Region，未配置时回退 `region` |
| `defaultEndpoint` | `ide` | 凭据未指定 endpoint 时使用的端点 |
| `apiDomain` | `amazonaws` | 对话 / MCP 上游域名：`amazonaws` → `q.{region}.amazonaws.com`，`kiro` → `runtime.{region}.kiro.dev`。见[上游域名](#api-domain) |
| `tlsBackend` | `rustls` | `rustls` 或 `native-tls` |
| `proxyUrl` | 无 | 全局代理，支持 `http://`、`https://`、`socks5://` |
| `proxyUsername` / `proxyPassword` | 无 | 全局代理认证 |
| `loadBalancingMode` | `priority` | `priority` 或 `balanced` |
| `accountThrottleFailover` | `true` | 账号级 429 是否冷却并切换凭据；覆盖 USER/CREDIT 请求限流和 suspicious activity |
| `accountThrottleCooldownSecs` | `1800` | 账号级 429 限流冷却秒数 |
| `modelApi429RetryEnabled` | `true` | 无需换号的容量类模型 API 429 是否由中转自动重试 |
| `suspendedDetectionEnabled` | `true` | 是否识别 403 账号封禁文案（`suspended` + `locked your account`）并立即禁用该凭据、不参与自愈 |
| `selfHealEnabled` | `true` | 全部凭据被自动禁用时是否重置失败计数并重新启用（自愈） |
| `selfHealMinIntervalSecs` | `300` | 两次自愈的最小冷却间隔（秒），打断持续 403 死循环的关键 |
| `selfHealMaxConsecutiveRounds` | `5` | 连续自愈且无成功的最大轮数（`0`=不限），超限即停并提示人工介入 |
| `modelCacheTtlSecs` | `3600` | 每个凭据的上游可用模型缓存 TTL（秒） |
| `extractThinking` | `true` | 非流式响应是否把旧 `<thinking>` 文本提取成 thinking block |
| `cacheMeteringCapacity` | `262144` | prompt cache 计量条目上限，钳到 `[1024, 8388608]`；见下方「计量容量选型」 |
| `cacheMeteringPersistEnabled` | `false` | 是否把计量快照落到 `cache_metering.json`（**0.9.x 起默认关闭**） |
| `cacheMeteringFlushIntervalSecs` | `300` | 快照落盘间隔（秒），钳到 `[10, 3600]`，仅在落盘开启时有意义 |
| `cacheMeteringMissDiagnostics` | `true` | 是否开启 miss 归因墓碑表（约 3MB 封顶），关闭后 `missEvicted` 退化为 `missNoEntry` |
| `traceEnabled` | `true` | 是否写入 `traces.db` |
| `traceRetentionDays` | `7` | trace 保留天数 |
| `usageLogRetentionDays` | `31` | `usage_log.*.jsonl` 保留天数 |
| `countTokensApiUrl` | 无 | 外部 count_tokens API 地址 |
| `countTokensApiKey` | 无 | 外部 count_tokens API Key |
| `countTokensAuthType` | `x-api-key` | `x-api-key` 或 `bearer` |
| `githubToken` | 无 | 在线更新访问 GitHub API 时使用，降低 rate limit 风险 |
| `updateAutoApply` | `false` | 是否每天自动检查并应用新版本 |
| `updateAutoApplyTime` | `03:00` | 自动更新时间，本地时区 `HH:MM` |

非空的 `config.apiKey` 每次启动都会同步为不可删除、可轮换的系统 Key `id=0`。手动修改配置后，旧系统 Key 立即失效，新值自动启用；已有名称、描述、分组和累计统计会保留。Admin 面板新建或轮换的客户端 Key 统一以 `sk-` 开头，但请求鉴权不会对任何已存储 Key 强制检查前缀。`adminApiKey` 独立用于 Admin UI / Admin API 登录，不参与 `/v1` 业务流量鉴权。

<a id="credentials"></a>
## 🔐 凭据

默认凭据文件名是 `credentials.json`。推荐通过 Admin UI 添加、登录和重登凭据；直接编辑文件时建议使用数组格式。

```json
[
  {
    "id": 1,
    "refreshToken": "xxx",
    "expiresAt": "2026-12-31T00:00:00Z",
    "authMethod": "idc",
    "provider": "BuilderId",
    "clientId": "xxx",
    "clientSecret": "xxx",
    "priority": 0
  }
]
```

### 支持的凭据类型

#### Builder ID / IdC

```json
{
  "refreshToken": "xxx",
  "expiresAt": "2026-12-31T00:00:00Z",
  "authMethod": "idc",
  "provider": "BuilderId",
  "clientId": "xxx",
  "clientSecret": "xxx"
}
```

#### Enterprise IAM Identity Center

```json
{
  "refreshToken": "xxx",
  "expiresAt": "2026-12-31T00:00:00Z",
  "authMethod": "idc",
  "provider": "Enterprise",
  "startUrl": "https://example.awsapps.com/start",
  "region": "us-east-1",
  "clientId": "xxx",
  "clientSecret": "xxx"
}
```

Enterprise / IdC 账号在流式调用前会按需调用 `ListAvailableProfiles` 解析真实 `profileArn`，成功后写回凭据。纯 Builder ID/free 账号没有 Enterprise profile 时，会回退到官方 IDE 使用的 Builder ID 占位 ARN，以避免流式端点缺少 `profileArn` 返回 400。

上游对这类账号的 `ListAvailableProfiles` 固定返回 403 `AWS Builder ID is not supported for this operation`，该结果按「无 Enterprise profile」处理：不再请求另一区域，并在进程内标记已解析，后续请求不再重复查询（避免每个请求首字前多出两次串行握手）。

#### Social 登录

```json
{
  "refreshToken": "xxx",
  "expiresAt": "2026-12-31T00:00:00Z",
  "authMethod": "social",
  "provider": "Github"
}
```

`provider` 可为 `Github` 或 `Google`。Social 登录会使用固定 Social profile ARN。

#### 企业 SSO（Microsoft Entra ID / Azure AD）

```json
{
  "refreshToken": "xxx",
  "accessToken": "xxx",
  "expiresAt": "2026-12-31T00:00:00Z",
  "authMethod": "external_idp",
  "provider": "AzureAD",
  "clientId": "11111111-2222-3333-4444-555555555555",
  "tokenEndpoint": "https://login.microsoftonline.com/<tenant>/oauth2/v2.0/token",
  "issuerUrl": "https://login.microsoftonline.com/<tenant>/v2.0",
  "scopes": "openid profile offline_access <resource-scope>"
}
```

适用于 Microsoft 365 / Entra ID / Azure AD 企业租户账号（既不是 AWS Builder ID 也不是 IAM Identity Center）。Token 刷新走 IdP 的 OAuth2 `refresh_token` grant（公共客户端，表单编码，无 `clientSecret`）：`clientId` 与 `tokenEndpoint` 必填，`scopes` 需含 `offline_access` 才能拿到 refresh token。数据面与 Profile 请求会自动携带 `TokenType: EXTERNAL_IDP` 头，真实 `profileArn` 由 `ListAvailableProfiles` 懒解析回填。

`authMethod` 除 `external_idp` 外也接受 `azuread` / `azure` / `entra` / `microsoft` / `m365` 等别名（统一归一化）；未写 `authMethod` 但带 `tokenEndpoint` 时会自动判定为 `external_idp`。出于防 SSRF / refresh token 外泄考虑，`tokenEndpoint`（及 `issuerUrl`）必须为 `https` 且 host 命中允许列表（`*.microsoftonline.com` / `.us` / `.cn`），否则拒绝导入。

> 核心逻辑参考 [Quorinex/Kiro-Go#131](https://github.com/Quorinex/Kiro-Go/pull/131)。本版仅支持以 JSON 导入（不含浏览器门户登录），按需手动获取 Azure 凭据后导入即可。

#### Kiro API Key

```json
{
  "kiroApiKey": "ksk_xxx",
  "authMethod": "api_key",
  "endpoint": "cli"
}
```

也可以通过环境变量临时注入最高优先级 API Key 凭据：

```bash
KIRO_API_KEY=ksk_xxx ./kiro-rs
```

### 凭据字段

| 字段 | 说明 |
|---|---|
| `id` | 凭据 ID，Admin 管理时自动分配 |
| `refreshToken` / `accessToken` | OAuth token |
| `expiresAt` | RFC3339 过期时间 |
| `authMethod` | `idc`、`social`、`external_idp`、`api_key`。旧值 `builder-id`、`iam` 会规范化为 `idc`；`azuread` / `entra` 等别名归一化为 `external_idp` |
| `provider` | `BuilderId`、`Enterprise`、`Github`、`Google`、`IAM_SSO`、`AzureAD` 等 |
| `clientId` / `clientSecret` | IdC 刷新 token 所需 OIDC client；`external_idp` 仅需 `clientId`（公共客户端，无 `clientSecret`） |
| `startUrl` | Enterprise IAM Identity Center Start URL |
| `tokenEndpoint` / `issuerUrl` / `scopes` | 企业 SSO（Entra ID / Azure AD）专用：IdP 刷新端点 / OIDC issuer（备注）/ 已授权 scope（需含 `offline_access`） |
| `profileArn` | 真实 profile ARN 或已知固定 ARN；通常由程序维护 |
| `priority` | 数字越小优先级越高 |
| `region` | 凭据级 Region，兼容旧配置 |
| `authRegion` | 凭据级 token 刷新 Region |
| `apiRegion` | 凭据级 API 请求 Region |
| `machineId` | 凭据级 machine id，未填时自动派生 |
| `email` / `subscriptionTitle` | Admin 查询后回填的展示信息 |
| `proxyUrl` | 凭据级代理；填 `direct` 表示绕过全局代理 |
| `proxyUsername` / `proxyPassword` | 凭据级代理认证 |
| `disabled` | 是否禁用 |
| `kiroApiKey` | `ksk_*` Kiro API Key |
| `endpoint` | `ide` 或 `cli`，未填使用 `config.defaultEndpoint` |

<a id="models"></a>
## 模型

`GET /v1/models` 会查询当前客户端 Key 所属分组可访问的凭据，返回这些凭据上游模型列表的去重并集，不额外生成 `-thinking` 模型。列表按凭据缓存，默认 TTL 为一小时；刷新部分失败时继续使用最后一次成功缓存。为兼容已有客户端，请求中仍可使用 `-thinking` 后缀自动开启 Thinking。

请求模型按以下顺序解析：

1. 优先匹配 `customModels` 中的显式别名。
2. 规范化常见 Claude ID，包括日期后缀、`latest`、`-thinking`、点号/连字符版本和旧式 `claude-3-5-sonnet` 顺序。
3. 其余非空合法模型 ID 原样发给 Kiro，例如 `glm-5`、`minimax-m2.5`、`deepseek-3.2`。最终可用性由上游判断。

动态模型缓存也用于凭据路由：已确认包含目标模型的凭据优先，已加载缓存且明确不包含目标模型的凭据会跳过；尚未加载缓存的凭据仍允许尝试，因此模型列表接口暂时不可用不会形成新的本地白名单。

上下文窗口估算：

- `gpt-5.6-sol`、`gpt-5.6-terra`、`gpt-5.6-luna`：`1_000_000`（静态模型声明最大输出为 `64_000`）
- 其它 `gpt-5.*`：`272_000`
- `claude-sonnet-4.6`、`claude-sonnet-4.8`、`claude-sonnet-5`、`claude-sonnet-5.5`、`claude-opus-4.6`、`claude-opus-4.7`、`claude-opus-4.8`、`claude-opus-5`、`claude-opus-5.5`、`claude-fable-5`：`1_000_000`
- 其它模型：`200_000`

### 自定义模型

可在 `config.json` 增加 `customModels` 数组，把任意客户端模型别名映射到 Kiro 后端模型 ID。自定义条目**优先于**内置 Claude 格式规范化和开放透传，既能新增模型，也能覆盖模型的后端指向。

```jsonc
{
  "customModels": [
    {
      "id": "my-opus",                 // 客户端请求用的模型名（大小写不敏感精确匹配）
      "backendId": "claude-opus-4.8",  // 实际下发给 Kiro 的后端模型 ID（必填）
      "displayName": "My Opus",        // 可选，/v1/models 展示名，缺省用 id
      "contextWindow": 1000000,         // 可选，上下文窗口，缺省 200000
      "maxTokens": 64000,               // 可选，/v1/models 展示的最大输出，缺省 64000
      "supportsReasoning": true,        // 可选，是否放行原生 reasoning，缺省 false
      "ownedBy": "custom"               // 可选，/v1/models 的 owned_by，缺省 "custom"
    }
  ]
}
```

行为说明：

- **匹配**：按 `id` 大小写不敏感精确匹配；客户端传 `my-opus-thinking` 时，若无同名精确条目，会自动剥离 `-thinking` 后缀回退到 `my-opus`。
- **优先级**：命中自定义表直接返回其 `backendId`，不再走内置关键词映射，因此可用同名 `id` 覆盖内置模型的后端指向。
- **展示**：所有 `customModels` 条目会合并到 `GET /v1/models`，同名时自定义展示名、所有者和最大输出 token 元数据优先；最终列表按模型 ID 稳定排序。
- **上下文窗口**：设了 `contextWindow` 时以其为准，否则回退到内置估算。
- **reasoning**：`supportsReasoning: true` 会让该 `backendId` 放行 `additionalModelRequestFields`（`output_config` 等）；后端不接受时上游会返回 400，此时置 false 即可。
- **兼容性**：默认空数组，现有配置无需迁移；`/v1/chat/completions` 与 `/v1/responses` 因复用同一映射链路自动生效。

<a id="thinking-tools-websearch"></a>
## Thinking、工具与 WebSearch

### Thinking

客户端可以显式发送 Anthropic `thinking` 字段，也可以直接使用带 `-thinking` 后缀的模型名。Claude Code 当前也可能在普通模型名下默认发送 `thinking.type=enabled`；服务端会按请求体实际 thinking 配置处理，不依赖模型名是否带后缀。

普通 thinking：

```json
{
  "model": "claude-sonnet-4-8-thinking",
  "max_tokens": 4096,
  "thinking": {
    "type": "enabled",
    "budget_tokens": 20000
  },
  "messages": [
    { "role": "user", "content": "推理一下这个问题" }
  ]
}
```

`budget_tokens` 会限制在 `24576` 以内。

模型名带 `-thinking` 后缀时会自动覆写 thinking 配置：

- Opus 4.6：`thinking.type=adaptive`，并默认设置 `output_config.effort=high`。
- 其它 thinking 模型：`thinking.type=enabled`，`budget_tokens=20000`。

Adaptive thinking：

```json
{
  "model": "claude-opus-4-6-thinking",
  "max_tokens": 4096,
  "thinking": {
    "type": "adaptive"
  },
  "output_config": {
    "effort": "high"
  },
  "messages": [
    { "role": "user", "content": "给出完整分析" }
  ]
}
```

`additionalModelRequestFields.output_config` 是 Kiro 上游的窄兼容字段。当前只会在已知可接受该字段的 Opus 4.6 adaptive thinking 路径上传递；Sonnet 4.5 / 4.8、Opus 4.6 非 adaptive thinking 等路径会跳过该字段，避免上游返回 `additionalModelRequestFields is not supported for this model`。`effort` 会先归一化大小写和空格；已知 4.5 / 4.6 系列不接受 `xhigh`，会降级为最接近的 `high`；Opus 4.7 / 4.8、Fable 5、Mythos 5 会保留 `xhigh`；其它未知模型的已知 effort 值也会保持原样，避免维护一张容易过期的模型白名单；未知 effort 值会回退到 `high`。

Kiro 上游可能返回原生 `reasoningContentEvent`。`kiro-rs` 会把它转换为 Anthropic 兼容内容：

- `text` → 流式 `thinking_delta`，非流式 `thinking` block。
- `signature` → 流式 `signature_delta`，非流式 `thinking.signature`。
- `redactedContent` → `redacted_thinking` block。
- 原生 reasoning 始终保留思考类型，即使请求未启用 thinking，也不会混入普通正文；签名及 redacted 内容同样保留。请求中的 thinking / effort 仍按原规则控制上游生成，不会因为输出分块而强制开启思考。

思考输出按接口分别返回：`/v1/messages`（含 `/cc/v1/messages`）使用 `thinking` / `thinking_delta`，`/v1/chat/completions` 使用 `message.reasoning_content` / `delta.reasoning_content`，`/v1/responses` 使用 `reasoning` 项及 reasoning summary 事件。流式与非流式均适用，客户端需支持对应字段。搜索循环的明文思考也使用独立思考块，不再仅放在 `kiro_thinking` 扩展字段中。无法从没有任何标记的普通正文中可靠推断哪些内容属于思考。

非流式响应优先使用原生 reasoning 事件；只有没有原生 reasoning 时，才回退到旧的 `<thinking>...</thinking>` 文本提取路径。

### Tool Use

服务端会把 Anthropic tools 转成 Kiro 工具定义，并处理以下兼容逻辑：

- 长工具名会被缩短，并在响应流中恢复原始名称。
- 孤立的 `tool_use` / `tool_result` 会被过滤或修复，避免上游因消息配对错误返回不可恢复错误。
- tool_result 中的图片会提升到 Kiro 顶层图片字段，并走同一套图片缩放逻辑。

### WebSearch

支持 Anthropic web_search tool：

```json
{
  "model": "claude-sonnet-4-8",
  "max_tokens": 2048,
  "stream": true,
  "tools": [
    {
      "type": "web_search_20250305",
      "name": "web_search",
      "max_uses": 5
    }
  ],
  "messages": [
    { "role": "user", "content": "搜索今天的相关信息" }
  ]
}
```

纯 web_search 请求会直接走上游 MCP 搜索接口。混合工具场景下，如果上游返回只包含 `web_search` 的工具调用，`kiro-rs` 会内部调用同一套 MCP 搜索接口，把结果作为 tool_result 喂回上游，直到上游停止搜索或达到轮数限制；其它工具调用会原样返回给客户端。

MCP 搜索请求通过 `x-amzn-kiro-profile-arn` 头携带 profileArn，取值规则与对话请求相同（真实 ARN，或 Builder ID / Social 占位 ARN；API Key 凭据不发送）。上游 `/mcp` 缺少该头会返回 400 `profileArn is required for this request.`，表现为客户端收到 502 `Upstream API request failed.`。

<a id="images"></a>
## 图片处理

入站图片会在本地 CPU 上按需压缩，默认策略：

- 长边上限：`1568px`
- base64 字段大小上限：`400000`
- JPEG 质量：`85`
- PNG / JPEG / WebP 大图会重编码为 JPEG
- GIF 保留原格式，避免破坏动画
- 解码失败时保留原图并记录 warning，不会让整个请求失败

环境变量：

| 变量 | 默认值 | 说明 |
|---|---:|---|
| `KIRO_RS_IMAGE_RESIZE` | `1` | `0`、`false`、`no`、`off` 可关闭 |
| `KIRO_RS_IMAGE_MAX_LONG_SIDE` | `1568` | 长边像素上限 |
| `KIRO_RS_IMAGE_MAX_BYTES` | `400000` | base64 字段大小阈值 |
| `KIRO_RS_IMAGE_JPEG_QUALITY` | `85` | JPEG 输出质量 |

<a id="usage-cache-logs"></a>
## 用量、缓存与日志

运行数据默认落在 `credentials.json` 所在目录。Docker 部署时就是 `./data/`。

```text
data/
├── config.json
├── credentials.json
├── client_api_keys.json
├── kiro_stats.json
├── kiro_balance_cache.json
├── proxy_pool.json
├── cache_metering.json
├── traces.db
└── usage_log.YYYY-MM-DD.jsonl
```

说明：

- `client_api_keys.json`：系统 Key 和 Admin 生成的 `sk-...` 客户端 Key，明文存储，用于鉴权。
- `kiro_stats.json`：凭据成功 / 失败 / 额度 / 冷却等统计。
- `kiro_balance_cache.json`：凭据订阅、额度、邮箱等缓存。
- `proxy_pool.json`：代理池与健康状态。
- `cache_metering.json`：prompt cache 计量快照，**默认不再读写**（需显式 `cacheMeteringPersistEnabled: true`）。
- `traces.db`：SQLite 请求链路追踪数据库，WAL 模式。
- `usage_log.*.jsonl`：按日滚动请求用量日志。

`CacheMeter` 会基于 `cache_control` 和会话信息模拟 Anthropic prompt cache 口径，输出互斥的：

- `input_tokens`
- `cache_creation_input_tokens`
- `cache_read_input_tokens`
- `output_tokens`

### Prompt cache 计量容量选型

`CacheMeter` 是一张定容表。写入速率为 `W` 条/秒时，一条 entry 的实际存活期是

```text
T_eff = min(C / W, TTL)
```

`C < W × TTL` 时 TTL 完全失效，命中率由容量而不是 TTL 决定，表现为 `cache_read`
随负载**无声衰减**。上游每请求最多写 4 条断点、默认 TTL 300 秒，所以容量应满足：

```text
C >= peak_req_per_sec × entries_per_req × TTL_secs
```

默认 `262144` 支撑约 `262144 / (4 × 300) ≈ 218 req/s`，内存约 16–24MB
（单条按 64 字节估算）。上界 `8388608` 约 512MB —— 那是「再大就该上 Redis
共享层而不是堆内存」的界线，不是推荐值。改过切段策略、`entries_per_req`
远大于 4 的部署，按公式自行调高。

容量、落盘开关、落盘间隔、miss 诊断开关**启动时固定，运行时不可改**：
改容量会立即触发一次大规模淘汰，不适合做热调旋钮。改这些 = 改 `config.json`
（或环境变量）+ 重启。Admin API `GET /api/admin/cache-metering` 只读暴露当前生效值。

环境变量（优先级：`config.json` 显式值 > 环境变量 > 内置默认）：

| 环境变量 | 对应字段 | 默认 |
|---|---|---:|
| `KIRO_RS_CACHE_CAPACITY` | `cacheMeteringCapacity` | `262144` |
| `KIRO_RS_CACHE_PERSIST` | `cacheMeteringPersistEnabled` | `false` |
| `KIRO_RS_CACHE_FLUSH_INTERVAL_SECS` | `cacheMeteringFlushIntervalSecs` | `300` |
| `KIRO_RS_CACHE_MISS_DIAGNOSTICS` | `cacheMeteringMissDiagnostics` | `true` |

数值解析失败、为 `0` 或为空串都视为「未表态」并打一条 warn，回落到下一优先级，不会中止启动。

#### 怎么判断该不该调容量

后台任务每 60 秒输出一行（本周期无任何增量时不输出）：

```text
cache_meter stats entries_len=13421 capacity=262144 hits=90120 misses=10233 evicted=0 expired=240311
  miss_no_entry=8120 miss_expired=1900 miss_evicted=13 miss_no_breakpoint=180 miss_no_seed=20 tombstone_clears=0
```

- `miss_evicted` 持续非零 + `entries_len` 贴着 `capacity` → **容量不够**，按上面的公式调高。
- `miss_expired` 占多数、`evicted=0` → 是真实 TTL 过期，调容量没用。
- `miss_no_breakpoint` / `miss_no_seed` 占多数 → 客户端没声明 `cache_control`，
  或请求缺少 `metadata.user_id` 且用的是 `id=0` 系统 Key，与容量无关。

单条 miss 的归因还会在 `RUST_LOG=debug` 下输出：有断点的请求按断点逐条打
`cache_meter miss block_idx=.. reason=Absent|Expired|Evicted`（一个断点最多一条，
单请求最多 4 条）；整条请求级的两种原因打 `cache_meter miss reason=no_breakpoint`
与 `cache_meter miss reason=no_seed`。五种原因都可在 debug 日志里看到。

> 计数口径：**一个断点只记一次账**。lookback 会向后探最多 20 个 hash，
> 「深层没有、浅层命中」是正常的部分前缀命中，只记 1 次 `hits`，不会同时产生
> 一堆 `misses` —— 否则命中率报表会被探测次数稀释。

#### 升级提示：快照默认不再落盘

`cacheMeteringPersistEnabled` 默认为 `false`，即**默认不再读写 `cache_metering.json`**。
依据：计量 TTL 只有 300 秒，快照只在重启后 5 分钟内有价值，而容量调大后单次全量
JSON 重写的代价线性放大。关闭时启动不读该文件、运行与退出都不写该文件，
也**不会删除**已有文件（留给回滚）。需要原行为显式写：

```json
{ "cacheMeteringPersistEnabled": true }
```

快照格式仍是 JSON（未换二进制）：默认关闭后这条路径不再是热点，换格式要带
迁移与兼容分支，收益不抵复杂度。旧版本写的快照文件可直接被新版本加载。

<a id="admin-ui"></a>
## 🖥️ Admin UI

启用 `adminApiKey` 后访问 `/admin`。当前页面：

- 概览：整体请求量、token、模型分布、凭据贡献。
- 凭据管理：添加、登录、重登、删除、禁用、优先级、余额、模型列表、超额开关、代理绑定。
- 客户端 Key：创建、编辑、禁用、轮换、删除、分组绑定和重置统计；系统 Key 不可删除但可轮换。
- 请求日志：查询 `traces.db`，查看失败原因、状态码、凭据尝试链路和 token 用量。

Admin 还提供：

- Social 登录和 IdC / Enterprise 登录流程。
- 全局代理设置和代理池健康检查。
- 负载均衡模式配置。
- 账号级 429 限流故障转移配置。
- 按请求作用域和凭据隔离的自愈治理（403 封禁识别 / 冷却 / 连续上限，状态跨重启保留）。
- trace / usage log 保留策略。
- 在线更新、自动更新和回退。

<a id="proxy-region"></a>
## 代理和 Region

### Region 优先级

Token 刷新：

```text
credential.authRegion -> credential.region -> config.authRegion -> config.region
```

API 请求：

```text
credential.apiRegion -> credential.region -> config.apiRegion -> config.region
```

仅配置凭据 `region` 时，Token 刷新和 API 请求都默认使用该区域。需要分开设置时，使用凭据级 `authRegion` 和 `apiRegion`；`authRegion` 本身不作为 API 请求区域的回退值。

部分 REST / 管理类上游接口只在 `us-east-1` 和 `eu-central-1` 提供服务，代码会按账号区域选择候选端点并在必要时回退。

<a id="api-domain"></a>
### 上游域名

`apiDomain` 决定对话（`generateAssistantResponse`）和 MCP（WebSearch）请求的 host：

| 值 | host | 说明 |
|---|---|---|
| `amazonaws`（默认） | `q.{apiRegion}.amazonaws.com` | 原有行为 |
| `kiro` | `runtime.{apiRegion}.kiro.dev` | 新版 Kiro IDE / Kiro CLI 使用的域名；官方客户端只内置 `us-east-1`、`eu-central-1` 等少数区域 |

Token 刷新、用量查询、`ListAvailableProfiles` 等 REST 接口不受影响。可在 Admin 面板「调度设置 → 上游域名」切换，或调用 `PUT /api/admin/config/api-domain`（Body：`{"domain":"kiro"}`），立即生效并写回 `config.json`，无需重启。

### 代理优先级

```text
credential.proxyUrl -> config.proxyUrl -> direct
```

凭据级 `proxyUrl` 填 `direct` 表示即使配置了全局代理也直连。

支持：

- `http://host:port`
- `https://host:port`
- `socks5://host:port`

如果 `rustls` 环境下代理或证书行为异常，可以在 `config.json` 中切到：

```json
{
  "tlsBackend": "native-tls"
}
```

<a id="load-balancing-failover"></a>
## 负载均衡与故障转移

`loadBalancingMode` 支持：

- `priority`：优先使用 priority 数字最小的可用凭据。
- `balanced`：在可用凭据之间均衡分配。

故障处理：

- 单凭据连续 API 失败会增加失败计数，达到阈值后跳过。
- 402 / quota exhausted 会禁用该凭据并切换。
- 401 / 403 中识别到 bearer token 失效时，会对该凭据强制刷新一次 token 后重试。
- USER/CREDIT 请求限流和 suspicious activity 429 可触发账号级冷却并切换凭据。
- `INSUFFICIENT_MODEL_CAPACITY` 等容量类 429 不切换凭据，可配置是否在当前凭据上退避重试。
- 400 客户端请求错误不会切换凭据。
- 网关超时和部分不可恢复错误会快速失败，避免一次请求内无限放大重试。

<a id="updates-release"></a>
## 在线更新和发布

发布 tag `vX.Y.Z` 会触发 Release workflow：

- 校验 `Cargo.toml` 版本和 tag 一致。
- 构建 Admin UI。
- 构建多平台二进制。
- 构建并推送 Docker Hub 多架构镜像。
- 创建 GitHub Release。

当前稳定版：[v0.9.3](https://github.com/ZyphrZero/kiro.rs/releases/tag/v0.9.3)。

Docker 镜像：

- `zyphrzero/kiro-rs:<version>`
- `zyphrzero/kiro-rs:latest`
- `zyphrzero/kiro-rs:beta`（master beta 构建）

容器内在线更新会下载对应平台二进制并替换当前可执行文件；替换后进程退出，由 Docker `restart: unless-stopped` 拉起新进程。回退依赖本地 `<exe>.backup`。

<a id="development"></a>
## 开发

常用命令：

```bash
# 后端测试
cargo test

# 前端构建
cd admin-ui && bun run build

# 后端 release 构建
cargo build --release --locked

# 开启 debug 日志
RUST_LOG=debug ./target/release/kiro-rs
```

发布前建议：

```bash
cargo test
cd admin-ui && bun run build
git diff --check
```

<a id="project-structure"></a>
## 目录结构

```text
.
├── src/
│   ├── anthropic/      # Anthropic API 兼容层
│   ├── kiro/           # Kiro / Amazon Q 上游、token、endpoint、event-stream
│   ├── admin/          # Admin API、用量、trace、代理池、在线更新
│   ├── admin_ui/       # 嵌入式 Admin UI 静态资源路由
│   ├── model/          # CLI 参数和 config.json 模型
│   ├── common/         # 通用鉴权工具
│   ├── image_resize.rs # 图片缩放与 token 估算
│   ├── token.rs        # count_tokens 估算和远程 count_tokens 调用
│   └── main.rs         # 入口
├── admin-ui/           # React Admin UI
├── .github/workflows/  # build、docker、release workflows
├── docker-compose.yml
├── Cargo.toml
└── CHANGELOG.md
```

<a id="license"></a>
## License

见 [LICENSE](LICENSE)。

<a id="community"></a>
## 💬 社区支持

欢迎到 [linux.do](https://linux.do/) 交流、分享和反馈。

也欢迎加入 [Telegram 讨论群组：kiro.rs](https://t.me/+SXAjVkZDWFUyMWVl)。

<a id="acknowledgements"></a>
## 🙏 致谢

本项目的实现离不开社区项目和反馈的帮助：

- [hank9999/kiro.rs](https://github.com/hank9999/kiro.rs)
- [kiro2api](https://github.com/caidaoli/kiro2api)
- [proxycast](https://github.com/aiclientproxy/proxycast)
- [Kiro-account-manager](https://github.com/chaogei/Kiro-account-manager)

感谢所有 issue、PR、测试和部署反馈的贡献者。


## 本地二次开发范围（2026-10-12）

二开最初以 `1ef319fa4f62fb00d222779d35826cf374753cc2` 为基线，随后按要求回退至 `c8e897d` 并继续开发。2026-10-20 在已验证的 `858ac89` 上合入上游 `v0.9.2`，保留下列二开功能与配置兼容性；未恢复此前回退的故障转移及冷却改动。

- 凭据管理支持 Enterprise、GitHub、Google、Builder ID、IAM SSO、Entra ID、API Key 及未识别登录类型，分别设置默认限制和 FREE、PRO、PRO+、MAX、POWER、STUDENT、待识别订阅等级的并发／RPM。每项限制作用于单个账号；优先级为：单账号设置 → 登录类型＋订阅等级 → 登录类型默认 → 通用订阅等级 → 全局默认。并发和 RPM 分别继承，清空单项恢复继承，0 表示不限；新账号自动采用对应策略。旧配置未设置登录类型策略时保持原有限制。
- 类型策略和单账号设置均在凭据管理页面；类型策略面板默认折叠，可切换登录类型编辑，保存时仅更新改动的字段。显示账号的登录类型、订阅等级、限制来源、实时并发及最近 60 秒请求数。
- 每次进入凭据管理页面自动刷新额度；确认额度已耗尽时自动禁用，可手动关闭该功能。读取失败或额度未知不视为耗尽。
- 支持删除全部选中的凭据（含跨分页选择），显示进度和失败原因，失败项保留选中。
- 兼容平铺／嵌套凭据导入，导入导出保留单账号流量策略。
- 上游总请求超时默认 3600 秒，兼容原 `upstreamRequestTimeoutSecs`。上游新增的 `upstreamTotalTimeoutSecs` 仅在显式设置时优先，设为 0 可关闭总时限；不设置时保留原值，不回退到上游默认的 720 秒。连接超时默认 15 秒、连续读取无数据超时默认 120 秒，三者独立生效。
- 所有 `claude-opus-*` 模型转发输出预算 `max_tokens`，与是否开启思考无关。Opus 4.6 保留最小 1024、超过 64000 按 64000 处理的规则；其他 Opus 模型按请求原值转发（如 Opus 5.5 的 128000），实际可接受上限与生成长度由上游决定。非 Opus 模型保持原有支持范围，保留输出结束原因识别。
- 明确的账号凭据认证失败返回 403 和受控原因文案；普通参数错误仍为 400，本站访问 Key 校验沿用原行为。已经开始输出的 SSE 流不能再改变 HTTP 状态。

本机外部连接测试配置独立保留。不得因为本地修改而自动推送、部署或修改生产账号数据；发布需用户另行要求。

### 静态 IP 独占管理（本地二开）

在凭据管理的更多操作中打开「静态 IP 管理」。按供应商提供的信息填写协议、IP、端口、用户名和密码；列表只显示密码是否已配置。批量导入支持四列 `IP 端口 用户名 密码`（空格或制表符分隔）、`IP:端口:用户名:密码`，以及完整代理 URL。协议默认 HTTP，可选择 HTTPS、SOCKS5、SOCKS5H；请以供应商说明为准。

- 同一 IP 只登记一次，不因端口、认证信息或协议不同而允许重复占用。仅有一个允许类型的空闲账号使用该 IP 时，可以登记并保留绑定；多个账号共用、类型不允许或仍有请求／导入进行中时，整批拒绝登记。旧数据中的冲突会阻止相关静态账号调用，需手动调整。
- 添加代理后开启「新导入账号自动分配空闲 IP」。开关默认关闭，已有账号不会被重新分配。允许类型的新账号未指定代理时，先按原有网络配置刷新 Token、查询真实订阅和额度并保存，再独占绑定空闲 IP 并启用；批量导入并发执行也不会重复领取。静态 IP 数量不限制正常导入，等待账号同样保留初始订阅和额度。导入阶段查询失败会明确报错，不用导出标题冒充验证结果。网页登录新增账号共用这套初始化与额度保存流程；手动指定静态 IP 的账号仍按所选导入模式查询额度，不能仅因绑定 IP 就标记为验活成功。 初始额度缓存写入失败时，返回带账号编号的错误，保留凭据并保持禁用，不自动领取 IP；修复存储后刷新额度并手动启用。
- 没有空闲 IP 时仍保存凭据，归入独立的「等待静态 IP」未启用类别。新增、恢复可分配或释放 IP 后，自动为等待账号逐一绑定并启用，无需重新导入；等待状态重启后保留，不自动共用 IP 或改为直连。显式指定的普通代理、`direct` 直连配置保留原行为。
- 正常账号的 IP 固定且重启后保留。封禁、超额禁用、凭据确认失效、手动禁用都会自动释放；删除或手动解绑也会释放。临时限流、网络／刷新失败冷却和可正常刷新的 accessToken 到期不会释放。生成、Token 刷新、额度查询等正在使用该 IP 时显示「等待请求结束后释放」，结束后后台约 1 秒内释放。导入失败／取消会释放预留。
- 页面可配置允许的个人账号类型，默认 GitHub、Google、Builder ID。已有配置保留已保存的类型开关，可在「静态 IP 管理」中开启 Builder ID。Enterprise、IAM SSO、Entra ID、API Key 与未知类型均不允许分配；自动分配、手动绑定、编辑代理使用同一限制。关闭类型会解除该类型已有绑定，恢复全局代理或直连；尚未分配 IP 的等待账号继续保持禁用，重新允许该类型后再分配；有进行中的请求时拒绝关闭，避免中途切换。
- 自动释放的账号恢复后先领取空闲 IP，再参与调用；无空闲 IP 时显示等待且不会直连或使用已转给其他账号的 IP。关闭自动分配开关不取消此保护。对已禁用账号手动查询额度／刷新时，只能临时领取空闲 IP，结束后再次释放。
- 「暂停分配」只阻止新的分配，不中断已有绑定。占用中、分配中的 IP 不能从库中删除，也不能设为全局代理。静态代理认证信息以库中的设置为准。
- 库文件 `static_ips.json` 保存在凭据目录，绑定以凭据文件的代理设置为准，重启后自动恢复占用。库文件包含代理密码，已排除在 Git 和 Docker 构建上下文之外。
- 普通代理仍允许共享，但导入期间正在使用的地址不能同时登记为静态 IP；导入成功、失败或取消后自动清理在途登记。只编辑备注、邮箱等信息不会触发重新绑定，真实代理变更仍受独占、账号类型及在途请求限制。

本功能管理的是代理出口，不会给本机网卡绑定公网 IP；原有共享代理池保留。

个人账号批量导入支持 JSON 数组、单个对象、KAM 导出，以及直接粘贴的多个逗号分隔对象（外层 `[]` 可省略）。Google/GitHub 的 `email`、`refreshToken`、`provider` 格式会按 Social 登录导入。导入阶段的刷新与额度查询仅对网络传输失败最多尝试三次；无效凭据不重试，错误提示包含失败阶段和安全的网络原因。
