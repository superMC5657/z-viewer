#!/usr/bin/env node
/**
 * 管理 CLI：生成激活码 / 吊销
 *
 * 用法：
 *   node gen-codes.mjs --count 5 --note "v0.3 发售"
 *   node gen-codes.mjs --revoke IV-XXXX-XXXX-XXXX
 *
 * 环境变量：
 *   IV_ADMIN_KEY    管理密钥（默认 dev-admin-key，与 server.mjs 一致）
 *   IV_API          服务地址（默认 http://127.0.0.1:8787）
 */
const BASE = process.env.IV_API ?? "http://127.0.0.1:8787";
const ADMIN_KEY = process.env.IV_ADMIN_KEY ?? "dev-admin-key";

function parseArgs() {
  const args = process.argv.slice(2);
  const o = { count: 1, note: "", revoke: null };
  for (let i = 0; i < args.length; i++) {
    if (args[i] === "--count") o.count = Number(args[++i]) || 1;
    else if (args[i] === "--note") o.note = args[++i] ?? "";
    else if (args[i] === "--revoke") o.revoke = args[++i] ?? null;
    else if (args[i] === "--help" || args[i] === "-h") {
      console.log("用法: node gen-codes.mjs [--count N] [--note 备注] [--revoke 激活码]");
      process.exit(0);
    }
  }
  return o;
}

async function main() {
  const o = parseArgs();
  if (o.revoke) {
    const resp = await fetch(`${BASE}/api/admin/revoke`, {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({ apiKey: ADMIN_KEY, code: o.revoke }),
    });
    const r = await resp.json();
    if (!resp.ok) {
      console.error(`吊销失败: ${r.error ?? resp.status}`);
      process.exit(1);
    }
    console.log(`已吊销: ${o.revoke}`);
    return;
  }

  const resp = await fetch(`${BASE}/api/admin/gen`, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ apiKey: ADMIN_KEY, count: o.count, note: o.note }),
  });
  const r = await resp.json();
  if (!resp.ok) {
    console.error(`生成失败: ${r.error ?? resp.status}（服务是否已启动？node server.mjs）`);
    process.exit(1);
  }
  console.log(`生成 ${r.codes.length} 个激活码${o.note ? `（备注: ${o.note}）` : ""}:`);
  for (const c of r.codes) console.log(`  ${c}`);
}

main().catch((e) => {
  console.error(e.message);
  process.exit(1);
});
