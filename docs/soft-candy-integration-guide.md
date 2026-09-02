# 软糖铺应用接入指南（授权 · 埋点 · 检查更新）

> 面向需要上架到软糖铺的桌面应用（框架不限，Tauri / wails / Electron 均可）。
> 本文档自包含：客户端需要调用的全部接口、错误码与行为约定都在这一份里，
> 可以直接按它开发；服务端实现细节（数据库、密钥管理、支付）不在范围内。
>
> ✅ 首个真实接入方：看图王（Tauri 2 + Rust，产品标识 `z-viewer`）
> 已按本文档完成激活 + 埋点接入并实测通过，参考实现见第 6 节。z-ffmpeg
> 已按第 4 节协议接入埋点。服务端对任意 slug 都解析公共字段并落库，产品标识改名不影响埋点上报。

## 0. 如何使用本文档（给接入方与 AI 编码助手）

本文档是**自包含的接入契约**，可将整个文件复制到接入方仓库（建议放 `docs/` 下），
交给开发者或 AI 编码助手实施。使用方式：

1. **开始前先向软糖铺收集以下参数**，实现时作为配置项注入（不要硬编码到业务逻辑里）：

   | 参数 | 用途 | 来源 |
   |---|---|---|
   | 生产 Base URL | 替换各接口的 `{base}` | 软糖铺提供 |
   | 产品标识 `{slug}` | 拼接接口路径 | 上架时分配，见第 1 节 |
   | 授权等级名与公钥 PEM | 授权等级（如 `pro`）+ 离线验签公钥，编译进客户端 | 见第 1、3.4 节 |
   | 埋点 Token | 埋点上报鉴权（如启用） | 见第 4.2 节 |

   联调阶段可先用本地地址 `http://localhost:8080`。

2. **建议实施顺序**：按第 3.6 节实现授权状态机（激活 / 续验 / 注销 + 离线降级）
   → 第 4 节埋点上报 → 第 5 节检查更新 → 对照第 7 节清单逐项自查。
3. **必须遵守**：各节中的「客户端行为约定」（幂等、静默失败、超时、令牌覆盖等）是
   契约的一部分，不是可选建议。
4. **约束**：本文档是接口契约的唯一依据；不要发明文档之外的接口、字段或错误码；
   实现与文档冲突时以本文档为准，并向软糖铺确认。

## 1. 接入总览

一个应用接入软糖铺需要用到三组客户端接口（下表），另有一组配置（产品 / 密钥 / 版本）由
软糖铺运营在管理后台完成，应用方不需要调用。

| 能力 | 接口 | 章节 |
|---|---|---|
| 授权激活（激活 / 续验 / 注销激活） | `POST /api/v1/apps/{slug}/activate` / `verify` / `deactivate` | 第 3 节 |
| 埋点上报 | `POST /api/v1/apps/{slug}/analytics` | 第 4 节 |
| 检查更新 | `GET /api/v1/apps/{slug}/latest` | 第 5 节 |

### 上架前需要与软糖铺对齐的 4 件事

1. **产品标识 `slug`**：小写字母数字连字符（如 `z-viewer`），管理接口一经创建不可改，
   所有接口路径都以它为准。定价、激活次数上限（一个激活码可绑定的设备数）、
   可购授权等级（用户下单时选择，产品级默认授权等级已移除）由管理后台配置。
2. **授权等级公钥**：软糖铺为每个授权等级（如 `pro`）生成 Ed25519 密钥对，
   应用方拿到对应等级的**公钥 PEM，编译进客户端**用于离线验签（第 3.4 节）；私钥只在服务端。
3. **埋点 Token**（可选）：管理后台可按产品生成 32 字节 token；配置后埋点上报必须携带。
4. **版本发布**：按 `平台 × 通道` 发布（版本号约定纯数字分段，如 `1.2.10`）；
   客户端启动时调用检查更新接口提示用户手动下载，不做静默更新。

## 2. 通用约定

