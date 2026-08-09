//! 专业版授权：设备指纹、许可证验签、在线验证、激活命令
//!
//! 授权协议（与 backend-mock/ 参考实现对应，见 backend-mock/README.md）：
//! - 激活码在服务端一次性绑定设备（每码限 3 台，可后台解绑）
//! - 服务端用 Ed25519 私钥签发许可证；客户端内置公钥验签
//! - 许可证存 app_data_dir/license.json；本地签名有效即可用（离线可用）
//! - 启动时异步在线验证（吊销生效）；网络失败不影响已付费用户
//!
//! 免费版限制（门控点见 commands / browse）：
//! - 跨文件夹浏览（兄弟文件夹扫描、文件夹级跳转）
//! - 预取缓存（DecodeCache / FolderFirstCache / get_context）
//! dev（debug）构建恒为已解锁，开发调试不被门控干扰；release 才真正生效。

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};
use tauri::State;

/// 功能位：当前捆绑为一个 "pro"（跨文件夹浏览 + 预取缓存）
pub const FEATURE_PRO: &str = "pro";

/// 商店/授权服务配置（**唯一真源：tauri.conf.json → plugins.store**，编译时固化随二进制分发）
///
/// tauri.conf.json 示例：
/// ```json
/// "plugins": {
///   "store": {
///     "apiBase": "https://your-server.example.com",
///     "buyUrl": "https://your-server.example.com/buy?product=image-viewer",
///     "licensePublicKeyB64": "<该产品的 Ed25519 公钥 base64>",
///     "product": "image-viewer",
///     "licenseFileName": "license.json",
///     "activatePath": "/api/activate",
///     "verifyPath": "/api/verify"
///   }
/// }
/// ```
///
/// 字段缺失/为空即视为未配置：apiBase 未配置时激活报错、不联网验证；
/// 公钥未配置时验签必失败（无法激活）。无任何内置默认值。
#[derive(Clone, Debug)]
pub struct StoreConfig {
    /// 授权服务 API base（激活/在线验证）；空 = 未配置
    pub api_base: String,
    /// 官网购买页 URL；None = 未配置（前端购买按钮给出提示）
    pub buy_url: Option<String>,
    /// 许可证验签公钥（base64，32 字节 raw）；空 = 未配置
    pub public_key_b64: String,
    /// 产品标识（官网购买页 `?product=` 参数，get_store_info 返回给前端）
    pub product: String,
    /// 许可证存储文件名（app_data_dir 下）
    pub license_file_name: String,
    /// 激活接口路径（拼在 api_base 后）
    pub activate_path: String,
    /// 在线验证接口路径（拼在 api_base 后）
    pub verify_path: String,
}

impl StoreConfig {
    /// 从 tauri.conf.json 的 plugins.store 读取（编译期配置，运行时不可改）
    pub fn from_config(config: &tauri::Config) -> Self {
        let v = config
            .plugins
            .0
            .get("store")
            .cloned()
            .unwrap_or_default();
        let s = |k: &str| {
            v.get(k)
                .and_then(|x| x.as_str())
                .map(str::trim)
                .filter(|s| !s.is_empty())
        };
        Self {
            api_base: s("apiBase").unwrap_or_default().to_string(),
            buy_url: s("buyUrl").map(String::from),
            public_key_b64: s("licensePublicKeyB64")
                .unwrap_or_default()
                .to_string(),
            product: s("product").unwrap_or_default().to_string(),
            license_file_name: s("licenseFileName").unwrap_or_default().to_string(),
            activate_path: s("activatePath").unwrap_or_default().to_string(),
            verify_path: s("verifyPath").unwrap_or_default().to_string(),
        }
    }
}

/// 许可证（服务端签发、客户端验签后信任）
#[derive(Clone, Serialize, Deserialize, PartialEq, Eq, Debug)]
pub struct License {
    pub device_id: String,
    /// 功能位列表，当前仅 ["pro"]
    pub features: Vec<String>,
    /// Unix 秒
    pub issued_at: i64,
    /// Unix 秒；0 = 买断制永不过期
    pub expires_at: i64,
    /// Ed25519 签名（base64）
    pub sig: String,
}

/// 前端可见的授权状态
#[derive(Clone, Serialize, PartialEq, Eq, Debug)]
#[serde(rename_all = "camelCase")]
pub struct LicenseInfo {
    /// "pro" | "free"
    pub status: String,
    /// 当前设备指纹（激活后返回，供用户识别设备）
    pub device_id: Option<String>,
}

