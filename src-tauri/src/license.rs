//! 专业版授权：设备指纹、Ed25519 JWT 验签、soft-candy 激活与在线续验
//!
//! 授权协议（soft-candy `internal/license` 与 `docs/api.md` §4.2/4.3）：
//! - 激活码在 soft-candy 服务端绑定设备，签发 EdDSA JWT
//! - JWT claims：`iss/aud/sub/code/app/level/levelLabel/deviceId/features/iat/exp`
//! - 每应用、每等级独立 Ed25519 公钥；客户端只内置当前产品对应等级的公钥
//! - 许可证存 `app_data_dir/license.json`，本地 JWT 验签有效即可用（离线可用）
//! - 启动时异步在线续验；网络失败不影响已付费用户
//!
//! 免费版限制（门控点见 commands / browse）：
//! - 跨文件夹浏览（兄弟文件夹扫描、文件夹级跳转）
//! - 预取缓存（DecodeCache / FolderFirstCache / get_context）
//!
//! dev 与 release 行为一致：无有效 license 即免费版，需激活后才解锁专业版。

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use base64::Engine;
use ed25519_dalek::{Signature, VerifyingKey};
use serde::{Deserialize, Serialize};
use tauri::State;

/// 功能位：当前捆绑为一个 "pro"（跨文件夹浏览 + 预取缓存）
pub const FEATURE_PRO: &str = "pro";

/// 在线续验发现授权已失效时发往前端的事件（payload 为 LicenseInfo）
pub const LICENSE_STATUS_CHANGED: &str = "license://status-changed";

/// soft-candy 签发 JWT 时固定的 issuer。
pub const LICENSE_ISSUER: &str = "soft-candy";

/// 商店/授权服务配置（**唯一真源：tauri.conf.json → plugins.store**，编译时固化随二进制分发）
///
/// tauri.conf.json 示例：
/// ```json
/// "plugins": {
///   "store": {
///     "apiBase": "https://your-server.example.com",
///     "buyUrl": "https://your-server.example.com/p/z-viewer",
///     "product": "z-viewer",
///     "licenseLevel": "pro",
///     "licensePublicKeys": {
///       "pro": "<z-viewer/pro 的 Ed25519 公钥，raw/DER base64 或 PEM>"
///     },
///     "licenseFileName": "license.json",
///     "activatePath": "/api/v1/apps/z-viewer/activate",
///     "verifyPath": "/api/v1/apps/z-viewer/verify",
///     "deactivatePath": "/api/v1/apps/z-viewer/deactivate",
///     "analyticsPath": "/api/v1/apps/z-viewer/analytics",
///     "analyticsToken": "<soft-candy 管理后台生成的埋点 Bearer token，空 = 不携带>"
///   }
/// }
/// ```
///
/// 字段缺失/为空即视为未配置：apiBase 未配置时激活报错、不联网验证；
/// 对应等级公钥未配置时验签必失败（无法激活）。无任何内置默认值。
/// analyticsToken 为空时不携带 Authorization 头（服务端对该产品不校验，
/// 兼容未配置 token 的部署）；配置后每次埋点上报携带 `Bearer <token>`。
#[derive(Clone, Debug)]
pub struct StoreConfig {
    /// 授权服务 API base（激活/在线验证）；空 = 未配置
    pub api_base: String,
    /// 官网购买页 URL；None = 未配置（前端购买按钮给出提示）
    pub buy_url: Option<String>,
    /// soft-candy 产品标识（也是 JWT `aud`/`app`，及购买页 `?product=` 参数）
    pub product: String,
    /// 本构建购买的授权等级；soft-candy 支持多等级，等级名必须对应 `licensePublicKeys`
    pub license_level: String,
    /// 等级名 -> Ed25519 公钥（raw 32 字节 base64、完整 SPKI DER base64 或 PEM）
    pub public_keys: HashMap<String, String>,
    /// 许可证存储文件名（app_data_dir 下）
    pub license_file_name: String,
    /// 激活接口路径（拼在 api_base 后）
    pub activate_path: String,
    /// 在线续验接口路径（拼在 api_base 后）
    pub verify_path: String,
    /// 注销激活接口路径（拼在 api_base 后）
    pub deactivate_path: String,
    /// 会话统计上报接口路径（拼在 api_base 后）；空 = 不上报
    pub analytics_path: String,
    /// 埋点上报 Bearer token（soft-candy 管理后台生成）；空 = 不携带 Authorization 头
    pub analytics_token: String,
}

impl StoreConfig {
    /// 从 tauri.conf.json 的 plugins.store 读取（编译期配置，运行时不可改）
    pub fn from_config(config: &tauri::Config) -> Self {
        let v = config.plugins.0.get("store").cloned().unwrap_or_default();
        let s = |k: &str| {
            v.get(k)
                .and_then(|x| x.as_str())
                .map(str::trim)
                .filter(|s| !s.is_empty())
        };

        let license_level = s("licenseLevel").unwrap_or("pro").to_string();
        let mut public_keys = HashMap::new();
        if let Some(levels) = v.get("licensePublicKeys").and_then(|x| x.as_object()) {
            for (level, key) in levels {
                if let Some(key) = key.as_str().map(str::trim).filter(|k| !k.is_empty()) {
                    public_keys.insert(level.clone(), key.to_string());
                }
            }
        }

        Self {
            api_base: s("apiBase").unwrap_or_default().to_string(),
            buy_url: s("buyUrl").map(String::from),
            product: s("product").unwrap_or_default().to_string(),
            license_level,
            public_keys,
            license_file_name: s("licenseFileName").unwrap_or_default().to_string(),
            activate_path: s("activatePath").unwrap_or_default().to_string(),
            verify_path: s("verifyPath").unwrap_or_default().to_string(),
            deactivate_path: s("deactivatePath").unwrap_or_default().to_string(),
            analytics_path: s("analyticsPath").unwrap_or_default().to_string(),
            analytics_token: s("analyticsToken").unwrap_or_default().to_string(),
        }
    }

    fn public_key_for(&self, level: &str) -> Option<&str> {
        self.public_keys.get(level).map(String::as_str)
    }
}

/// soft-candy 签发的 Ed25519 JWT 载荷。
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Claims {
    pub iss: String,
    pub aud: String,
    pub sub: String,
    pub code: String,
    pub app: String,
    pub level: String,
    pub level_label: String,
    pub device_id: String,
    /// 购买邮箱
    #[serde(default)]
    pub email: Option<String>,
    pub features: Vec<String>,
    pub iat: i64,
    pub exp: i64,
}

/// 许可证本地记录（code + JWT + email）
#[derive(Clone, Serialize, Deserialize, PartialEq, Eq, Debug)]
#[serde(rename_all = "camelCase")]
pub struct License {
    /// 激活码（大写；本地保存，注销激活时回传）
    pub code: String,
    /// soft-candy 返回的 JWT；磁盘字段名为 `license`
    #[serde(rename = "license")]
    pub token: String,
    /// 购买邮箱（本地保存，激活/验证/注销激活时回传）
    pub email: String,
}

