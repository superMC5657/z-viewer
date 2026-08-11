# Image Viewer 授权后端（参考实现）

在线激活的参考后端，零 npm 依赖，单文件可跑。

## 快速开始（本地验证）

```bash
node gen-keys.mjs            # 首次：生成密钥对（backend-mock/keys/，已 gitignore 不入库）
                             # 生成后把输出里的公钥 base64 同步到
                             # 客户端 src-tauri/src/license.rs 的 LICENSE_PUBLIC_KEY_B64
node server.mjs              # 启动在 http://127.0.0.1:8787（客户端 dev 默认地址）
node gen-codes.mjs --count 3 --note "本地测试"   # 生成激活码
```

客户端 `pnpm tauri dev` 打开后：点缓存/文件夹按钮 → 解锁对话框 → 输入激活码即解锁。

## API

| 接口 | 说明 |
|---|---|
| `POST /api/activate` | `{ code, device_id }` → `{ license }`；激活码绑定设备（每码最多 3 台同时在线，满员时**自动踢掉最旧设备**，FIFO；重复激活幂等） |
| `POST /api/verify` | `{ device_id, license }` → `{ valid }`；客户端启动时调用，吊销生效 |
| `POST /api/analytics` | 会话统计埋点（客户端正常退出时上报，负载见下）；本参考实现仅打印，生产端自行接入存储/看板 |
| `POST /api/admin/gen` | `{ apiKey, count, note }` → `{ codes }` |
| `POST /api/admin/revoke` | `{ apiKey, code }` → `{ ok }` |
| `GET /api/health` | 健康检查 |

管理密钥：环境变量 `IV_ADMIN_KEY`（默认 `dev-admin-key`，**生产必须修改**）。

## 数据存储

`data.json`（默认 `backend/data.json`）：激活码 + 设备绑定关系。单机 JSON 足够；
生产可迁移 Cloudflare Workers（KV）/ SQLite，**保持 API 与 payload 协议不变即可**，
客户端无需改动。

## ⚠️ 生产上线前必做

1. **更换签名密钥**：`node gen-keys.mjs` 生成新密钥对。
   - 私钥 `backend/keys/ed25519-private.pem` 部署到服务器，**切勿入库**
     （`backend/keys/` 与 `backend/data.json` 已在 .gitignore，含激活码与设备绑定数据）。
   - 公钥 base64 更新到客户端 `src-tauri/src/license.rs` 的 `LICENSE_PUBLIC_KEY_B64`，
     重新构建发布。
2. **修改管理密钥**：`IV_ADMIN_KEY` 换成强随机值。
3. **改客户端服务地址**：`src-tauri/src/license.rs` 的 `LICENSE_API_BASE` 指向部署后的地址。
4. 建议 HTTPS（`/api/activate` 传输激活码，防中间人截获）。

## 🔑 密钥生命周期（重要：为什么生成一次就不动了）

**授权密钥对是「信任锚」，生成一次后长期使用，发新版绝不更换。**

```
授权密钥对（backend/keys/ed25519-*.pem）   ← 信任锚，几乎永远不变
   ├─ 私钥 → 只在服务端：激活时给许可证签名
   └─ 公钥 → 烧进客户端二进制：license.rs 里 LICENSE_PUBLIC_KEY_B64

激活码（IV-XXXX-XXXX-XXXX）                ← 有生命周期，随时生成/吊销
   └─ 只是服务端数据库里的一行随机字符串，与密钥无关

许可证（license.json）                      ← 激活时签发，用当时的私钥签名
```

- **发新版**（打 tag、更新版本号）：**不需要也不应该碰密钥**。已发激活码、已激活
  许可证全部继续有效。密钥对生成一次，此后每次发版都用同一把。
- **换密钥的后果**：换了公钥，旧版本客户端（内置旧公钥）就无法验新签发的许可证；
  且旧私钥签的许可证在新客户端下也失效 → **所有老用户必须重新激活**。
- **唯一换密钥的理由 = 私钥泄露**。泄露时换密钥 + 发新版，代价是用户全量重激活
  （旧私钥可能被滥用，旧许可证不可信，必须作废）。

> 类比：像 HTTPS 的 CA 根证书，几年不换一次；而激活码像「兑换券」，随时可发可废。

## 授权协议（客户端-服务端一致）

```
payload = iv:{device_id}:{features.join(",")}:{issued_at}:{expires_at}
license = { device_id, features:["pro"], issued_at, expires_at:0, sig: base64(ed25519(payload)) }
```

- 服务端用私钥签发；客户端内置公钥验签（`license.rs::verify_signature`）
- 防篡改：本地改动 license.json 任何字段都会验签失败
- 防分享：激活码绑设备（每码最多 3 台**同时在线**，满员时新设备激活自动踢掉最旧设备
  ——FIFO 滑动窗口；被踢设备下次启动在线验证降级为免费版）
- 离线可用：网络失败不阻止已付费用户（仅吊销/被踢场景需要在线）

## 会话统计埋点（POST /api/analytics）

客户端在**正常退出**（点关闭/Alt+F4/菜单退出）时上报；`kill`/任务管理器强杀
进程无代码执行机会，无法上报（已知限制）。上报地址 = `plugins.store.apiBase`
+ `analyticsPath`（tauri.conf.json 编译期配置）。

负载（全部为聚合统计，**不含任何文件路径/文件名**）：

```json
{
  "deviceId": "机器指纹（与授权相同）",
  "licenseStatus": "pro | free",
  "version": "0.3.2",
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

## 限制与取舍

- 设备指纹用 MachineGuid 哈希，重装系统会变化 → 后台 `revoke` 旧设备后再激活即可（3 台同时在线）
- 满员踢出是 **FIFO（最旧先踢）**，不是"距离上次使用最久先踢"——mock 不记录设备最后活跃时间；若需 LRU 策略需在 `devices` 里加时间戳（生产 Go 后端可加）
- JSON 文件存储无并发保护，多实例部署前请迁移数据库