- **Base URL**：本地联调 `http://localhost:8080`，生产环境域名由软糖铺提供后替换。
- **路径前缀**：`/api/v1`；`{slug}` 为应用的产品标识。
- **请求 / 响应**：均为 JSON，UTF-8。
- **错误响应**统一结构：

```json
{ "error": "ERROR_CODE", "message": "给用户看的中文说明" }
```

## 3. 授权激活（激活 / 续验 / 注销激活）

### 3.1 基本概念

- **双凭证**：激活 / 验证 / 注销激活都需要**购买邮箱**。购买邮箱是下单时填写的邮箱，
  与服务端订单记录做大小写不敏感匹配；服务端不会告知"激活码对应的邮箱"，
  忘记邮箱时凭邮箱 + 订单号走客服找回流程。
- **激活码格式**：`XXXX-XXXX-XXXX-XXXX`，大写字母（避开 `0/O/1/I/L`）与数字，
  接口传入时大小写不敏感。
- **deviceId（机器码）**：8-128 字符，建议用稳定的机器指纹哈希；
  **换机器码 = 换设备**，会占用一个新的设备名额。
- **授权令牌（license）**：Ed25519 签名的 JWT，激活 / 续验成功后返回，
  **客户端必须本地保存**，后续验证与离线宽限期都靠它。

### 3.2 激活

把激活码绑定到当前设备，获取授权令牌。

#### 请求

```
POST {base}/api/v1/apps/{slug}/activate
Content-Type: application/json
```

```json
{
  "code": "SDX4-K9TP-2M7Q-W3HZ",
  "deviceId": "machine-hash-00000001",
  "email": "buyer@example.com",
  "level": "pro"
}
```

| 字段 | 必填 | 说明 |
|---|---|---|
| `code` | 是 | 激活码，大小写不敏感（服务端会转大写），须符合 4×4 格式 |
| `deviceId` | 是 | 机器码，8-128 字符 |
| `email` | 是 | 购买邮箱，大小写不敏感 |
| `level` | 否 | 授权等级名，如 `pro`；省略时使用激活码所属等级。**支持多等级的应用建议省略**：写死 `level` 时，用户持其他等级（如 `ultra`）激活码激活会返回 `LEVEL_MISMATCH` |

#### 成功 `200`

```json
{
  "license": "eyJhbGciOiJFZERTQSIsInR5cCI6IkpXVCJ9...",
  "expiresAt": "2026-09-14T12:00:00+08:00",
  "appSlug": "sujiya",
  "level": "pro",
  "levelLabel": "专业版",
  "deviceId": "machine-hash-00000001"
}
```

| 字段 | 说明 |
|---|---|
| `license` | Ed25519 签名的 JWT，本地保存，后续验证与离线宽限期都靠它 |
| `expiresAt` | 令牌到期时间（RFC3339）；到期前应在线验证续期 |
| `appSlug` / `level` / `levelLabel` / `deviceId` | 回显信息，可用于界面展示 |

#### 错误

| HTTP | `error` | 含义 |
|---|---|---|
| 422 | `INVALID_REQUEST` | 请求体不是合法 JSON |
| 422 | `INVALID_CDK` | 激活码格式不正确 |
| 422 | `INVALID_DEVICE_ID` | 机器码长度不在 8-128 |
| 422 | `INVALID_EMAIL` | 邮箱格式不正确 |
| 404 | `CDK_NOT_FOUND` | 激活码不存在 |
| 422 | `APP_MISMATCH` | 激活码不属于当前应用（slug 不对） |
| 422 | `CDK_UNAVAILABLE` | 激活码已失效（撤销 / 退款 / 订单未支付） |
| 422 | `LEVEL_MISMATCH` | 指定的 level 与激活码等级不一致 |
| 404 | `LEVEL_NOT_FOUND` | 该应用未配置此授权等级 |
| 403 | `LEVEL_DISABLED` | 该授权等级已停用 |
| 403 | `EMAIL_MISMATCH` | 激活码与邮箱不匹配 |
| 403 | `LIMIT_EXCEEDED` | 激活码已达到可用次数上限（设备数已满） |

