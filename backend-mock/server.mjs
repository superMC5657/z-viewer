#!/usr/bin/env node
/**
 * Image Viewer 授权服务（参考实现，零 npm 依赖）
 *
 * 用法：node server.mjs [--port 8787]
 * 环境变量：
 *   IV_ADMIN_KEY    管理 API 密钥（默认 dev-admin-key，生产必须修改）
 *   IV_DATA_FILE    数据文件路径（默认 ./data.json）
 *   IV_PRIVATE_KEY  签发私钥 PEM 路径（默认 ./keys/ed25519-private.pem）
 *
 * API：
 *   POST /api/activate   { code, device_id }            → { license } | { error }
 *                        （每码最多 MAX_DEVICES 台同时在线；满员时自动踢掉最旧设备，FIFO）
 *   POST /api/verify     { device_id, license }         → { valid, reason? }
 *   POST /api/analytics  <会话统计负载>                  → { ok }（仅打印，不落盘）
 *   GET  /api/health                                    → { ok }
 *   POST /api/admin/gen     { apiKey, count?, note? }   → { codes: [...] }
 *   POST /api/admin/revoke  { apiKey, code }            → { ok }
 *
 * 许可证签发：Ed25519 签名，payload 拼接规则与客户端 license.rs 一致
 *   `iv:{device_id}:{features.join(",")}:{issued_at}:{expires_at}`
 *
 * 生产部署提示：此实现用 JSON 文件持久化（单机足够）；
 * 迁移 Cloudflare Workers / SQLite 时保持 API 与 payload 协议不变即可。
 */
import { createServer } from "node:http";
import { readFileSync, writeFileSync, existsSync, mkdirSync } from "node:fs";
import { join, dirname } from "node:path";
import { fileURLToPath } from "node:url";
import crypto from "node:crypto";

const __dir = dirname(fileURLToPath(import.meta.url));
const PORT = Number(process.env.IV_PORT ?? 8787);
const ADMIN_KEY = process.env.IV_ADMIN_KEY ?? "dev-admin-key"; // 生产必须修改
const DATA_FILE = process.env.IV_DATA_FILE ?? join(__dir, "data.json");
const PRIVATE_KEY_PEM =
  process.env.IV_PRIVATE_KEY ?? join(__dir, "keys", "ed25519-private.pem");

/** 每码可绑设备上限 */
const MAX_DEVICES = 3;

// ---------- 存储 ----------

function loadData() {
  if (!existsSync(DATA_FILE)) return { codes: [] };
  try {
    return JSON.parse(readFileSync(DATA_FILE, "utf8"));
  } catch {
    return { codes: [] };
  }
}

let data = loadData();

function saveData() {
  mkdirSync(dirname(DATA_FILE), { recursive: true });
  writeFileSync(DATA_FILE, JSON.stringify(data, null, 2));
}

// ---------- 签名 ----------

function loadPrivateKey() {
  if (!existsSync(PRIVATE_KEY_PEM)) {
    throw new Error(
      `缺少签发私钥：${PRIVATE_KEY_PEM}\n` +
        `请先运行 node gen-keys.mjs 生成密钥对（或设置 IV_PRIVATE_KEY）。`
    );
  }
  return readFileSync(PRIVATE_KEY_PEM, "utf8");
}

const PRIVATE_KEY = loadPrivateKey();

/** 签发许可证 payload（与客户端 license.rs payload() 完全一致） */
function signLicense(deviceId, features, issuedAt, expiresAt) {
  const payload = `iv:${deviceId}:${features.join(",")}:${issuedAt}:${expiresAt}`;
  const sig = crypto.sign(null, Buffer.from(payload, "utf8"), PRIVATE_KEY);
  return {
    device_id: deviceId,
    features,
    issued_at: issuedAt,
    expires_at: expiresAt,
    sig: sig.toString("base64"),
  };
}

// ---------- 激活码 ----------

const CODE_ALPHABET = "ABCDEFGHJKMNPQRSTUVWXYZ23456789"; // 去掉易混淆 I L O 0 1
function randomCode() {
  const seg = () => {
    let s = "";
    for (let i = 0; i < 4; i++) {
      s += CODE_ALPHABET[crypto.randomInt(CODE_ALPHABET.length)];
    }
    return s;
  };
  return `IV-${seg()}-${seg()}-${seg()}`;
}

function genCodes(count, note) {
  const now = Math.floor(Date.now() / 1000);
  const codes = [];
  for (let i = 0; i < count; i++) {
    const code = randomCode();
    data.codes.push({
      code,
      note: note ?? "",
      created_at: now,
      status: "active",
      devices: [],
      max_devices: MAX_DEVICES,
    });
    codes.push(code);
  }
  saveData();
  return codes;
}

// ---------- 业务 ----------