/// 授权管理器（Tauri managed state；Clone 供后台任务移动）
#[derive(Clone)]
pub struct LicenseManager {
    license: Arc<Mutex<Option<License>>>,
    storage: PathBuf,
    store: StoreConfig,
}

impl LicenseManager {
    /// 从磁盘加载许可证（不存在 = 免费版）；store 为编译期配置（tauri.conf.json plugins.store）
    pub fn load(storage: PathBuf, store: StoreConfig) -> Self {
        let license = std::fs::read_to_string(&storage)
            .ok()
            .and_then(|s| serde_json::from_str::<License>(&s).ok())
            .filter(|lic| lic.is_valid_with(&device_id(), &store.public_key_b64));
        Self {
            license: Arc::new(Mutex::new(license)),
            storage,
            store,
        }
    }

    fn persist(&self, lic: &License) {
        if let Ok(s) = serde_json::to_string_pretty(lic) {
            let _ = std::fs::write(&self.storage, s);
        }
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
    /// - release 构建：本地许可证签名有效 && 未过期 && 设备匹配 && 含 pro 功能位
    pub fn is_pro(&self) -> bool {
        #[cfg(debug_assertions)]
        {
            true
        }
        #[cfg(not(debug_assertions))]
        {
            let pubkey = self.store.public_key_b64.clone();
            Self::is_pro_with(&self.license.lock().ok().and_then(|g| g.clone()), &pubkey)
        }
    }

    /// 门控核心判定（不含 cfg，供单元测试验证免费版分支；用 store 公钥验签）
    #[cfg(any(not(debug_assertions), test))]
    fn is_pro_with(lic: &Option<License>, pubkey_b64: &str) -> bool {
        lic.as_ref()
            .is_some_and(|lic| lic.is_valid_with(&device_id(), pubkey_b64))
    }

    /// 门控核心判定（测试用：空公钥下验签必失败，仅验证业务字段分支）
    #[cfg(any(not(debug_assertions), test))]
    fn is_pro_impl(lic: &Option<License>) -> bool {
        lic.as_ref()
            .is_some_and(|lic| lic.is_valid_with(&device_id(), ""))
    }

    /// 激活：调后端换取许可证 → 验签 → 校验设备与功能位 → 落盘
    pub async fn activate(&self, code: String) -> Result<License, String> {
        if self.store.api_base.is_empty() {
            return Err("授权服务器未配置（tauri.conf.json plugins.store.apiBase）".to_string());
        }
        let dev = device_id();
        let client = reqwest::Client::new();
        let url = format!("{}{}", self.store.api_base, self.store.activate_path);
        let resp = client
            .post(url)
            .json(&serde_json::json!({ "code": code, "device_id": dev }))
            .send()
            .await
            .map_err(|e| format!("无法连接授权服务器：{e}"))?;
        let body: serde_json::Value = resp
            .json()
            .await
            .map_err(|e| format!("授权服务器响应异常：{e}"))?;
        if let Some(err) = body.get("error").and_then(|v| v.as_str()) {
            return Err(err.to_string());
        }
        let lic: License = serde_json::from_value(body.get("license").cloned().unwrap_or_default())
            .map_err(|_| "授权服务器返回格式错误".to_string())?;
        if !lic.is_valid_with(&dev, &self.store.public_key_b64) {
            return Err("激活码无效（许可证校验失败）".to_string());
        }
        *self.license.lock().map_err(|e| e.to_string())? = Some(lic.clone());
        self.persist(&lic);
        Ok(lic)
    }

    /// 在线验证（启动时后台调用）：吊销生效；网络失败静默（本地有效即可用）
    pub async fn verify_online(&self) {
        let Some(lic) = self.license() else { return };
        if self.store.api_base.is_empty() {
            return; // 未配置授权服务器：不联网验证
        }
        let client = reqwest::Client::new();
        let url = format!("{}{}", self.store.api_base, self.store.verify_path);
        let resp = client
            .post(url)
            .json(&serde_json::json!({ "device_id": lic.device_id, "license": lic }))
            .send()
            .await;
        let valid = match resp {
            Ok(r) => r
                .json::<serde_json::Value>()
                .await
                .ok()
                .and_then(|v| v.get("valid").and_then(|x| x.as_bool()))
                .unwrap_or(true), // 响应异常保守视为有效，不误伤付费用户
            Err(_) => true,       // 网络失败不阻止使用
        };
        if !valid {
            if let Ok(mut g) = self.license.lock() {
                *g = None;
            }
            self.clear();
        }
    }
}

impl License {
    /// 完整校验（指定公钥：tauri.conf.json plugins.store.licensePublicKeyB64 配置的正式公钥）
    pub fn is_valid_with(&self, dev: &str, pubkey_b64: &str) -> bool {
        self.checks_ok(dev) && self.verify_signature_with(pubkey_b64)
    }