#### 行为约定

- **重复激活幂等**：同一 `code + deviceId` 重复激活返回 `200` 和新令牌，直接覆盖本地 license。
- **换设备**：新设备用同一激活码激活会占用一个新名额；名额已满返回 `LIMIT_EXCEEDED`，
  需先在其他设备上注销激活释放名额，或联系客服解绑。

### 3.3 在线验证（续验）

把本地保存的 license 回传，服务端确认激活码仍有效、该设备仍绑定，并签发**新令牌**顺带延长
离线宽限期。建议应用启动时与运行期间周期调用（如每 24 小时一次）。

#### 请求

```
POST {base}/api/v1/apps/{slug}/verify
Content-Type: application/json
```

```json
{
  "deviceId": "machine-hash-00000001",
  "license": "eyJhbGciOiJFZERTQSIsInR5cCI6IkpXVCJ9...",
  "email": "buyer@example.com"
}
```

| 字段 | 必填 | 说明 |
|---|---|---|
| `deviceId` | 是 | 与激活时相同的机器码 |
| `license` | 是 | 本地保存的 JWT（激活或上次验证返回的） |
| `email` | 是 | 购买邮箱 |

#### 成功 `200`

```json
{ "valid": true, "license": "新令牌", "expiresAt": "2026-09-15T12:00:00+08:00" }
```

**必须用返回的新 `license` 覆盖本地令牌**，否则下次验证会因令牌过期失败。

#### 错误

| HTTP | `error` | 含义 |
|---|---|---|
| 422 | `INVALID_REQUEST` / `INVALID_DEVICE_ID` / `INVALID_EMAIL` | 请求体或字段格式问题 |
| 422 | `INVALID_LICENSE` | 令牌格式不正确（不是合法 JWT） |
| 422 | `LICENSE_MISMATCH` | 令牌与当前设备或应用不匹配（`deviceId` 对不上，或 slug 不对） |
| 403 | `EMAIL_MISMATCH` | 令牌未绑定邮箱，或邮箱与令牌内邮箱不一致 |
| 404 | `LEVEL_NOT_FOUND` | 授权等级已被删除 |
| 401 | `INVALID_SIGNATURE` | 令牌签名无效（被篡改，或应用公钥已更换） |
| 401 | `CDK_REVOKED` | 激活码已失效（撤销 / 退款 / 订单未支付） |
| 401 | `DEVICE_NOT_ACTIVATED` | 该设备未绑定此激活码（已被注销激活或解绑） |

#### 行为约定

- 收到 `401 CDK_REVOKED` / `401 DEVICE_NOT_ACTIVATED`：授权已失效，
  立即禁用专业功能并引导用户处理（重新激活 / 联系客服）。
- 收到 `403 EMAIL_MISMATCH`：通常是旧版本令牌（未含邮箱），用购买邮箱重新激活一次即可。
- **等级停用不影响已激活设备**：停用授权等级仅阻止**新激活**（activate 返回 `403 LEVEL_DISABLED`）；
  已激活设备的续验不受影响。此为有意设计：停用用于暂停售卖，不应打断已购用户。
- **网络失败时**：不要立刻判定授权失效，走离线降级（第 3.4 节）。

### 3.4 离线验证（本地验签）

授权令牌是 Ed25519 签名的 JWT，客户端可离线验签：应用公钥（上架时拿到的等级公钥 PEM）
编译进客户端。

#### JWT payload（claims）

```json
{
  "iss": "soft-candy",
  "aud": "sujiya",
  "sub": "SDX4-K9TP-2M7Q-W3HZ",
  "code": "SDX4-K9TP-2M7Q-W3HZ",
  "app": "sujiya",
  "level": "pro",
  "levelLabel": "专业版",
  "deviceId": "machine-hash-00000001",
  "email": "buyer@example.com",
  "features": ["pro", "ocr"],
  "iat": 1750000000,
  "exp": 1752592000
}
```