impl License {
    pub fn claims(&self) -> Option<Claims> {
        parse_claims_from_token(&self.token).ok()
    }

    pub fn device_id(&self) -> Option<String> {
        self.claims().map(|c| c.device_id)
    }

    /// 从磁盘反序列化许可证
    fn from_persisted(s: &str) -> Option<License> {
        serde_json::from_str(s).ok()
    }

    /// 完整校验：JWT 验签通过 + 产品/等级/设备/功能位/有效期均匹配。
    pub fn is_valid_with(&self, dev: &str, store: &StoreConfig) -> bool {
        let Some(claims) = self.claims() else {
            return false;
        };
        let Some(pubkey_b64) = store.public_key_for(&claims.level) else {
            return false;
        };
        let Ok(verified) = verify_jwt(&self.token, pubkey_b64) else {
            return false;
        };
        verified == claims
            && claims_ok(&verified, dev, &store.product)
            && self.code.eq_ignore_ascii_case(&verified.code)
            && email_matches_claims(&self.email, &verified)
    }
}

/// 前端可见的授权状态（跨 IPC 的富权益答案：status/level/features 由同一判定源派生，
/// 前端 LicenseService 快照此答案回答权益问题，不再各处拼装光秃 boolean）
#[derive(Clone, Serialize, PartialEq, Eq, Debug)]
#[serde(rename_all = "camelCase")]
pub struct LicenseInfo {
    /// "pro" | "free"
    pub status: String,
    /// 当前设备指纹（激活后返回，供用户识别设备）
    pub device_id: Option<String>,
    /// 当前 JWT 的等级标识（如 pro / enterprise）
    pub level: Option<String>,
    /// 当前 JWT 的等级名称（如 专业版）
    pub level_label: Option<String>,
    /// 本地保存的激活码（未激活时为 null）
    pub code: Option<String>,
    /// 本地保存的购买邮箱（未激活时为 null）
    pub email: Option<String>,
    /// JWT 权益位（与 status 同源；前端按位回答细分权益，加权益只扩展此处）
    pub features: Vec<String>,
}

/// 授权管理器（Tauri 托管状态；Clone 供后台任务移动）
#[derive(Clone)]
pub struct LicenseManager {
    license: Arc<Mutex<Option<License>>>,
    storage: PathBuf,
    store: StoreConfig,
}

/// 授权接口 HTTP 客户端：10s 超时。
/// 激活/续验/注销都不该让连接挂起无限等待（服务器不响应时激活对话框会永久卡死）；
/// builder 失败（如 TLS 初始化异常）退回无配置默认客户端。
fn http_client() -> reqwest::Client {
    reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()
        .unwrap_or_else(|_| reqwest::Client::new())
}

/// 解析授权服务器错误响应：body 中 `error` 为字符串时返回用户可读错误信息
/// （有 `message` 用 `{msg}（{code}）`，否则用 `{action}失败（{code}）`）；
/// 无 `error` 字段返回 None。网络/JSON 层错误由调用方自行处理。
fn server_error(body: &serde_json::Value, action: &str) -> Option<String> {
    if let Some(code) = body.get("error").and_then(|v| v.as_str()) {
        if let Some(msg) = body.get("message").and_then(|v| v.as_str()) {
            return Some(format!("{msg}（{code}）"));
        }
        return Some(format!("{action}失败（{code}）"));
    }
    None
}

impl LicenseManager {
    /// 从磁盘加载许可证（不存在/验签失败 = 免费版）；store 为编译期配置。
    pub fn load(storage: PathBuf, store: StoreConfig) -> Self {
        let existed = storage.is_file();
        let license = std::fs::read_to_string(&storage)
            .ok()
            .and_then(|s| License::from_persisted(&s))
            .filter(|lic| lic.is_valid_with(&device_id(), &store));
        if existed && license.is_none() {
            log::debug!("license load invalid fallback to free");
        }
        Self {
            license: Arc::new(Mutex::new(license)),
            storage,
            store,
        }
    }

    /// 许可证落盘：先确保存储目录存在，再写入 JWT。
    /// 原子替换（临时文件 + rename）：写盘中途断电/崩溃不会留下半截
    /// license.json（那会让已激活用户掉回免费版，需重新激活）。
    fn persist(&self, lic: &License) -> Result<(), String> {
        let s = serde_json::to_string_pretty(lic).map_err(|e| e.to_string())?;
        if let Some(dir) = self.storage.parent() {
            std::fs::create_dir_all(dir)
                .map_err(|e| format!("无法创建许可证目录 {}: {e}", dir.display()))?;
        }
        let tmp = self.storage.with_extension("json.tmp");
        std::fs::write(&tmp, s).map_err(|e| format!("无法保存许可证到 {}: {e}", tmp.display()))?;
        match std::fs::rename(&tmp, &self.storage) {
            Ok(()) => Ok(()),
            Err(e) => {
                let _ = std::fs::remove_file(&tmp);
                Err(format!("无法保存许可证到 {}: {e}", self.storage.display()))
            }
        }
    }

    fn clear(&self) {
        let _ = std::fs::remove_file(&self.storage);
    }

    fn clear_license(&self) {
        if let Ok(mut g) = self.license.lock() {
            *g = None;
        }
        self.clear();
    }

    /// 当前许可证（未验证；校验用 is_pro）
    pub fn license(&self) -> Option<License> {
        self.license.lock().ok()?.clone()
    }

    /// setup→manage 缓存的商店配置复用入口：analytics 上报等调用方优先
    /// 从已托管的本实例取配置，避免重复解析 tauri::Config（Top2 收口；
    /// 未托管等极端回退路径仍走 StoreConfig::from_config）。
    pub fn store_config(&self) -> StoreConfig {
        self.store.clone()
    }

    /// 是否已解锁专业版（dev 与 release 行为一致）
    /// 本地 JWT 验签有效 && 产品/设备/功能位匹配 && 未过期；无有效 license 即免费版。
    ///
    /// 判定本体为纯函数 is_pro_with（可测）；此处只做快照传入。
    pub fn is_pro(&self) -> bool {
        is_pro_with(
            &self.license.lock().ok().and_then(|g| g.clone()),
            &self.store,
            &device_id(),
        )
    }

    /// 激活：调 soft-candy 换取 JWT → 验签 → 校验设备/等级/功能位 → 落盘
    pub async fn activate(&self, code: String, email: String) -> Result<License, String> {
        if self.store.api_base.is_empty() {
            return Err("授权服务器未配置（tauri.conf.json plugins.store.apiBase）".to_string());
        }
        let code = code.trim().to_uppercase();
        let email = email.trim().to_lowercase();
        let dev = device_id();
        let client = http_client();
        let url = format!("{}{}", self.store.api_base, self.store.activate_path);
        let resp = client
            .post(url)
            .json(&serde_json::json!({
                "code": code.clone(),
                "deviceId": dev,
                "email": email.clone(),
                "level": self.store.license_level,
            }))
            .send()
            .await
            .map_err(|e| format!("无法连接授权服务器：{e}"))?;
        let body: serde_json::Value = resp
            .json()
            .await
            .map_err(|e| format!("授权服务器响应异常：{e}"))?;
        if let Some(e) = server_error(&body, "激活") {
            return Err(e);
        }

        let token = body
            .get("license")
            .and_then(|v| v.as_str())
            .ok_or_else(|| "授权服务器返回格式错误".to_string())?;
        let lic = License {
            code,
            token: token.to_string(),
            email,
        };
        if !lic.is_valid_with(&dev, &self.store) {
            return Err("激活码无效（JWT 验签失败）".to_string());
        }
        *self.license.lock().map_err(|e| e.to_string())? = Some(lic.clone());
        self.persist(&lic)?;
        Ok(lic)
    }