/** 激活：校验激活码 → 绑定设备 → 签发许可证
 * 设备数策略（FIFO 滑动窗口）：同一激活码最多 `max_devices`（默认 3）台设备同时在线；
 * 满员时新设备激活 → **自动踢掉最旧的一台**，新设备顶上（而不是拒绝）。
 * 被踢设备下次启动在线验证返回 valid:false，客户端自动清许可证降级免费版。
 */
function activate({ code, device_id }) {
  if (!code || !device_id) return { error: "缺少激活码或设备标识" };
  const c = data.codes.find((x) => x.code === code.toUpperCase());
  if (!c) return { error: "激活码不存在" };
  if (c.status === "revoked") return { error: "激活码已被吊销" };
  if (c.devices.includes(device_id)) {
    // 同一设备重复激活：直接返回已签发的许可证（幂等）
    return { license: signLicense(device_id, ["pro"], c.created_at, 0) };
  }
  if (c.status === "exhausted") {
    return { error: "激活码已封禁，无法继续绑定设备" };
  }
  if (c.devices.length >= c.max_devices) {
    // 满员：踢掉最旧一台（数组头部），新设备顶上
    const kicked = c.devices.shift();
    console.log(
      `[license] ${c.code} 设备满员（${c.max_devices} 台）：踢掉 ${kicked}，绑定 ${device_id}`
    );
  }
  c.devices.push(device_id);
  saveData();
  return { license: signLicense(device_id, ["pro"], c.created_at, 0) };
}

/** 在线验证：设备绑定关系 + 吊销状态；签名由客户端验 */
function verify({ device_id, license }) {
  if (!license?.device_id || license.device_id !== device_id) {
    return { valid: false, reason: "设备标识不匹配" };
  }
  const found = data.codes.find(
    (x) => x.devices.includes(device_id) && x.status === "active"
  );
  if (!found) return { valid: false, reason: "许可证未绑定或已被吊销" };
  return { valid: true };
}

// ---------- HTTP ----------

function send(res, status, body) {
  res.writeHead(status, { "content-type": "application/json; charset=utf-8" });
  res.end(JSON.stringify(body));
}

function readBody(req) {
  return new Promise((resolve) => {
    let raw = "";
    req.on("data", (c) => (raw += c));
    req.on("end", () => {
      try {
        resolve(JSON.parse(raw || "{}"));
      } catch {
        resolve({});
      }
    });
  });
}

const server = createServer(async (req, res) => {
  const url = new URL(req.url, `http://${req.headers.host}`);
  const path = url.pathname;
  const method = req.method;

  try {
    // 管理接口（需 apiKey）
    if (path === "/api/admin/gen" && method === "POST") {
      const body = await readBody(req);
      if (body.apiKey !== ADMIN_KEY) return send(res, 401, { error: "管理密钥错误" });
      const count = Math.min(Math.max(Number(body.count) || 1, 1), 100);
      const codes = genCodes(count, body.note ?? "");
      return send(res, 200, { codes });
    }

    if (path === "/api/admin/revoke" && method === "POST") {
      const body = await readBody(req);
      if (body.apiKey !== ADMIN_KEY) return send(res, 401, { error: "管理密钥错误" });
      const c = data.codes.find((x) => x.code === (body.code ?? "").toUpperCase());
      if (!c) return send(res, 404, { error: "激活码不存在" });
      c.status = "revoked";
      saveData();
      return send(res, 200, { ok: true });
    }

    // 激活
    if (path === "/api/activate" && method === "POST") {
      const body = await readBody(req);
      const r = activate(body);
      if (r.error) return send(res, 400, r);
      return send(res, 200, r);
    }

    // 在线验证
    if (path === "/api/verify" && method === "POST") {
      const body = await readBody(req);
      return send(res, 200, verify(body));
    }

    // 会话统计埋点（客户端退出时上报；仅打印观察，生产端自行接入存储/看板）
    if (path === "/api/analytics" && method === "POST") {
      const body = await readBody(req);
      console.log(
        `[analytics] ${new Date().toISOString()} 收到会话上报:`,
        JSON.stringify(body, null, 2)
      );
      return send(res, 200, { ok: true });
    }

    // 健康检查
    if (path === "/api/health") {
      return send(res, 200, { ok: true, active_codes: data.codes.filter((x) => x.status === "active").length });
    }

    send(res, 404, { error: "not found" });
  } catch (err) {
    console.error(err);
    send(res, 500, { error: "服务器内部错误" });
  }
});

server.listen(PORT, () => {
  console.log(`Image Viewer 授权服务已启动: http://127.0.0.1:${PORT}`);
  console.log(`数据文件: ${DATA_FILE}`);
  console.log(`激活码: node gen-codes.mjs --count 5 --note "..."`);
});