| claim | 说明 |
|---|---|
| `code` / `app` / `level` / `levelLabel` | 激活码、应用、授权等级 |
| `deviceId` | 绑定的机器码，离线校验须与当前机器一致 |
| `email` | 购买邮箱 |
| `features` | 该等级功能特性列表，客户端据此开关功能 |
| `exp` | 到期时间（Unix 秒） |

#### 离线策略建议

- 在线验证失败（网络异常等）时，用本地公钥验签 + 检查 `exp` + 检查 `deviceId`，
  通过则按离线宽限期继续提供功能；
- 离线宽限期以最近一次成功在线验证的 `exp` 为界（令牌有效期由产品授权等级在管理端配置，默认 30 天；请以每次激活/续验响应中的 `expiresAt` 为准，不要假设固定时长）；
- **支持多等级（如 `pro` / `ultra`）时**：客户端内置「等级名 → 公钥」映射表，验签时按令牌内
  `level` 选择对应公钥；未登记的等级验签失败，按免费版处理（为接入
  ultra 等新等级预留）。

### 3.5 注销激活

解除当前设备与该激活码的绑定，**释放一个可用次数**（例如换机前先注销旧设备）。
注销后本机令牌立即失效。

#### 请求

```
POST {base}/api/v1/apps/{slug}/deactivate
Content-Type: application/json
```

```json
{
  "code": "SDX4-K9TP-2M7Q-W3HZ",
  "deviceId": "machine-hash-00000001",
  "email": "buyer@example.com"
}
```

| 字段 | 必填 | 说明 |
|---|---|---|
| `code` | 是 | 激活码 |
| `deviceId` | 是 | 当前机器码 |
| `email` | 是 | 购买邮箱 |

#### 成功 `200`

```json
{ "unbound": true }
```

**幂等**：设备已解绑时同样返回 `200`，网络重试不会报错。

#### 错误

| HTTP | `error` | 含义 |
|---|---|---|
| 422 | `INVALID_REQUEST` / `INVALID_CDK` / `INVALID_DEVICE_ID` / `INVALID_EMAIL` | 请求体或字段格式问题 |
| 404 | `CDK_NOT_FOUND` | 激活码不存在 |
| 422 | `APP_MISMATCH` | 激活码不属于当前应用 |
| 422 | `CDK_UNAVAILABLE` | 激活码已拉黑（退款/补发）或订单非已支付，黑名单不可解除 |
| 403 | `EMAIL_MISMATCH` | 激活码与邮箱不匹配 |

#### 行为约定

- 注销成功后**必须删除本地保存的 license**，并停用专业功能；
- 建议做成"注销激活"按钮，二次确认后调用，并在发送请求前提示用户
  "注销后将释放一个设备名额，需重新激活才能继续使用"。

### 3.6 推荐接入流程

```
激活（首次）
  填写 code + email → POST /activate → 保存 license/expiresAt → 启用专业功能

启动 / 周期验证
  本地有 license？
    是 → POST /verify（带 deviceId + email）
          成功 → 覆盖本地 license → 正常使用
          401 类错误 → 禁用专业功能，引导重新激活或联系客服
          网络失败 → 离线验签（公钥 + exp + deviceId），按宽限期降级
    否 → 进入激活界面

注销激活（用户主动）
  二次确认 → POST /deactivate → 删除本地 license → 停用专业功能
```

### 3.7 关键提醒

1. **购买邮箱必须能正确输入**：激活 / 验证 / 注销都校验邮箱，输错会得到 `EMAIL_MISMATCH`。
2. **`deviceId` 要稳定**：同一次安装内不要变化，否则验证会 `LICENSE_MISMATCH`。
3. **及时覆盖新令牌**：`/verify` 返回的新 `license` 必须保存，避免令牌过期。
4. **激活码不转赠**：激活码与购买邮箱绑定，无法转给他人使用。

