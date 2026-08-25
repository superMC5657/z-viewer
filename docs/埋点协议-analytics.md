# Image Viewer 埋点协议（会话统计）

> 版本：v0.5.0+
> 状态：客户端已落地（2026-08-11），支持 soft-candy 埋点鉴权 token（2026-08-25 起）
> 相关代码：`src-tauri/src/analytics.rs`、`src-tauri/src/main.rs`（退出上报）、
> `src-tauri/src/commands/mod.rs`（`record_view` 计数命令）、`src/main.ts`（显示成功计数）、
> `backend-mock/server.mjs`（接收端点）

## 1. 概述

应用在**正常退出**时上报一次会话统计：用户本次运行看了多少张图、用时多久、
用了什么功能配置等。仅用于产品分析（DAU、读图习惯、格式分布、付费转化），
**不上报任何文件路径、文件名、文件夹名**。

## 2. 上报时机与边界

| 场景 | 是否上报 |
|---|---|
| 点窗口关闭按钮 / Alt+F4 / 菜单退出 | ✅ 上报（`RunEvent::ExitRequested` 回调，同步发送，3 秒超时） |
| `kill` / 任务管理器结束进程 | ❌ **无法上报**（进程被直接终止，无任何代码执行机会） |
| 崩溃 / 断电 | ❌ 无法上报 |

> **kill 场景说明**：这是所有桌面应用的固有限制。若未来需要覆盖 kill 场景，
> 需改为「会话中定期增量上报 + 退出时全量上报」策略（当前已明确不做）。

## 3. 上报地址

```
POST {apiBase}{analyticsPath}
```

- `apiBase` / `analyticsPath` 均来自 `tauri.conf.json → plugins.store`（编译期固化）：
  ```json
  "plugins": {
    "store": {
      "apiBase": "https://your-server.example.com",
      "analyticsPath": "/api/analytics"
    }
  }
  ```
- `apiBase` 与授权体系（激活/在线验证）共用同一地址，域名一致，便于服务端统一鉴权。
- `analyticsPath` 为空或 `apiBase` 为空 = 不启用埋点（老配置无此字段时自动关闭，安全）。
- **埋点鉴权**：`analyticsToken` 非空时上报携带 `Authorization: Bearer <token>`，
  为空则不携带（服务端对该产品不校验，兼容未配置 token 的部署）。
  ```json
  "analyticsPath": "/api/v1/apps/image-viewer/analytics",
  "analyticsToken": "<soft-candy 管理后台产品「埋点 Token」生成，空 = 不携带>"
  ```
  token 由服务端 `tracking-design.md` §6.1 定义：32 字节随机 hex，生成后旧 token 立即失效；
  配置了 token 的产品缺失/错误 token 上报返回 `401 UNAUTHORIZED`。

## 4. 上报字段

负载为 JSON，全部字段：

| 字段 | 类型 | 说明 |
|---|---|---|
| `deviceId` | string | 设备指纹，**与授权体系完全一致**（同一台机器两个系统里是同一个 id，便于 DAU 去重与付费转化分析） |
| `licenseStatus` | string | `"pro"` / `"free"`（debug 构建恒 `"pro"`） |
| `version` | string | 应用版本号（`CARGO_PKG_VERSION`，如 `0.3.2`） |
| `os` | string | 操作系统描述，如 `Windows 11 Pro (build 26100)`（Windows 读注册表 ProductName + CurrentBuild） |
| `arch` | string | CPU 架构，如 `x86_64` |
| `sessionStart` | int | 应用启动时刻（Unix 秒） |
| `sessionEnd` | int | 退出时刻（Unix 秒） |
| `imagesViewed` | int | 读图总次数（含重复浏览；缓存命中同样计数） |
| `uniqueImages` | int | 去重后的图片数（会话内不同的图片张数） |
| `formats` | object | 图片格式分布：`{ "jpg": 90, "png": 30, "cr2": 3 }`（扩展名小写） |
| `foldersViewed` | int | 本次会话浏览过的文件夹数 |
| `cacheLevel` | int | 缓存等级 `0`=关闭 `1`=开启 `2`=高（免费版恒 `0`） |

