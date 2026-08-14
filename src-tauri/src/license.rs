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
//! dev（debug）构建恒为已解锁，开发调试不被门控干扰；release 才真正生效。

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use base64::Engine;
use ed25519_dalek::{Signature, VerifyingKey};
use serde::{Deserialize, Serialize};
use tauri::State;

/// 功能位：当前捆绑为一个 "pro"（跨文件夹浏览 + 预取缓存）
pub const FEATURE_PRO: &str = "pro";

/// soft-candy 签发 JWT 时固定的 issuer。
pub const LICENSE_ISSUER: &str = "soft-candy";

/// 商店/授权服务配置（**唯一真源：tauri.conf.json → plugins.store**，编译时固化随二进制分发）
///
/// tauri.conf.json 示例：
/// ```json
/// "plugins": {
///   "store": {
///     "apiBase": "https://your-server.example.com",
///     "buyUrl": "https://your-server.example.com/p/image-viewer",
///     "product": "image-viewer",
///     "licenseLevel": "pro",
///     "licensePublicKeys": {
///       "pro": "<image-viewer/pro 的 Ed25519 公钥，raw/DER base64 或 PEM>"
///     },
///     "licenseFileName": "license.json",
///     "activatePath": "/api/v1/apps/image-viewer/activate",
///     "verifyPath": "/api/v1/apps/image-viewer/verify",
///     "analyticsPath": "/api/v1/apps/image-viewer/analytics"
///   }
/// }
/// ```
///
/// 字段缺失/为空即视为未配置：apiBase 未配置时激活报错、不联网验证；
/// 对应等级公钥未配置时验签必失败（无法激活）。无任何内置默认值。
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
    /// 会话统计上报接口路径（拼在 api_base 后）；空 = 不上报
    pub analytics_path: String,
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
        if public_keys.is_empty() {
            // 兼容旧单等级配置：单个公钥按当前 licenseLevel 生效。
            if let Some(key) = s("licensePublicKeyB64") {
                public_keys.insert(license_level.clone(), key.to_string());
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
            analytics_path: s("analyticsPath").unwrap_or_default().to_string(),
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
    pub features: Vec<String>,
    pub iat: i64,
    pub exp: i64,
}

/// 许可证（soft-candy 返回的 JWT；本地只保存原始令牌，验签结果即时计算）
#[derive(Clone, Serialize, Deserialize, PartialEq, Eq, Debug)]
pub struct License {
    pub token: String,
}

impl License {
    pub fn claims(&self) -> Option<Claims> {
        parse_claims_from_token(&self.token).ok()
    }

    pub fn device_id(&self) -> Option<String> {
        self.claims().map(|c| c.device_id)
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
        verified == claims && claims_ok(&verified, dev, &store.product)
    }
}

/// 前端可见的授权状态
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
}

/// 授权管理器（Tauri managed state；Clone 供后台任务移动）
#[derive(Clone)]
pub struct LicenseManager {
    license: Arc<Mutex<Option<License>>>,
    storage: PathBuf,
    store: StoreConfig,
}

impl LicenseManager {
    /// 从磁盘加载许可证（不存在/格式旧/验签失败 = 免费版）；store 为编译期配置。
    pub fn load(storage: PathBuf, store: StoreConfig) -> Self {
        let license = std::fs::read_to_string(&storage)
            .ok()
            .and_then(|s| serde_json::from_str::<License>(&s).ok())
            .filter(|lic| lic.is_valid_with(&device_id(), &store));
        Self {
            license: Arc::new(Mutex::new(license)),
            storage,
            store,
        }
    }

    /// 许可证落盘：先确保存储目录存在，再写入 JWT。
    fn persist(&self, lic: &License) -> Result<(), String> {
        let s = serde_json::to_string_pretty(lic).map_err(|e| e.to_string())?;
        if let Some(dir) = self.storage.parent() {
            std::fs::create_dir_all(dir)
                .map_err(|e| format!("无法创建许可证目录 {}: {e}", dir.display()))?;
        }
        std::fs::write(&self.storage, s)
            .map_err(|e| format!("无法保存许可证到 {}: {e}", self.storage.display()))
    }

    fn clear(&self) {
        let _ = std::fs::remove_file(&self.storage);
    }

    /// 当前许可证（未验证；校验用 is_pro）
    pub fn license(&self) -> Option<License> {
        self.license.lock().ok()?.clone()
    }

