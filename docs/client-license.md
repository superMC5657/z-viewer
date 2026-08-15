# 客户端授权接口文档（激活 / 验证 / 注销激活）

> 面向各 wails 桌面应用的接入文档：只描述客户端需要调用的接口与行为。
> 服务端实现细节（数据库、密钥管理）不在本文档范围。

## 0. 通用约定

- **Base URL**：生产环境域名确定后替换；本地联调为 `http://localhost:8080`
- **路径前缀**：`/api/v1`
- **请求/响应**：均为 JSON，UTF-8
- **错误响应**统一结构：

```json
{ "error": "ERROR_CODE", "message": "给用户看的中文说明" }
```

- **双凭证**：激活 / 验证 / 注销激活都需要**购买邮箱**。购买邮箱是下单时填写的邮箱，与服务端订单记录做大小写不敏感匹配；服务端不会直接告诉你"激活码对应的邮箱是什么"，忘记邮箱时按现有客服找回流程处理（凭邮箱 + 订单号）。
- **激活码格式**：`XXXX-XXXX-XXXX-XXXX`，大写字母（避开 `0/O/1/I/L`）与数字。

## 1. 激活

把激活码绑定到当前设备，获取授权令牌（JWT）。

### 请求

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
| `deviceId` | 是 | 机器码，8-128 字符，建议用稳定的机器指纹哈希；**换机器码=换设备** |
| `email` | 是 | 购买邮箱，大小写不敏感 |
| `level` | 否 | 授权等级名，如 `pro`；省略时使用激活码所属等级 |

### 成功 `200`

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
| `license` | Ed25519 签名的 JWT，**客户端必须本地保存**，后续验证与离线宽限期都靠它 |
| `expiresAt` | 令牌到期时间（RFC3339）；到期前应在线的验证续期 |
| `appSlug` / `level` / `levelLabel` / `deviceId` | 回显信息，可用于界面展示 |

### 错误

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
| 403 | `EMAIL_MISMATCH` | **激活码与邮箱不匹配**（邮箱不是该激活码的购买邮箱） |
| 403 | `LIMIT_EXCEEDED` | 该激活码已达到可用次数上限（设备数已满） |

### 客户端行为约定

- **重复激活幂等**：同一 `code + deviceId` 重复激活会返回 `200` 和新令牌，可直接覆盖本地 license。
- **换设备**：在新设备上用同一激活码激活，会占用一个新的设备名额；若名额已满返回 `LIMIT_EXCEEDED`，需先在其他设备上「注销激活」释放名额，或联系客服解绑。
- **可用次数**：一个激活码可绑定的设备数由应用配置决定（购买页可见）。

## 2. 在线验证（续验）

客户端把本地保存的 license 回传，服务端确认激活码仍有效、该设备仍绑定，并签发**新令牌**顺带延长离线宽限期。建议应用启动时与运行期间周期调用（如每 24 小时一次）。

### 请求

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

### 成功 `200`

```json
{ "valid": true, "license": "新令牌", "expiresAt": "2026-09-15T12:00:00+08:00" }
```

**必须用返回的新 `license` 覆盖本地令牌**，否则下次验证会因令牌过期失败。

### 错误

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

### 客户端行为约定

- 收到 `401 CDK_REVOKED` / `401 DEVICE_NOT_ACTIVATED`：授权已失效，应立即禁用专业功能并引导用户处理（重新激活 / 联系客服）。
- 收到 `403 EMAIL_MISMATCH`：通常是旧版本令牌（未含邮箱），用购买邮箱重新激活一次即可。
- **网络失败时**：不要立刻判定授权失效，可离线降级（见第 3 节）。

## 3. 离线验证（可选，本地验签）

授权令牌是 **Ed25519 签名的 JWT**，客户端可离线验签：**应用公钥编译进客户端**（管理后台生成/录入的等级公钥 PEM）。

### JWT payload（claims）

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

### 离线策略建议

- 在线验证失败（网络异常等）时，用本地公钥验签 + 检查 `exp` + 检查 `deviceId`，通过则按离线宽限期继续提供功能；
- 离线宽限期建议以最近一次成功在线验证的 `exp` 为界（服务端每次续验签发 30 天有效期令牌）。

## 4. 注销激活

解除当前设备与该激活码的绑定，**释放一个可用次数**（例如换机前在新设备激活前先注销旧设备）。注销后本机令牌立即失效。

### 请求

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

### 成功 `200`

```json
{ "unbound": true }
```

**幂等**：设备已解绑时同样返回 `200`，网络重试不会报错。

### 错误

| HTTP | `error` | 含义 |
|---|---|---|
| 422 | `INVALID_REQUEST` / `INVALID_CDK` / `INVALID_DEVICE_ID` / `INVALID_EMAIL` | 请求体或字段格式问题 |
| 404 | `CDK_NOT_FOUND` | 激活码不存在 |
| 422 | `APP_MISMATCH` | 激活码不属于当前应用 |
| 403 | `EMAIL_MISMATCH` | 激活码与邮箱不匹配 |

### 客户端行为约定

- 注销成功后**必须删除本地保存的 license**，并停用专业功能；
- 可做成"注销激活"按钮，二次确认后调用；建议在发送请求前提示用户"注销后将释放一个设备名额，需重新激活才能继续使用"。

## 5. 推荐接入流程

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

## 6. 关键提醒

1. **购买邮箱必须能正确输入**：激活 / 验证 / 注销都校验邮箱，输错会得到 `EMAIL_MISMATCH`。
2. **`deviceId` 要稳定**：同一次安装内不要变化，否则验证会 `LICENSE_MISMATCH`。
3. **及时覆盖新令牌**：`/verify` 返回的新 `license` 必须保存，避免令牌过期。
4. **激活码不转赠**：激活码与购买邮箱绑定，无法转给他人使用。