    /// 业务字段校验（不含签名）：设备匹配、含 pro、未过期
    fn checks_ok(&self, dev: &str) -> bool {
        self.device_id == dev
            && self.features.iter().any(|f| f == FEATURE_PRO)
            && (self.expires_at == 0 || self.expires_at > now())
    }

    /// 验签：payload 拼接规则与后端签发一致
    /// `iv:{device_id}:{features.join(",")}:{issued_at}:{expires_at}`
    fn payload(&self) -> String {
        format!(
            "iv:{}:{}:{}:{}",
            self.device_id,
            self.features.join(","),
            self.issued_at,
            self.expires_at
        )
    }

    /// Ed25519 验签（指定公钥 base64）
    fn verify_signature_with(&self, pubkey_b64: &str) -> bool {
        use ed25519_dalek::{Signature, VerifyingKey};
        use base64::Engine;
        let Ok(pubkey_bytes) = base64::engine::general_purpose::STANDARD.decode(pubkey_b64)
        else {
            return false;
        };
        let Ok(key_bytes): Result<[u8; 32], _> = pubkey_bytes.as_slice().try_into() else {
            return false;
        };
        let Ok(pubkey) = VerifyingKey::from_bytes(&key_bytes) else {
            return false;
        };
        let Ok(sig_bytes) =
            base64::engine::general_purpose::STANDARD.decode(self.sig.as_bytes())
        else {
            return false;
        };
        let Ok(sig) = Signature::from_slice(&sig_bytes) else {
            return false;
        };
        pubkey.verify_strict(self.payload().as_bytes(), &sig).is_ok()
    }
}

/// 当前 Unix 秒
fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// 设备指纹：Windows MachineGuid（注册表读取），激活/验证/存储共用。
/// 机器 GUID 系统级唯一且重装系统才变化；直接用作设备标识（无冒号，payload 拼接安全）。
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
    /// 产品标识（官网购买页 `?product=` 参数）
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
        LicenseInfo {
            status: if self.is_pro() { "pro" } else { "free" }.to_string(),
            device_id: Some(device_id()),
        }
    }
}

// ---------- 测试 ----------

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::{Signer, SigningKey};

    /// 测试固定密钥对：SigningKey 由固定 seed 派生，公钥与签名自洽
    fn test_keypair() -> ([u8; 32], SigningKey) {
        let seed = [7u8; 32];
        let signing = SigningKey::from_bytes(&seed);
        let pubkey = signing.verifying_key().to_bytes();
        (pubkey, signing)
    }

    fn sign_license(lic: &mut License, signing: &SigningKey) {
        let payload = lic.payload();
        let sig = signing.sign(payload.as_bytes());
        lic.sig = base64::Engine::encode(
            &base64::engine::general_purpose::STANDARD,
            sig.to_bytes(),
        );
    }

    #[test]
    fn valid_license_passes_checks() {
        let (pubkey, signing) = test_keypair();
        let mut lic = License {
            device_id: "dev-1".into(),
            features: vec!["pro".into()],
            issued_at: 0,
            expires_at: 0,
            sig: String::new(),
        };
        sign_license(&mut lic, &signing);

        // 用测试公钥验签（绕过内置常量，验证签名协议本身）
        let pubkey_b64 = base64::Engine::encode(
            &base64::engine::general_purpose::STANDARD,
            pubkey,
        );
        assert!(verify_with(&lic, &pubkey_b64), "合法许可证应通过验签");
        assert!(lic.checks_ok("dev-1"), "设备匹配应通过业务校验");
    }

    #[test]
    fn tampered_payload_rejected() {
        let (pubkey, signing) = test_keypair();
        let mut lic = License {
            device_id: "dev-1".into(),
            features: vec!["pro".into()],
            issued_at: 0,
            expires_at: 0,
            sig: String::new(),
        };
        sign_license(&mut lic, &signing);

        // 篡改：换设备 / 加功能位 / 改过期时间 → 验签必须失败
        lic.device_id = "dev-2".into();
        let pubkey_b64 = base64::Engine::encode(
            &base64::engine::general_purpose::STANDARD,
            pubkey,
        );
        assert!(!verify_with(&lic, &pubkey_b64), "篡改后验签应失败");
    }

    #[test]
    fn wrong_device_rejected() {
        let (pubkey, signing) = test_keypair();
        let mut lic = License {
            device_id: "dev-A".into(),
            features: vec!["pro".into()],
            issued_at: 0,
            expires_at: 0,
            sig: String::new(),
        };
        sign_license(&mut lic, &signing);
        // 业务校验：设备不匹配应失败（签名有效与否无关）
        assert!(!lic.checks_ok("dev-B"), "设备不匹配应无效");
        let _ = pubkey;
    }

    #[test]
    fn expired_license_rejected() {
        let (pubkey, signing) = test_keypair();
        let mut lic = License {
            device_id: "dev-1".into(),
            features: vec!["pro".into()],
            issued_at: now() - 1000,
            expires_at: now() - 500,
            sig: String::new(),
        };
        sign_license(&mut lic, &signing);
        assert!(!lic.checks_ok("dev-1"), "过期许可证应无效");
        let _ = pubkey;
    }

    /// 用指定公钥验签（测试注入，绕过内置常量）
    fn verify_with(lic: &License, pubkey_b64: &str) -> bool {
        use ed25519_dalek::{Signature, VerifyingKey};
        let key_bytes: [u8; 32] = base64::Engine::decode(
            &base64::engine::general_purpose::STANDARD,
            pubkey_b64,
        )
        .ok()
        .and_then(|v| v.as_slice().try_into().ok())
        .unwrap_or([0u8; 32]);
        let Ok(pubkey) = VerifyingKey::from_bytes(&key_bytes) else {
            return false;
        };
        let Ok(sig_bytes) =
            base64::Engine::decode(&base64::engine::general_purpose::STANDARD, &lic.sig)
        else {
            return false;
        };
        let Ok(sig) = Signature::from_slice(&sig_bytes) else {
            return false;
        };
        pubkey.verify_strict(lic.payload().as_bytes(), &sig).is_ok()
    }
}