    /// 是否已解锁专业版
    /// - debug 构建恒为 true（开发调试用；`pnpm tauri build --debug` 亦解锁）
    /// - release 构建：本地 JWT 验签有效 && 产品/设备/功能位匹配 && 未过期
    pub fn is_pro(&self) -> bool {
        #[cfg(debug_assertions)]
        {
            true
        }
        #[cfg(not(debug_assertions))]
        {
            let store = self.store.clone();
            Self::is_pro_with(&self.license.lock().ok().and_then(|g| g.clone()), &store)
        }
    }

    /// 门控核心判定（不含 cfg，供单元测试验证免费版分支；用 store 配置的公钥验签）
    #[cfg(any(not(debug_assertions), test))]
    fn is_pro_with(lic: &Option<License>, store: &StoreConfig) -> bool {
        lic.as_ref()
            .is_some_and(|lic| lic.is_valid_with(&device_id(), store))
    }

    /// 门控核心判定（测试用：空公钥下验签必失败，仅验证业务字段分支）
    #[cfg(test)]
    fn is_pro_impl(lic: &Option<License>) -> bool {
        let store = StoreConfig {
            api_base: String::new(),
            buy_url: None,
            product: String::new(),
            license_level: "pro".into(),
            public_keys: HashMap::new(),
            license_file_name: String::new(),
            activate_path: String::new(),
            verify_path: String::new(),
            analytics_path: String::new(),
        };
        lic.as_ref()
            .is_some_and(|lic| lic.is_valid_with(&device_id(), &store))
    }

    /// 激活：调 soft-candy 换取 JWT → 验签 → 校验设备/等级/功能位 → 落盘
    pub async fn activate(&self, code: String) -> Result<License, String> {
        if self.store.api_base.is_empty() {
            return Err("授权服务器未配置（tauri.conf.json plugins.store.apiBase）".to_string());
        }
        let dev = device_id();
        let client = reqwest::Client::new();
        let url = format!("{}{}", self.store.api_base, self.store.activate_path);
        let resp = client
            .post(url)
            .json(&serde_json::json!({
                "code": code,
                "deviceId": dev,
                "level": self.store.license_level,
            }))
            .send()
            .await
            .map_err(|e| format!("无法连接授权服务器：{e}"))?;
        let body: serde_json::Value = resp
            .json()
            .await
            .map_err(|e| format!("授权服务器响应异常：{e}"))?;
        if let Some(msg) = body.get("message").and_then(|v| v.as_str()) {
            return Err(msg.to_string());
        }
        if let Some(code) = body.get("error").and_then(|v| v.as_str()) {
            return Err(format!("激活失败（{code}）"));
        }

        let token = body
            .get("license")
            .and_then(|v| v.as_str())
            .ok_or_else(|| "授权服务器返回格式错误".to_string())?;
        let lic = License {
            token: token.to_string(),
        };
        if !lic.is_valid_with(&dev, &self.store) {
            return Err("激活码无效（JWT 验签失败）".to_string());
        }
        *self.license.lock().map_err(|e| e.to_string())? = Some(lic.clone());
        self.persist(&lic)?;
        Ok(lic)
    }