    /// 在线续验（启动时后台调用）：soft-candy 会返回新 JWT 延长离线宽限期；
    /// 网络失败静默，只有明确吊销/失效时才清空本地授权并返回最新状态。
    pub async fn verify_online(&self) -> Option<LicenseInfo> {
        let lic = self.license()?;
        let lic_device_id = lic.device_id()?;
        if lic.email.is_empty() {
            return None; // 缺少购买邮箱：不发起在线验证
        }
        if self.store.api_base.is_empty() {
            return None; // 未配置授权服务器：不联网验证
        }
        let email = lic.email.clone();
        let code = lic.code.clone();
        let client = http_client();
        let url = format!("{}{}", self.store.api_base, self.store.verify_path);
        let resp = client
            .post(url)
            .json(&serde_json::json!({
                "deviceId": lic_device_id,
                "license": lic.token,
                "email": email.clone(),
            }))
            .send()
            .await;
        let Ok(resp) = resp else {
            // 在线验证网络失败：离线降级，不阻止使用，只 warn（不记 URL/载荷）
            log::warn!("license verify offline, using grace period");
            return None; // 网络失败不阻止使用
        };
        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.json::<serde_json::Value>().await.unwrap_or_default();
            let error = body.get("error").and_then(|v| v.as_str()).unwrap_or("");
            if status.as_u16() == 401 || is_revocation_error(error) {
                // 吊销：只记静态文案，不记 error/message 原文与授权值
                log::warn!("license revoked, credentials removed (http-status)");
                self.clear_license();
                return Some(self.status());
            }
            // 其他服务端错误：保留凭证等重验，只 debug（release 不可见）
            log::debug!("license verify deferred (http-status)");
            return None; // 其他服务端错误保守视为有效，不误伤付费用户
        };
        let Ok(body) = resp.json::<serde_json::Value>().await else {
            log::debug!("license verify deferred (body)");
            return None; // 响应异常保守视为有效，不误伤付费用户
        };
        let error = body.get("error").and_then(|v| v.as_str()).unwrap_or("");
        if is_revocation_error(error) {
            log::warn!("license revoked, credentials removed (body-error)");
            self.clear_license();
            return Some(self.status());
        }
        let valid = body.get("valid").and_then(|v| v.as_bool()).unwrap_or(true);
        if !valid {
            log::warn!("license revoked, credentials removed (valid-flag)");
            self.clear_license();
            return Some(self.status());
        }

        // soft-candy 在线续验成功后会签发新 JWT；本地验签通过后立即换新。
        if let Some(token) = body.get("license").and_then(|v| v.as_str()) {
            let refreshed = License {
                code,
                token: token.to_string(),
                email,
            };
            if refreshed.is_valid_with(&device_id(), &self.store) {
                if let Ok(mut g) = self.license.lock() {
                    *g = Some(refreshed.clone());
                }
                let _ = self.persist(&refreshed);
            }
        }
        log::debug!("license verify ok");
        None
    }

    /// 注销激活：调 soft-candy 解绑 → 删除本地 license → 停用专业功能
    pub async fn deactivate(&self) -> Result<(), String> {
        let lic = self.license().ok_or_else(|| "当前设备未激活".to_string())?;
        if lic.code.is_empty() || lic.email.is_empty() {
            return Err("本地缺少完整激活信息，请重新激活后再注销".to_string());
        }
        if self.store.api_base.is_empty() {
            return Err("授权服务器未配置（tauri.conf.json plugins.store.apiBase）".to_string());
        }
        if self.store.deactivate_path.is_empty() {
            return Err(
                "注销激活接口未配置（tauri.conf.json plugins.store.deactivatePath）".to_string(),
            );
        }

        let client = http_client();
        let url = format!("{}{}", self.store.api_base, self.store.deactivate_path);
        let code = lic.code.clone();
        let device_id = lic.device_id();
        let email = lic.email.clone();
        let resp = client
            .post(url)
            .json(&serde_json::json!({
                "code": code,
                "deviceId": device_id,
                "email": email,
            }))
            .send()
            .await
            .map_err(|e| format!("无法连接授权服务器：{e}"))?;
        let body: serde_json::Value = resp
            .json()
            .await
            .map_err(|e| format!("授权服务器响应异常：{e}"))?;
        if let Some(e) = server_error(&body, "注销激活") {
            return Err(e);
        }
        if body.get("unbound").and_then(|v| v.as_bool()) != Some(true) {
            return Err("注销激活失败".to_string());
        }

        self.clear_license();
        Ok(())
    }
}

/// 门控核心判定（纯函数，恒编译）：JWT 验签 + 产品/等级/设备/功能位/有效期。
/// dev 显式传入——测试喂真实 device_id（正例）或伪造 id（wrong-device 反例），
/// 生产传 device_id()。port（StoreConfig/HTTP/存储/时钟）已 DROPPED：
/// StoreConfig 是数据（构造注入），HTTP/存储/时钟经 localhost mock + temp-dir
/// + 相对 now() 测试，无需 trait。
pub(crate) fn is_pro_with(lic: &Option<License>, store: &StoreConfig, dev: &str) -> bool {
    lic.as_ref()
        .is_some_and(|lic| lic.is_valid_with(dev, store))
}

fn claims_ok(claims: &Claims, dev: &str, product: &str) -> bool {
    claims.iss == LICENSE_ISSUER
        && claims.aud == product
        && claims.app == product
        && !claims.code.is_empty()
        && claims.sub == claims.code
        && !claims.level.is_empty()
        && claims.device_id == dev
        && claims.features.iter().any(|f| f == FEATURE_PRO)
        && claims.exp > now()
}

fn parse_claims_from_token(token: &str) -> Result<Claims, String> {
    let parts: Vec<&str> = token.trim().split('.').collect();
    if parts.len() != 3 {
        return Err("JWT 格式不正确".to_string());
    }
    parse_claims(parts[1])
}

fn parse_claims(payload_part: &str) -> Result<Claims, String> {
    let payload = decode_jwt_part(payload_part)?;
    serde_json::from_slice(&payload).map_err(|_| "JWT 载荷解析失败".to_string())
}

