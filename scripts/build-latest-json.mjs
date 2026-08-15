#!/usr/bin/env node
/**
 * 生成 Tauri updater 用的 latest.json（多平台合并 + gh-proxy.com 加速前缀）。
 *
 * 用法:
 *   node scripts/build-latest-json.mjs \
 *     --repo <owner>/<releases-repo> \
 *     --tag <vX.Y.Z> \
 *     --assets '[{"target":"windows-x86_64","file":"Image Viewer_0.1.0_x64-setup.exe"}, ...]' \
 *     --dir artifacts \
 *     --output latest.json
 *
 * 说明:
 *   - file 为安装包文件名(basename,即 release 资产名);实际文件与同名 .sig
 *     位于 <dir>/<target>/ 下(由 `tauri signer sign` 生成)。
 *   - url 统一加 gh-proxy.com 前缀加速国内下载:
 *     https://gh-proxy.com/https://github.com/<repo>/releases/download/<tag>/<file>
 *   - 平台 key 遵循 Tauri updater 约定:windows-x86_64 / darwin-aarch64 /
 *     darwin-x86_64 / linux-x86_64。
 *   - 产物统一命名为 latest.json(不带 -cn 之类后缀)。
 */
import { readFileSync, writeFileSync } from 'node:fs'
import { resolve } from 'node:path'

function parseArgs(argv) {
  const args = {}
  for (let i = 0; i < argv.length; i++) {
    const a = argv[i]
    if (a.startsWith('--')) {
      args[a.slice(2)] = argv[++i]
    }
  }
  return args
}

const {
  repo,
  tag,
  assets: assetsJson,
  dir = '.',
  output = 'latest.json',
} = parseArgs(process.argv.slice(2))

if (!repo || !tag || !assetsJson) {
  console.error(
    '用法: node scripts/build-latest-json.mjs --repo <owner/repo> --tag <vX.Y.Z> --assets <json> [--dir artifacts] [--output latest.json]',
  )
  process.exit(1)
}
if (repo.includes('<') || repo.includes('>')) {
  console.error(
    `错误: --repo 仍是占位符 "${repo}",请替换为真实产物仓库(如 myname/image-viewer-release)后再发布`,
  )
  process.exit(1)
}

const assets = JSON.parse(assetsJson)
if (!Array.isArray(assets) || assets.length === 0) {
  console.error('错误: --assets 必须是非空数组')
  process.exit(1)
}

const platforms = {}
for (const { target, file } of assets) {
  if (!target || !file) {
    console.error(`跳过无效资产条目: ${JSON.stringify({ target, file })}`)
    continue
  }
  const sigPath = resolve(dir, target, file + '.sig')
  let signature
  try {
    signature = readFileSync(sigPath, 'utf8').trim()
  } catch {
    console.error(`错误: 找不到签名文件 ${sigPath}（需先对产物执行 tauri signer sign）`)
    process.exit(1)
  }
  platforms[target] = {
    signature,
    // 文件名可能含空格(如 "Image Viewer_0.4.1_x64-setup.exe"),URL 必须百分号编码,
    // 否则 Tauri updater 解析 URL 会失败
    url: `https://gh-proxy.com/https://github.com/${repo}/releases/download/${tag}/${encodeURIComponent(file)}`,
  }
}

if (Object.keys(platforms).length === 0) {
  console.error('错误: 没有任何可用平台产物')
  process.exit(1)
}

const latest = {
  version: tag.replace(/^v/, ''),
  notes: `Image Viewer ${tag}`,
  pub_date: new Date().toISOString(),
  platforms,
}

writeFileSync(resolve(output), JSON.stringify(latest, null, 2))
console.log(
  `已生成 ${output}:${Object.keys(platforms)
    .map((k) => `\n  ${k} -> ${platforms[k].url}`)
    .join('')}`,
)