    /// 在线续验（启动时后台调用）：soft-candy 会返回新 JWT 延长离线宽限期；网络失败静默。
    pub async fn verify_online(&self) {
        let Some(lic) = self.license() else { return };
        let Some(lic_device_id) = lic.device_id() else {
            return;
        };
        if self.store.api_base.is_empty() {
            return; // 未配置授权服务器：不联网验证
        }
        let client = reqwest::Client::new();
        let url = format!("{}{}", self.store.api_base, self.store.verify_path);
        let resp = client
            .post(url)
            .json(&serde_json::json!({
                "deviceId": lic_device_id,
                "license": lic.token,
            }))
            .send()
            .await;
        let Ok(resp) = resp else {
            return; // 网络失败不阻止使用
        };
        let Ok(body) = resp.json::<serde_json::Value>().await else {
            return; // 响应异常保守视为有效，不误伤付费用户
        };
        let valid = body.get("valid").and_then(|v| v.as_bool()).unwrap_or(true);
        if !valid {
            if let Ok(mut g) = self.license.lock() {
                *g = None;
            }
            self.clear();
            return;
        }

        // soft-candy 在线续验成功后会签发新 JWT；本地验签通过后立即换新。
        if let Some(token) = body.get("license").and_then(|v| v.as_str()) {
            let refreshed = License {
                token: token.to_string(),
            };
            if refreshed.is_valid_with(&device_id(), &self.store) {
                if let Ok(mut g) = self.license.lock() {
                    *g = Some(refreshed.clone());
                }
                let _ = self.persist(&refreshed);
            }
        }
    }
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

/// 当前 Unix 秒
fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// 设备指纹：Windows MachineGuid（注册表读取），激活/验证/存储共用。
/// 机器 GUID 系统级唯一且重装系统才变化；直接用作设备标识。
pub fn device_id() -> String {
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
) -> Result<LicenseInfo, String> {
    let code = code.trim().to_uppercase();
    if code.is_empty() {
        return Err("请输入激活码".to_string());
    }
    state.activate(code).await?;
    Ok(state.status())
}

/// 查询授权状态（启动时前端调用，同步按钮禁用态）
#[tauri::command]
pub fn get_license_status(state: State<'_, LicenseManager>) -> LicenseInfo {
    state.status()
}

impl LicenseManager {
    pub fn status(&self) -> LicenseInfo {
        let dev = device_id();
        let claims = self
            .license()
            .and_then(|l| l.claims())
            .filter(|c| c.device_id == dev && c.app == self.store.product);
        LicenseInfo {
            status: if self.is_pro() { "pro" } else { "free" }.to_string(),
            device_id: Some(dev),
            level: claims.as_ref().map(|c| c.level.clone()),
            level_label: claims.as_ref().map(|c| c.level_label.clone()),
        }
    }
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
            product: "image-viewer".into(),
            license_level: "pro".into(),
            public_keys: HashMap::from([("pro".to_string(), pubkey_b64.to_string())]),
            license_file_name: "license.json".into(),
            activate_path: String::new(),
            verify_path: String::new(),
            analytics_path: String::new(),
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
            aud: "image-viewer".into(),
            sub: "ABCD-EFGH-JKMN-PQRS".into(),
            code: "ABCD-EFGH-JKMN-PQRS".into(),
            app: "image-viewer".into(),
            level: level.into(),
            level_label: if level == "pro" { "专业版" } else { level }.into(),
            device_id: device_id.into(),
            features: vec![FEATURE_PRO.into()],
            iat: now() - 60,
            exp: expires_at,
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
        assert!(License { token }.is_valid_with("dev-1", &store));
    }

    #[test]
    fn full_spki_and_pem_public_key_passes() {
        let (pubkey, signing) = test_keypair();
        let claims = claims("dev-1", "pro", now() + 3600);
        let token = sign_jwt(&claims, &signing);

        for key in [spki_public_key(&pubkey), pem_public_key(&pubkey)] {
            let store = test_store(&key);
            assert!(License {
                token: token.clone(),
            }
            .is_valid_with("dev-1", &store));
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
        assert!(!License { token: tampered }.is_valid_with("dev-1", &store));
    }

    #[test]
    fn wrong_app_or_device_rejected() {
        let (pubkey, signing) = test_keypair();
        let mut c = claims("dev-1", "pro", now() + 3600);
        c.app = "other-app".into();
        c.aud = "other-app".into();
        let token = sign_jwt(&c, &signing);
        let store = test_store(&pubkey);
        assert!(!License { token }.is_valid_with("dev-1", &store));

        let c = claims("dev-2", "pro", now() + 3600);
        let token = sign_jwt(&c, &signing);
        assert!(!License { token }.is_valid_with("dev-1", &store));
    }

    #[test]
    fn expired_jwt_rejected() {
        let (pubkey, signing) = test_keypair();
        let claims = claims("dev-1", "pro", now() - 1);
        let token = sign_jwt(&claims, &signing);
        let store = test_store(&pubkey);
        assert!(!License { token }.is_valid_with("dev-1", &store));
    }

    #[test]
    fn missing_level_key_rejected() {
        let (pubkey, signing) = test_keypair();
        let claims = claims("dev-1", "enterprise", now() + 3600);
        let token = sign_jwt(&claims, &signing);
        let store = test_store(&pubkey);
        assert!(!License { token }.is_valid_with("dev-1", &store));
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
        let lic = License { token };
        *m.license.lock().unwrap() = Some(lic.clone());
        m.persist(&lic).expect("persist 应自动创建目录并成功落盘");

        let m2 = LicenseManager::load(storage.clone(), store.clone());
        assert_eq!(m2.license().expect("重启后应从磁盘读回 JWT"), lic);
        assert!(
            LicenseManager::is_pro_with(&m2.license(), &store),
            "重启后仍应判为已解锁"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }
}

#[cfg(test)]
mod gate_tests {
    use super::*;

    #[test]
    fn free_branch_rejected() {
        assert!(!LicenseManager::is_pro_impl(&None), "无许可证应判免费版");
        let bad = License {
            token: "AAAA.BBBB.CCCC".into(),
        };
        assert!(
            !LicenseManager::is_pro_impl(&Some(bad)),
            "非法 JWT 应判免费版"
        );
    }
}