### 负载示例

```json
{
  "deviceId": "b8bb5843-a257-4519-8860-2e4dfe0801fe",
  "licenseStatus": "pro",
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

## 5. 隐私边界

- ✅ 上报的只有**聚合统计**（数量、格式分布、时长、版本、系统类型）。
- ❌ **绝不上报**：文件路径、文件名、文件夹名、图片内容、缩略图、任何浏览序列。
- 路径仅存在于进程内存中用于去重计数（`HashSet`），序列化时只输出 `len()`。
- 隐私保障有单元测试兜底（`analytics::tests::snapshot_shape_and_privacy`：
  断言序列化结果中不含路径字符串）。
- `deviceId` 是授权体系既有的设备标识（Windows MachineGuid），非埋点新增。

## 6. 服务端接入

### 参考实现（backend-mock）

`backend-mock/server.mjs` 已实现接收端点，仅打印观察，不落盘：

```bash
node server.mjs          # 启动 http://127.0.0.1:8787
# 客户端退出后，控制台打印收到的负载
```

### 生产端（Go 售卖网站 / soft-candy）

soft-candy 已实现接收与鉴权（`POST /api/v1/apps/image-viewer/analytics`，
见其 `docs/tracking-design.md`）：

1. 接收 JSON（字段见第 4 节），canonical event 落库 `tracking_events`（payload JSONB 保留原始负载）。
2. 鉴权：管理后台产品配置「埋点 Token」后，缺失/错误 token 返回 `401 UNAUTHORIZED`；
   token 为空不校验（兼容已接入客户端）。客户端在 `analyticsToken` 非空时自动携带 Bearer 头。
3. 限流：单产品单 IP 每分钟 60 次，超出 `429 RATE_LIMITED`。
4. 响应 `{ "ok": true }` 即可；客户端不校验响应，失败静默。

## 7. 客户端计数语义（供后续维护参考）

- 计数命令是 `record_view`（`src-tauri/src/commands/mod.rs`）：前端在**图片显示成功**
  后调用一次，`load_image` 命令本身不计数。
- 前端计数点唯一（`src/main.ts` 的 `showImage`）：
  - **asset 快速通道**（jpg/bmp 等浏览器原生解码，直接 `convertFileSrc`，不经 `load_image`）
    显示成功后调用；
  - **IPC 通道**（RAW/动画/可能动画格式）`load_image` 返回并渲染成功后调用。
  - 两条通道各计一次，保证所有格式都覆盖且不重复计数。
- 解码失败 / 显示被更新抢占（`seq` 校验不通过）**不计数**——用户没真正看到这张图。
- 预取（`prefetch.rs` / 前端 `refreshContext`）只预热缓存，不触发 `record_view`——
  预取不代表用户看到。
- 导航命令（next/prev/jump_folder）只返回状态，不计数，由前端显示成功时
  的 `record_view` 计。
- 去重规范化：`uniqueImages` / `foldersViewed` 的键在计数时统一转换
  （`analytics.rs::normalize_key`）——分隔符统一为 `/`，Windows 上路径大小写不敏感
  统一转小写，保证同一文件/文件夹无论以 `C:\Photos\a.JPG` 还是 `c:/photos/a.jpg`
  形态出现都只计一次。
- 根目录文件（无 parent 文件夹）不产生 `foldersViewed` 计数。

## 8. 本地联调验证

```bash
# 1. 启动 mock 后端
cd backend-mock && node server.mjs

# 2. 启动应用（dev 模式，打开测试目录）
pnpm tauri dev -- "Z:\...\test-images\A"

# 3. 正常关闭窗口（点 X 或 Alt+F4），观察 mock 控制台打印的负载
# 验证点：imagesViewed > 0、sessionStart/End 合理、无路径字段
```