fn verify_jwt(token: &str, pubkey_b64: &str) -> Result<Claims, String> {
    let parts: Vec<&str> = token.trim().split('.').collect();
    if parts.len() != 3 {
        return Err("JWT 格式不正确".to_string());
    }
    let header = decode_jwt_part(parts[0])?;
    let header: serde_json::Value =
        serde_json::from_slice(&header).map_err(|_| "JWT header 解析失败".to_string())?;
    if header.get("alg").and_then(|v| v.as_str()) != Some("EdDSA") {
        return Err("JWT 算法不是 EdDSA".to_string());
    }

    let claims = parse_claims(parts[1])?;
    let signature_bytes = decode_jwt_part(parts[2])?;
    let signature =
        Signature::from_slice(&signature_bytes).map_err(|_| "JWT 签名格式错误".to_string())?;
    let public_key = parse_public_key(pubkey_b64)?;
    public_key
        .verify_strict(format!("{}.{}", parts[0], parts[1]).as_bytes(), &signature)
        .map_err(|_| "JWT 签名无效".to_string())?;
    Ok(claims)
}

fn decode_jwt_part(part: &str) -> Result<Vec<u8>, String> {
    base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(part)
        .map_err(|_| "JWT 编码不正确".to_string())
}

fn parse_public_key(pubkey_b64: &str) -> Result<VerifyingKey, String> {
    let bytes = decode_public_key(pubkey_b64)?;
    let key_bytes: [u8; 32] = bytes
        .as_slice()
        .try_into()
        .map_err(|_| "公钥不是 32 字节 Ed25519 公钥".to_string())?;
    VerifyingKey::from_bytes(&key_bytes).map_err(|_| "公钥不是有效 Ed25519 公钥".to_string())
}

/// Ed25519 SPKI 的固定 DER 前缀（`SubjectPublicKeyInfo`）。
const ED25519_SPKI_PREFIX: [u8; 12] = [
    0x30, 0x2a, 0x30, 0x05, 0x06, 0x03, 0x2b, 0x65, 0x70, 0x03, 0x21, 0x00,
];

/// 解码三种可接受的公钥格式：
/// - raw 32 字节 Ed25519 公钥的 base64
/// - 完整 SPKI DER 的 base64（如 soft-candy 管理后台展示的公钥体）
/// - PEM（`-----BEGIN PUBLIC KEY-----` ... `-----END PUBLIC KEY-----`）
fn decode_public_key(input: &str) -> Result<Vec<u8>, String> {
    let compact = input
        .replace("-----BEGIN PUBLIC KEY-----", "")
        .replace("-----END PUBLIC KEY-----", "")
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect::<String>();
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(&compact)
        .map_err(|_| "公钥 base64 解码失败".to_string())?;

    if bytes.len() == 32 {
        return Ok(bytes);
    }
    if bytes.len() == 44 && bytes.starts_with(&ED25519_SPKI_PREFIX) {
        return Ok(bytes[12..].to_vec());
    }
    Err("公钥必须是 raw 32 字节 base64、完整 Ed25519 SPKI DER 或 PEM".to_string())
}

// 当前 Unix 秒：单实现位于 crate::util（此处按原本地名转发，调用点不变）
use crate::util::now;

/// 设备指纹：激活/验证/存储共用。
/// - Windows：注册表 MachineGuid，系统级唯一且重装系统才变化
/// - Linux：/etc/machine-id（systemd 机器标识，与 MachineGuid 等价）
///
/// 进程内缓存（OnceLock）：is_pro 在每个 Tauri 命令里都调用，读取
/// 不该重复发生（机器标识运行期不变）。
pub fn device_id() -> String {
    static CACHE: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    CACHE.get_or_init(device_id_uncached).clone()
}

#[cfg(windows)]
fn device_id_uncached() -> String {
    use winreg::enums::HKEY_LOCAL_MACHINE;
    use winreg::RegKey;
    RegKey::predef(HKEY_LOCAL_MACHINE)
        .open_subkey(r"SOFTWARE\Microsoft\Cryptography")
        .and_then(|k| k.get_value::<String, _>("MachineGuid"))
        .unwrap_or_else(|_| {
            // 兜底：注册表读取失败时用主机名+用户名组合，避免所有机器共用同一 id（跨机复制许可证）
            format!(
                "fallback-{}-{}",
                std::env::var("COMPUTERNAME").unwrap_or_else(|_| "host".into()),
                std::env::var("USERNAME").unwrap_or_else(|_| "user".into())
            )
        })
}

#[cfg(not(windows))]
fn device_id_uncached() -> String {
    std::fs::read_to_string("/etc/machine-id")
        .or_else(|_| std::fs::read_to_string("/var/lib/dbus/machine-id"))
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| {
            // 兜底：machine-id 不可用时用主机名+用户名组合，避免所有机器共用同一 id（跨机复制许可证）
            let host = std::fs::read_to_string("/etc/hostname")
                .map(|s| s.trim().to_string())
                .ok()
                .filter(|s| !s.is_empty())
                .or_else(|| std::env::var("HOSTNAME").ok())
                .unwrap_or_else(|| "host".into());
            format!(
                "fallback-{}-{}",
                host,
                std::env::var("USER").unwrap_or_else(|_| "user".into())
            )
        })
}

// ---------- Tauri 命令 ----------

/// 前端可见的商店信息（购买入口用）
#[derive(Clone, Serialize, PartialEq, Eq, Debug)]
#[serde(rename_all = "camelCase")]
pub struct StoreInfo {
    /// 产品标识（soft-candy 应用 slug / 购买页 `?product=` 参数）
    pub product: String,
    /// 官网购买页 URL；None = 未配置（前端提示联系开发者）
    pub buy_url: Option<String>,
}

/// 查询商店信息（购买按钮：获取官网购买页地址）
#[tauri::command]
pub fn get_store_info(state: State<'_, LicenseManager>) -> StoreInfo {
    StoreInfo {
        product: state.store.product.clone(),
        buy_url: state.store.buy_url.clone(),
    }
}

/// 激活专业版（输入激活码）
#[tauri::command]
pub async fn activate_license(
    state: State<'_, LicenseManager>,
    code: String,
    email: String,
) -> Result<LicenseInfo, String> {
    let code = code.trim().to_uppercase();
    let email = email.trim().to_lowercase();
    if code.is_empty() {
        return Err("请输入激活码".to_string());
    }
    if !is_valid_email(&email) {
        return Err("请输入正确的购买邮箱".to_string());
    }
    // 只记事件不记值：激活码/email/JWT 不进日志；Err 载荷含服务端原文与路径，只记静态文案
    match state.activate(code, email).await {
        Ok(_) => {
            log::info!("license activated");
            Ok(state.status())
        }
        Err(e) => {
            // 本地验签不过单独一条（精确匹配自有静态文案；服务端 msg 自由文本不参与分流）
            if e == "激活码无效（JWT 验签失败）" {
                log::warn!("license activate token verify failed");
            } else {
                log::warn!("license activate failed");
            }
            Err(e)
        }
    }
}