## 4. 埋点上报

每个应用按自己的协议上报，服务端按 `slug` 分发给该应用注册的解析组件。

### 4.1 请求

```
POST {base}/api/v1/apps/{slug}/analytics
Content-Type: application/json
Authorization: Bearer <analyticsToken>   # 产品配置了 token 后必填
```

请求体为**任意 JSON 对象（非数组）**，单次不超过 **1MB**。以下为已实测的 z-viewer
（看图王）会话统计负载（客户端版本 0.5.0），字段结构仅供参考，各应用自定义：

```json
{
  "deviceId": "b8bb5843-a257-4519-8860-2e4dfe0801fe",
  "licenseStatus": "pro",
  "version": "0.5.0",
  "os": "Windows 11 Pro (build 26100)",
  "arch": "x86_64",
  "sessionStart": 1750000000,
  "sessionEnd": 1750003600,
  "imagesViewed": 128,
  "uniqueImages": 42,
  "formats": { "jpg": 90, "png": 30, "gif": 5, "cr2": 3 },
  "foldersViewed": 2,
  "cacheLevel": 2
}
```

### 4.2 鉴权

产品在管理后台「产品上架 → 埋点 Token」生成 32 字节随机 token（重新生成后旧 token 立即失效；
管理后台对重新生成设有二次确认，但 token 仍可被随时重置，请保持本节配置可更新）：

- **配置了 token** 的产品：上报必须携带 `Authorization: Bearer <token>`，
  缺失或错误返回 `401 UNAUTHORIZED`；
- **未配置 token** 的产品：不校验（兼容已接入的客户端），软糖铺可随时开启，
  客户端应提前把 token 做成可配置项。

### 4.3 成功 `200`

```json
{ "ok": true }
```

### 4.4 错误

| HTTP | `error` | 含义 |
|---|---|---|
| 404 | `APP_NOT_FOUND` | slug 不存在或已下架 |
| 401 | `UNAUTHORIZED` | 产品已配置 token 但缺失/不正确 |
| 429 | `RATE_LIMITED` | 单产品单 IP 每分钟超过 60 次上报 |
| 400 | `INVALID_ANALYTICS` | 请求体不是 JSON 对象或读取失败 |
| 413 | `PAYLOAD_TOO_LARGE` | 单次埋点超过 1MB |
| 502 | `ANALYTICS_PROCESSING_FAILED` | 解析组件处理失败 |

### 4.5 客户端行为约定

- 埋点失败**不影响应用功能**：静默吞掉错误或仅记本地日志，不弹窗、不重试风暴。
- 建议在会话结束时一次性上报，避免高频小请求（服务端限流 60 次/分钟/产品/IP）。
- 公共会话协议（4.1 示例中的公共字段）**无需事先约定**，直接上报即可，服务端统一解析
  公共字段落库；负载中的**应用自有字段服务端原样保留**（不解析、不校验、不影响上报成功），
  后续可按需分析。仅当负载结构与公共会话协议差异较大（如事件流而非会话统计）时，
  才需要先与软糖铺约定专用解析方式。

## 5. 检查更新

### 5.1 请求

```
GET {base}/api/v1/apps/{slug}/latest?platform=windows&channel=stable
```

| 参数 | 默认值 | 说明 |
|---|---|---|
| `platform` | `windows` | 平台标识，如 `windows` |
| `channel` | `stable` | 发布通道，如 `stable` / `beta` |

版本取该 slug + 平台 + 通道下**号最大者**（约定纯数字分段版本号，如 `1.2.10 > 1.9.0`）。

### 5.2 成功 `200`

```json
{
  "appSlug": "sujiya",
  "version": "1.2.10",
  "platform": "windows",
  "channel": "stable",
  "sourceType": "oss_cdn",
  "downloadUrl": "https://cdn.example.com/sujiya/1.2.10/setup.exe",
  "notes": "修复若干问题",
  "forceUpdate": false,
  "publishedAt": "2026-08-11T12:00:00+08:00"
}
```