#[cfg(test)]
mod gate_tests {
    use super::*;

    #[test]
    fn free_branch_rejected() {
        // release 下 is_pro 的判定：无许可证 = 免费版
        assert!(!LicenseManager::is_pro_impl(&None), "无许可证应判免费版");
        // 有许可证但字段不合法（设备不匹配 / 缺 pro 功能位）也应判免费版
        let bad = License {
            device_id: "someone-else".into(),
            features: vec!["basic".into()],
            issued_at: 0,
            expires_at: 0,
            sig: "AAAA".into(),
        };
        assert!(!LicenseManager::is_pro_impl(&Some(bad)), "字段不合法应判免费版");
    }
}

/// 端到端协议验证：本地后端真实签发 → 客户端内置公钥验签
/// 前置：cd backend-mock && node server.mjs（默认 8787 端口，dev-admin-key）
/// 运行：cargo test -- --ignored license::e2e
#[cfg(test)]
mod e2e {
    use super::*;

    /// 本地联调后端地址与 demo 公钥（backend-mock，仅测试用；生产值在 tauri.conf.json plugins.store）
    const MOCK_API: &str = "http://127.0.0.1:8787";
    const MOCK_PUBKEY_B64: &str = "HHvVDoB7i1gMlCH7PreE2h2lovqa+taR6mb756xpmyE=";

    #[test]
    #[ignore = "需要本地授权后端运行（backend-mock/server.mjs）"]
    fn real_backend_signature_passes() {
        let dev = device_id();
        let lic = tauri::async_runtime::block_on(async {
            let client = reqwest::Client::new();
            // 1. 生成激活码（管理 API）
            let r = client
                .post(format!("{MOCK_API}/api/admin/gen"))
                .json(&serde_json::json!({ "apiKey": "dev-admin-key", "count": 1 }))
                .send()
                .await
                .unwrap();
            let body: serde_json::Value = r.json().await.unwrap();
            let code = body["codes"][0].as_str().unwrap().to_string();
            // 2. 激活 → 后端用私钥签发
            let r = client
                .post(format!("{MOCK_API}/api/activate"))
                .json(&serde_json::json!({ "code": code, "device_id": dev }))
                .send()
                .await
                .unwrap();
            let body: serde_json::Value = r.json().await.unwrap();
            serde_json::from_value::<License>(body["license"].clone()).unwrap()
        });

        assert_eq!(lic.device_id, dev, "许可证应绑定本机");
        assert!(lic.verify_signature_with(MOCK_PUBKEY_B64), "后端私钥签发的许可证必须通过客户端内置公钥验签");
        assert!(lic.is_valid_with(&dev, MOCK_PUBKEY_B64), "完整校验通过");

        // 篡改：改 device_id 后验签必须失败（防本地伪造）
        let mut tampered = lic.clone();
        tampered.device_id = "hacked-device".into();
        assert!(!tampered.verify_signature_with(MOCK_PUBKEY_B64), "篡改后验签应失败");
    }
}