/// 注销激活（成功会删除本地 license）
#[tauri::command]
pub async fn deactivate_license(state: State<'_, LicenseManager>) -> Result<LicenseInfo, String> {
    match state.deactivate().await {
        Ok(_) => {
            log::info!("license deactivated");
            Ok(state.status())
        }
        Err(e) => {
            log::warn!("license deactivate failed");
            Err(e)
        }
    }
}

/// 查询授权状态（启动时前端调用，同步按钮禁用态）
#[tauri::command]
pub fn get_license_status(state: State<'_, LicenseManager>) -> LicenseInfo {
    state.status()
}

impl LicenseManager {
    pub fn status(&self) -> LicenseInfo {
        let dev = device_id();
        let lic = self.license();
        // lic 与 pro 为相邻两次独立快照（license()/is_pro() 各自短暂加锁后即释放）：
        // 两读之间若恰有注销/吊销写入，单次返回内部可能短暂失配（pro=true 而凭证已空），
        // 下一次查询自愈；本地写路径（激活/注销/吊销清理）频率极低，不为此引入快照锁。
        // free 时 features 强制空（过期/篡改/失配但 device/app 匹配时不泄露权益位）。
        let pro = self.is_pro();
        let claims = lic
            .as_ref()
            .and_then(|l| l.claims())
            .filter(|c| c.device_id == dev && c.app == self.store.product);
        let code = lic
            .as_ref()
            .and_then(|l| (!l.code.is_empty()).then(|| l.code.clone()))
            .or_else(|| claims.as_ref().map(|c| c.code.clone()));
        let email = lic
            .as_ref()
            .and_then(|l| (!l.email.is_empty()).then(|| l.email.clone()))
            .or_else(|| {
                claims
                    .as_ref()
                    .and_then(|c| c.email.clone().filter(|e| !e.is_empty()))
            });
        LicenseInfo {
            status: if pro { "pro" } else { "free" }.to_string(),
            device_id: Some(dev),
            level: claims.as_ref().map(|c| c.level.clone()),
            level_label: claims.as_ref().map(|c| c.level_label.clone()),
            code,
            email,
            features: if pro {
                claims
                    .as_ref()
                    .map(|c| c.features.clone())
                    .unwrap_or_default()
            } else {
                Vec::new()
            },
        }
    }
}

/// 服务端吊销/失效错误码：出现即删本地凭证（verify_online 的 HTTP 状态与 body 两条分流共用）
fn is_revocation_error(error: &str) -> bool {
    matches!(
        error,
        "CDK_REVOKED" | "DEVICE_NOT_ACTIVATED" | "INVALID_SIGNATURE"
    )
}

fn email_matches_claims(email: &str, claims: &Claims) -> bool {
    match claims.email.as_deref() {
        Some(claims_email) if !claims_email.is_empty() => email.eq_ignore_ascii_case(claims_email),
        _ => true,
    }
}

fn is_valid_email(email: &str) -> bool {
    let email = email.trim();
    if email.is_empty() || email.contains(char::is_whitespace) || !email.contains('@') {
        return false;
    }
    let mut parts = email.split('@');
    let local = parts.next().unwrap_or_default();
    let domain = parts.next().unwrap_or_default();
    !local.is_empty() && !domain.is_empty() && parts.next().is_none()
}

