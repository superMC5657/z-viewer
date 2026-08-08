#!/usr/bin/env node
/**
 * 生成 Ed25519 密钥对（签发许可证用）
 *
 * ⚠️ 重要：密钥对是「信任锚」，只在以下两种情况运行本脚本：
 *   1. 首次上线（生产部署前）
 *   2. 私钥泄露（必须更换，且所有老用户需重新激活）
 * 发新版（打 tag / 更新版本号）绝对不要重新生成 —— 换了公钥，
 * 旧版本客户端将无法验证新许可证，所有已付费用户会失效。
 *
 * 步骤：
 *   1. node gen-keys.mjs            # 生成新密钥对到 ./keys/
 *   2. 私钥 backend-mock/keys/ed25519-private.pem 部署到服务器（勿入库）
 *   3. 公钥 base64 写进客户端 src-tauri/src/license.rs 的 LICENSE_PUBLIC_KEY_B64
 *   4. 重新构建客户端并发布
 *
 * 注意：仓库中现有的密钥对仅用于开发/演示，任何人 clone 都能用它签发激活码。
 */
import { generateKeyPairSync } from "node:crypto";
import { writeFileSync, mkdirSync } from "node:fs";
import { join, dirname } from "node:path";
import { fileURLToPath } from "node:url";

const __dir = dirname(fileURLToPath(import.meta.url));
const keysDir = join(__dir, "keys");

mkdirSync(keysDir, { recursive: true });

const { publicKey, privateKey } = generateKeyPairSync("ed25519");

writeFileSync(join(keysDir, "ed25519-private.pem"), privateKey.export({ type: "pkcs8", format: "pem" }));
writeFileSync(join(keysDir, "ed25519-public.pem"), publicKey.export({ type: "spki", format: "pem" }));

// raw 32 字节公钥（嵌入客户端）
const rawPub = publicKey.export({ type: "spki", format: "der" });
const raw32 = rawPub.subarray(rawPub.length - 32).toString("base64");

console.log("已生成密钥对:");
console.log(`  私钥: ${join(keysDir, "ed25519-private.pem")}`);
console.log(`  公钥: ${join(keysDir, "ed25519-public.pem")}`);
console.log(`客户端常量 LICENSE_PUBLIC_KEY_B64 = "${raw32}"`);