| 字段 | 说明 |
|---|---|
| `version` | 最新已发布版本号 |
| `sourceType` | 下载源策略：`github` 或 `oss_cdn`，**客户端不需要理解**，只消费 `downloadUrl` |
| `downloadUrl` | 安装包下载地址，跳转系统浏览器下载 |
| `notes` | 版本说明，更新弹窗可直接展示 |
| `forceUpdate` | 强制更新标记；为 `true` 时建议阻断使用直至升级 |

### 5.3 错误

| HTTP | `error` | 含义 |
|---|---|---|
| 404 | `VERSION_NOT_FOUND` | 该平台 + 通道下暂无已发布版本 |

### 5.4 客户端行为约定

- **启动时检查**：应用启动时带当前平台与通道查询一次；与本地版本比较
  （数字分段比较，勿用字符串比较）后决定是否提示。
- **提示而非静默**：有新版本时弹窗展示 `version` + `notes`，用户确认后用系统浏览器打开
  `downloadUrl` 手动下载安装；**不做自动/静默更新**。
- **失败静默**：接口失败（网络异常 / `VERSION_NOT_FOUND`）不应阻断应用启动，静默跳过即可。

## 6. 参考实现：看图王（Tauri 2，已实测；产品标识 z-viewer）

首个按本文档完成接入的桌面客户端（Tauri 2 + Rust），接入方式对 wails / 其他框架同样适用，
可作样板：

- **服务端配置**：产品 `z-viewer`、授权等级 `pro`（专业版）。
  等级 Ed25519 公钥以 SPKI DER PEM 形式登记在软糖铺；客户端内置同一密钥的
  raw 32 字节 base64——两种编码同密钥，客户端自动解包。
- **客户端配置**（Tauri 项目 `tauri.conf.json → plugins.store`，编译期固化随二进制分发）：
  `apiBase` / `product` / `licenseLevel` / `licensePublicKeys`（等级名 → 公钥，
  raw/DER base64 或 PEM）/ `licenseFileName` / `activatePath` / `verifyPath` /
  `deactivatePath` / `analyticsPath`。字段缺失即视为未配置：`apiBase` 空则不联网验证，
  等级公钥缺失则验签必失败。
- **授权模块行为**：
  - 本地许可证文件 `license.json`（`code + license(JWT) + email`），启动加载并**离线验签**
    （Ed25519 + `exp` + `deviceId` + 产品/等级/功能位），验签失败 = 免费版；
  - 激活 / 续验 / 注销 HTTP 客户端统一 **10s 超时**，避免对话框永久卡死；
  - 启动时异步在线续验，网络失败不影响已付费用户（离线宽限期策略见第 3.4 节）；
  - 免费版功能门控由授权状态控制；debug 构建恒为已解锁，开发调试不被门控干扰，
    release 才真正生效。
- **埋点上报**：正常退出时一次性上报会话统计（负载见第 4.1 节示例），
  token 写入配置随客户端分发。

## 7. 接入检查清单

对外发布前逐项确认：

- [ ] 接口路径中的 `{slug}` 与软糖铺分配的产品标识一致
- [ ] 生产 Base URL 已替换（本地联调地址不能带出去）
- [ ] 授权等级公钥已编译进客户端，离线验签通过
- [ ] （支持多等级时）激活请求不写死 `level`，离线验签按令牌 `level` 选择公钥
- [ ] 激活 / 验证 / 注销使用稳定 `deviceId` 与购买邮箱，错误码分支已处理
- [ ] `/verify` 返回的新 license 会覆盖本地保存
- [ ] 埋点在会话结束时上报、失败静默、token 可配置
- [ ] 启动检查更新失败静默、版本号按数字分段比较
- [ ] 各接口请求设置超时（参考实现为 10s），不阻塞主流程