// ---------- 测试 ----------

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::{Signer, SigningKey};

    fn test_keypair() -> (String, SigningKey) {
        let seed = [7u8; 32];
        let signing = SigningKey::from_bytes(&seed);
        let pubkey = signing.verifying_key().to_bytes();
        (
            base64::engine::general_purpose::STANDARD.encode(pubkey),
            signing,
        )
    }

    fn test_store(pubkey_b64: &str) -> StoreConfig {
        StoreConfig {
            api_base: String::new(),
            buy_url: None,
            product: "z-viewer".into(),
            license_level: "pro".into(),
            public_keys: HashMap::from([("pro".to_string(), pubkey_b64.to_string())]),
            license_file_name: "license.json".into(),
            activate_path: String::new(),
            verify_path: String::new(),
            deactivate_path: String::new(),
            analytics_path: String::new(),
            analytics_token: String::new(),
        }
    }

    fn spki_public_key(raw_b64: &str) -> String {
        let raw = base64::engine::general_purpose::STANDARD
            .decode(raw_b64)
            .unwrap();
        let mut der = vec![
            0x30, 0x2a, 0x30, 0x05, 0x06, 0x03, 0x2b, 0x65, 0x70, 0x03, 0x21, 0x00,
        ];
        der.extend_from_slice(&raw);
        base64::engine::general_purpose::STANDARD.encode(der)
    }

    fn pem_public_key(raw_b64: &str) -> String {
        format!(
            "-----BEGIN PUBLIC KEY-----\n{}\n-----END PUBLIC KEY-----\n",
            spki_public_key(raw_b64)
        )
    }

    fn claims(device_id: &str, level: &str, expires_at: i64) -> Claims {
        Claims {
            iss: LICENSE_ISSUER.into(),
            aud: "z-viewer".into(),
            sub: "ABCD-EFGH-JKMN-PQRS".into(),
            code: "ABCD-EFGH-JKMN-PQRS".into(),
            app: "z-viewer".into(),
            level: level.into(),
            level_label: if level == "pro" { "专业版" } else { level }.into(),
            device_id: device_id.into(),
            email: Some("buyer@example.com".into()),
            features: vec![FEATURE_PRO.into()],
            iat: now() - 60,
            exp: expires_at,
        }
    }

    fn test_license(claims: &Claims, token: String) -> License {
        License {
            code: claims.code.clone(),
            token,
            email: claims.email.clone().unwrap_or_default(),
        }
    }

    fn sign_jwt(claims: &Claims, signing: &SigningKey) -> String {
        let header = serde_json::json!({ "alg": "EdDSA", "typ": "JWT" });
        let header_b64 =
            base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(header.to_string().as_bytes());
        let payload_b64 = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode(serde_json::to_vec(claims).unwrap());
        let signing_input = format!("{header_b64}.{payload_b64}");
        let sig = signing.sign(signing_input.as_bytes());
        format!(
            "{signing_input}.{}",
            base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(sig.to_bytes())
        )
    }

    #[test]
    fn valid_jwt_passes_checks() {
        let (pubkey, signing) = test_keypair();
        let claims = claims("dev-1", "pro", now() + 3600);
        let token = sign_jwt(&claims, &signing);
        let store = test_store(&pubkey);
        assert!(test_license(&claims, token).is_valid_with("dev-1", &store));
    }

    #[test]
    fn full_spki_and_pem_public_key_passes() {
        let (pubkey, signing) = test_keypair();
        let claims = claims("dev-1", "pro", now() + 3600);
        let token = sign_jwt(&claims, &signing);

        for key in [spki_public_key(&pubkey), pem_public_key(&pubkey)] {
            let store = test_store(&key);
            assert!(test_license(&claims, token.clone()).is_valid_with("dev-1", &store));
        }
    }

    #[test]
    fn invalid_public_key_rejected() {
        assert!(parse_public_key("AAAA").is_err());
    }

    #[test]
    fn tampered_payload_rejected() {
        let (pubkey, signing) = test_keypair();
        let claims = claims("dev-1", "pro", now() + 3600);
        let token = sign_jwt(&claims, &signing);

        let parts: Vec<&str> = token.split('.').collect();
        let mut payload = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(parts[1])
            .unwrap();
        payload[10] ^= 1;
        let tampered = format!(
            "{}.{}.{}",
            parts[0],
            base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(payload),
            parts[2]
        );

        let store = test_store(&pubkey);
        assert!(!test_license(&claims, tampered).is_valid_with("dev-1", &store));
    }

    #[test]
    fn wrong_app_or_device_rejected() {
        let (pubkey, signing) = test_keypair();
        let mut c = claims("dev-1", "pro", now() + 3600);
        c.app = "other-app".into();
        c.aud = "other-app".into();
        let token = sign_jwt(&c, &signing);
        let store = test_store(&pubkey);
        assert!(!test_license(&c, token).is_valid_with("dev-1", &store));

        let c = claims("dev-2", "pro", now() + 3600);
        let token = sign_jwt(&c, &signing);
        assert!(!test_license(&c, token).is_valid_with("dev-1", &store));
    }

    #[test]
    fn expired_jwt_rejected() {
        let (pubkey, signing) = test_keypair();
        let claims = claims("dev-1", "pro", now() - 1);
        let token = sign_jwt(&claims, &signing);
        let store = test_store(&pubkey);
        assert!(!test_license(&claims, token).is_valid_with("dev-1", &store));
    }

    #[test]
    fn missing_level_key_rejected() {
        let (pubkey, signing) = test_keypair();
        let claims = claims("dev-1", "enterprise", now() + 3600);
        let token = sign_jwt(&claims, &signing);
        let store = test_store(&pubkey);
        assert!(!test_license(&claims, token).is_valid_with("dev-1", &store));
    }

    #[test]
    fn wrong_email_rejected() {
        let (pubkey, signing) = test_keypair();
        let claims = claims("dev-1", "pro", now() + 3600);
        let token = sign_jwt(&claims, &signing);
        let store = test_store(&pubkey);
        let lic = License {
            code: claims.code.clone(),
            token,
            email: "other@example.com".into(),
        };
        assert!(!lic.is_valid_with("dev-1", &store));
    }

    #[test]
    fn email_validation_accepts_common_formats() {
        assert!(is_valid_email("buyer@example.com"));
        assert!(is_valid_email("buyer@example"));
        assert!(is_valid_email("user@localhost"));
        assert!(is_valid_email("zy19980711@gmail.com"));
        assert!(!is_valid_email("user@"));
        assert!(!is_valid_email("user@@example.com"));
        assert!(!is_valid_email("user @example.com"));
    }

    /// 许可证持久化：storage 目录不存在（首次运行）时 persist 应自动创建目录并落盘，
    /// 重新 load（模拟重启）后能读回同一 JWT。
    #[test]
    fn license_survives_restart_with_missing_dir() {
        let (pubkey, signing) = test_keypair();
        let claims = claims(&device_id(), "pro", now() + 3600);
        let token = sign_jwt(&claims, &signing);
        let dir = std::env::temp_dir().join(format!("iv-jwt-license-test-{}", std::process::id()));
        let storage = dir.join("nested").join("license.json");

        let store = test_store(&pubkey);
        let m = LicenseManager::load(storage.clone(), store.clone());
        let lic = test_license(&claims, token);
        let persisted = serde_json::to_string(&lic).unwrap();
        assert!(persisted.contains("\"code\""));
        assert!(persisted.contains("\"license\""));
        assert!(persisted.contains("\"email\""));
        *m.license.lock().unwrap() = Some(lic.clone());
        m.persist(&lic).expect("persist 应自动创建目录并成功落盘");

        let m2 = LicenseManager::load(storage.clone(), store.clone());
        assert_eq!(m2.license().expect("重启后应从磁盘读回 JWT"), lic);
        assert!(
            is_pro_with(&m2.license(), &store, &device_id()),
            "重启后仍应判为已解锁"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn persisted_license_roundtrips_cleanly() {
        let (pubkey, signing) = test_keypair();
        let claims = claims(&device_id(), "pro", now() + 3600);
        let token = sign_jwt(&claims, &signing);
        let dir =
            std::env::temp_dir().join(format!("iv-jwt-clean-license-test-{}", std::process::id()));
        let storage = dir.join("license.json");
        std::fs::create_dir_all(&dir).unwrap();
        let original = License {
            code: claims.code.clone(),
            token,
            email: claims.email.clone().unwrap_or_default(),
        };
        std::fs::write(&storage, serde_json::to_string(&original).unwrap()).unwrap();

        let store = test_store(&pubkey);
        let m = LicenseManager::load(storage, store);
        let lic = m.license().expect("标准许可证应可正常加载");
        assert_eq!(lic.code, claims.code);
        assert_eq!(lic.email, claims.email.unwrap());
        assert!(
            is_pro_with(&m.license(), &test_store(&pubkey), &device_id()),
            "标准许可证应判为已解锁"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// is_pro_with 判定表：纯函数全分支（真实 store + 真实 device_id，只动被测维度）。
    #[test]
    fn is_pro_with_table() {
        let (pubkey, signing) = test_keypair();
        let store = test_store(&pubkey);
        let dev = device_id();
        let good_claims = claims(&dev, "pro", now() + 3600);
        let good_token = sign_jwt(&good_claims, &signing);
        let good = || test_license(&good_claims, good_token.clone());

        assert!(!is_pro_with(&None, &store, &dev), "无许可证 → free");
        let bad = License {
            code: "X".into(),
            token: "AAAA.BBBB.CCCC".into(),
            email: String::new(),
        };
        assert!(!is_pro_with(&Some(bad), &store, &dev), "非法 JWT → free");
        let expired_claims = claims(&dev, "pro", now() - 1);
        let expired = test_license(&expired_claims, sign_jwt(&expired_claims, &signing));
        assert!(!is_pro_with(&Some(expired), &store, &dev), "过期 → free");
        assert!(
            !is_pro_with(&Some(good()), &store, "other-device"),
            "设备不匹配 → free"
        );
        // store 缺对应等级公钥（验签材料缺失：吊销/换 key 的离线等价类）→ free。
        let nokey = StoreConfig {
            public_keys: HashMap::new(),
            ..store.clone()
        };
        assert!(!is_pro_with(&Some(good()), &nokey, &dev), "无公钥 → free");
        assert!(is_pro_with(&Some(good()), &store, &dev), "合法 → pro");
    }

    /// status() 同源派生表：features 永不脱离 status（free ⇒ 空）。
    /// dev 与 release 行为一致，无条件严格断言；过期用例若出现 free+非空即回归。
    #[test]
    fn status_features_co_derived() {
        let (pubkey, signing) = test_keypair();
        let dev = device_id();
        let store = test_store(&pubkey);
        let dir = temp_dir_unique("status-features");
        let storage = dir.join("license.json");
        let m = LicenseManager::load(storage, store);

        // 合法 JWT → pro + 含 pro 位。
        let good_claims = claims(&dev, "pro", now() + 3600);
        let good = test_license(&good_claims, sign_jwt(&good_claims, &signing));
        *m.license.lock().unwrap() = Some(good);
        let info = m.status();
        assert_eq!(info.status, "pro");
        assert!(
            info.features.iter().any(|f| f == FEATURE_PRO),
            "合法应含 pro 位"
        );

        // 过期 JWT → free + 空。
        let bad_claims = claims(&dev, "pro", now() - 1);
        let bad = test_license(&bad_claims, sign_jwt(&bad_claims, &signing));
        *m.license.lock().unwrap() = Some(bad);
        let info = m.status();
        assert_eq!(info.status, "free", "过期判 free");
        assert!(info.features.is_empty(), "过期 features 置空");

        let _ = std::fs::remove_dir_all(&dir);
    }

    static MOCK_SEQ: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

    fn temp_dir_unique(tag: &str) -> PathBuf {
        let n = MOCK_SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        std::env::temp_dir().join(format!("iv-lic-{tag}-{}-{n}", std::process::id()))
    }

    fn mock_store(api_base: &str, pubkey_b64: &str) -> StoreConfig {
        StoreConfig {
            api_base: api_base.into(),
            buy_url: None,
            product: "z-viewer".into(),
            license_level: "pro".into(),
            public_keys: HashMap::from([("pro".to_string(), pubkey_b64.to_string())]),
            license_file_name: "license.json".into(),
            activate_path: "/activate".into(),
            verify_path: "/verify".into(),
            deactivate_path: "/deactivate".into(),
            analytics_path: String::new(),
            analytics_token: String::new(),
        }
    }

    /// 最小 localhost HTTP mock（std-only，无新依赖）：按给定顺序 serving 既定响应。
    /// 非阻塞 accept + 总时限：manager 侧 10s 超时内必有结论，测试不会永久挂死。
    /// 返回 (api_base, stats, server_handle)；stats 永久区分三类故障：
    /// served==0（服务端没见到连接：connect 被拒/代理干扰）vs served==期望值
    /// （服务端已响应：客户端读侧故障）vs join 失败（服务端 panic：队列耗尽）。
    /// 用例结束 join + 断言 served 计数（需求满足后自行退出）。
    #[derive(Clone, Default)]
    struct MockStats {
        accepted: std::sync::Arc<std::sync::atomic::AtomicUsize>,
        served: std::sync::Arc<std::sync::atomic::AtomicUsize>,
    }

    impl MockStats {
        fn accepted(&self) -> usize {
            self.accepted.load(std::sync::atomic::Ordering::SeqCst)
        }
        fn served(&self) -> usize {
            self.served.load(std::sync::atomic::Ordering::SeqCst)
        }
    }

    fn mock_server(
        responses: Vec<(u16, String)>,
    ) -> (String, MockStats, std::thread::JoinHandle<()>) {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("mock 绑定回环");
        let port = listener.local_addr().expect("取 mock 端口").port();
        listener.set_nonblocking(true).expect("mock 非阻塞");
        let total = responses.len();
        let stats = MockStats::default();
        let thread_stats = stats.clone();
        let handle = std::thread::spawn(move || {
            let mut responses = responses.into_iter();
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
            let mut served = 0usize;
            while served < total && std::time::Instant::now() < deadline {
                match listener.accept() {
                    Ok((stream, _)) => {
                        thread_stats
                            .accepted
                            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                        if let Err(e) = mock_respond(stream, &mut responses) {
                            // 诊断性日志（测试输出）：服务端读/写失败即 announce，
                            // 否则客户端断言背锅时无法区分是谁先掉线。
                            eprintln!("MOCK[{port}] respond failed: {e:?}");
                            break;
                        }
                        served += 1;
                        thread_stats
                            .served
                            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(std::time::Duration::from_millis(5));
                    }
                    Err(e) => {
                        eprintln!("MOCK[{port}] accept failed: {e:?}");
                        break;
                    }
                }
            }
        });
        (format!("http://127.0.0.1:{port}"), stats, handle)
    }

    /// 读完一个 POST 请求（头 + Content-Length 体），按序回一个既定响应。
    fn mock_respond(
        mut stream: std::net::TcpStream,
        next: &mut impl Iterator<Item = (u16, String)>,
    ) -> std::io::Result<()> {
        use std::io::{Read, Write};
        // accept 继承了监听器的非阻塞模式：逐连接改回阻塞——否则数据未到时
        // read 报 WouldBlock， piecemeal 的客户端发送在高负载下必现此竞态
        // （根因：WouldBlock 被 ? 传出，服务端掉线，客户端 send 失败）。
        // 改回阻塞后读超时（10s）语义恢复正常，测试不会永久挂死。
        stream.set_nonblocking(false)?;
        stream.set_read_timeout(Some(std::time::Duration::from_secs(10)))?;
        let mut buf = Vec::with_capacity(4096);
        let mut chunk = [0u8; 1024];
        // 读到头结束。
        let head_len = loop {
            let n = stream.read(&mut chunk)?;
            if n == 0 {
                break buf.len();
            }
            buf.extend_from_slice(&chunk[..n]);
            if let Some(p) = find_header_end(&buf) {
                break p;
            }
        };
        // 按 Content-Length 读完体（小 JSON 通常一次即达，此处只为稳妥；大小写不敏感）。
        let body_len: usize = std::str::from_utf8(&buf[..head_len.min(buf.len())])
            .unwrap_or_default()
            .lines()
            .filter_map(|l| l.split_once(':'))
            .find(|(k, _)| k.trim().eq_ignore_ascii_case("content-length"))
            .and_then(|(_, v)| v.trim().parse().ok())
            .unwrap_or(0);
        while buf.len() < head_len + body_len {
            let n = stream.read(&mut chunk)?;
            if n == 0 {
                break;
            }
            buf.extend_from_slice(&chunk[..n]);
        }
        let (status, body) = next.next().expect("mock 响应队列不应耗尽");
        let reason = match status {
            200 => "OK",
            401 => "Unauthorized",
            _ => "Error",
        };
        write!(
            stream,
            "HTTP/1.1 {status} {reason}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        )?;
        stream.flush()?;
        Ok(())
    }

    fn find_header_end(buf: &[u8]) -> Option<usize> {
        buf.windows(4).position(|w| w == b"\r\n\r\n").map(|p| p + 4)
    }

    /// 回收 mock 线程并断言服务计数：served!=期望值说明服务端没见到连接
    /// （connect 被拒/代理干扰）或有多余连接；join 失败则是服务端 panic
    /// （如响应队列耗尽）——三类故障在此分流，客户端断言不再背锅。
    fn join_mock(server: std::thread::JoinHandle<()>, stats: &MockStats, expected: usize) {
        server.join().expect("mock 线程不应 panic");
        assert_eq!(
            stats.served(),
            expected,
            "mock 服务计数异常（accepted={}）：服务端没见到足量连接=传输层故障",
            stats.accepted()
        );
    }

    /// 激活成功：签名 JWT 落盘 → 重启读回 → LicenseInfo 载荷断言。
    #[test]
    fn activate_success_persists_and_roundtrips() {
        let (pubkey, signing) = test_keypair();
        let dev = device_id();
        let c = claims(&dev, "pro", now() + 3600);
        let token = sign_jwt(&c, &signing);
        let (api_base, stats, server) =
            mock_server(vec![(200, format!(r#"{{"license":"{token}"}}"#))]);
        let dir = temp_dir_unique("activate");
        let storage = dir.join("license.json");
        let store = mock_store(&api_base, &pubkey);
        let m = LicenseManager::load(storage.clone(), store);
        // 大小写归一化也在覆盖内：小写 code + 大写邮箱仍能激活。
        // 失败时带 mock 计数：served==0 即服务端没见到连接（传输层故障）。
        let lic = match tauri::async_runtime::block_on(
            m.activate("abcd-efgh-jkmn-pqrs".into(), "Buyer@Example.COM".into()),
        ) {
            Ok(lic) => lic,
            Err(e) => panic!(
                "激活失败: {e:?} (mock accepted={} served={})",
                stats.accepted(),
                stats.served()
            ),
        };
        assert_eq!(lic.code, "ABCD-EFGH-JKMN-PQRS");
        assert_eq!(lic.email, "buyer@example.com");
        assert_eq!(lic.token, token);
        assert!(storage.is_file(), "应落盘");
        // 重启读回（temp-dir round-trip）。
        let m2 = LicenseManager::load(storage.clone(), m.store_config());
        assert_eq!(m2.license().expect("重启应读回"), lic);
        // LicenseInfo 载荷断言。
        let info = m2.status();
        assert_eq!(info.level.as_deref(), Some("pro"));
        assert_eq!(info.level_label.as_deref(), Some("专业版"));
        assert_eq!(info.code.as_deref(), Some("ABCD-EFGH-JKMN-PQRS"));
        assert_eq!(info.email.as_deref(), Some("buyer@example.com"));
        assert_eq!(info.device_id.as_deref(), Some(dev.as_str()));
        join_mock(server, &stats, 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 在线续验成功：服务端新 JWT 被换入本地（返回 None 表成功）。
    #[test]
    fn verify_online_refreshes_jwt() {
        let (pubkey, signing) = test_keypair();
        let dev = device_id();
        let c = claims(&dev, "pro", now() + 3600);
        let token = sign_jwt(&c, &signing);
        let c2 = claims(&dev, "pro", now() + 7200);
        let token2 = sign_jwt(&c2, &signing);
        assert_ne!(token, token2);
        let (api_base, stats, server) =
            mock_server(vec![(200, format!(r#"{{"license":"{token2}"}}"#))]);
        let dir = temp_dir_unique("refresh");
        let storage = dir.join("license.json");
        let m = LicenseManager::load(storage, mock_store(&api_base, &pubkey));
        *m.license.lock().unwrap() = Some(test_license(&c, token));
        let out = tauri::async_runtime::block_on(m.verify_online());
        assert!(
            out.is_none(),
            "续验成功返回 None (mock accepted={} served={})",
            stats.accepted(),
            stats.served()
        );
        assert_eq!(
            m.license().expect("凭证应保留").token,
            token2,
            "应换入服务端新 JWT"
        );
        join_mock(server, &stats, 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 在线续验吊销：401 + CDK_REVOKED → 清内存 + 删文件 + 判 free。
    #[test]
    fn verify_online_revoked_clears() {
        let (pubkey, signing) = test_keypair();
        let dev = device_id();
        let c = claims(&dev, "pro", now() + 3600);
        let token = sign_jwt(&c, &signing);
        let (api_base, stats, server) = mock_server(vec![(
            401,
            r#"{"error":"CDK_REVOKED","message":"revoked"}"#.to_string(),
        )]);
        let dir = temp_dir_unique("revoked");
        let storage = dir.join("license.json");
        let store = mock_store(&api_base, &pubkey);
        let m = LicenseManager::load(storage.clone(), store.clone());
        let lic = test_license(&c, token);
        *m.license.lock().unwrap() = Some(lic.clone());
        m.persist(&lic).expect("预置磁盘授权");
        assert!(storage.is_file());

        let out = tauri::async_runtime::block_on(m.verify_online());
        assert!(
            out.is_some(),
            "吊销应返回最新状态 (mock accepted={} served={})",
            stats.accepted(),
            stats.served()
        );
        assert!(m.license().is_none(), "应清空内存授权");
        assert!(!storage.is_file(), "应删除磁盘授权");
        assert!(!is_pro_with(&m.license(), &store, &dev), "吊销后判 free");
        let info = out.unwrap();
        assert!(
            info.code.is_none() && info.email.is_none(),
            "清空后载荷无残留"
        );
        join_mock(server, &stats, 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 在线续验网络失败：连接拒绝 → 静默 None，凭证保留（优雅降级）。
    #[test]
    fn verify_online_network_failure_grace() {
        let (pubkey, signing) = test_keypair();
        let dev = device_id();
        let c = claims(&dev, "pro", now() + 3600);
        let token = sign_jwt(&c, &signing);
        // 占用后立即释放的端口 → 连接拒绝（快失败，不耗 10s 超时）。
        let port = {
            let l = std::net::TcpListener::bind("127.0.0.1:0").expect("取空闲端口");
            l.local_addr().expect("读端口").port()
        };
        let dir = temp_dir_unique("offline");
        let storage = dir.join("license.json");
        let m = LicenseManager::load(
            storage,
            mock_store(&format!("http://127.0.0.1:{port}"), &pubkey),
        );
        *m.license.lock().unwrap() = Some(test_license(&c, token));
        let out = tauri::async_runtime::block_on(m.verify_online());
        assert!(out.is_none(), "网络失败静默 None");
        assert!(m.license().is_some(), "凭证保留等重验");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 注销成功：服务端 unbound → 清内存 + 删文件。
    #[test]
    fn deactivate_success_clears() {
        let (pubkey, signing) = test_keypair();
        let dev = device_id();
        let c = claims(&dev, "pro", now() + 3600);
        let token = sign_jwt(&c, &signing);
        let (api_base, stats, server) = mock_server(vec![(200, r#"{"unbound":true}"#.to_string())]);
        let dir = temp_dir_unique("deactivate");
        let storage = dir.join("license.json");
        let m = LicenseManager::load(storage.clone(), mock_store(&api_base, &pubkey));
        let lic = test_license(&c, token);
        *m.license.lock().unwrap() = Some(lic.clone());
        m.persist(&lic).expect("预置磁盘授权");
        if let Err(e) = tauri::async_runtime::block_on(m.deactivate()) {
            panic!(
                "注销失败: {e:?} (mock accepted={} served={})",
                stats.accepted(),
                stats.served()
            );
        }
        assert!(m.license().is_none(), "应清空内存授权");
        assert!(!storage.is_file(), "应删除磁盘授权");
        join_mock(server, &stats, 1);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
