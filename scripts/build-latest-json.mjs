#!/usr/bin/env node
/**
 * 生成 Tauri updater 版本清单(双区:海外直链 latest.json + 镜像加速 latest-cn.json)。
 *
 * 用法示例(连续调用两次,仅 --proxy-prefix 不同):
 *   node scripts/build-latest-json.mjs --name <app> --repo <owner/repo> --tag <vX.Y.Z> \
 *     --assets '[{"target":"windows-x86_64","file":"<setup>.exe"}]' --dir artifacts \
 *     --proxy-prefix "" --extended --output artifacts/latest.json
 *   node scripts/build-latest-json.mjs --name <app> --repo <owner/repo> --tag <vX.Y.Z> \
 *     --assets '[{"target":"windows-x86_64","file":"<setup>.exe"}]' --dir artifacts \
 *     --extended --output artifacts/latest-cn.json
 *
 * 说明: --extended 为各平台附加 name/size/sha256(缺文件报错)。
 * 平台 key: windows-x86_64 / darwin-aarch64 / darwin-x86_64 / linux-x86_64。
 */
import { readFileSync, statSync, writeFileSync } from 'node:fs'
import { createHash } from 'node:crypto'
import { resolve } from 'node:path'

const BOOLEAN_FLAGS = new Set(['extended'])

function parseArgs(argv) {
  const args = {}
  for (let i = 0; i < argv.length; i++) {
    const a = argv[i]
    if (a.startsWith('--')) {
      const name = a.slice(2)
      if (BOOLEAN_FLAGS.has(name)) {
        args[name] = 'true'
        continue
      }
      if (i + 1 >= argv.length) {
        console.error(`错误: 参数 ${a} 缺少取值`)
        process.exit(1)
      }
      args[name] = argv[++i]
    }
  }
  return args
}

const {
  name,
  repo,
  tag,
  assets: assetsJson,
  dir = '.',
  output = 'latest.json',
  'proxy-prefix': proxyPrefix,
  extended,
} = parseArgs(process.argv.slice(2))

const isExtended = extended === 'true'

if (!name || !repo || !tag || !assetsJson) {
  console.error(
    '用法: node scripts/build-latest-json.mjs --name <appName> --repo <owner/repo> --tag <vX.Y.Z> --assets <json> [--dir artifacts] [--output latest.json] [--proxy-prefix <url>] [--extended]',
  )
  process.exit(1)
}
if (repo.includes('<') || repo.includes('>')) {
  console.error(
    `错误: --repo 仍是占位符 "${repo}",请替换为真实产物仓库(如 myname/my-app-release)后再发布`,
  )
  process.exit(1)
}

// url 加速前缀:默认 gh-proxy.com,--proxy-prefix 可覆盖(传空字符串 = 不加前缀)
const urlPrefix = proxyPrefix ?? 'https://gh-proxy.com/'

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

  const url = `${urlPrefix}https://github.com/${repo}/releases/download/${tag}/${encodeURIComponent(file)}`

  if (isExtended) {
    const filePath = resolve(dir, target, file)
    let size
    let sha256
    try {
      const fileStat = statSync(filePath)
      if (!fileStat.isFile()) {
        throw new Error('Not a file')
      }
      size = fileStat.size
      const buffer = readFileSync(filePath)
      sha256 = createHash('sha256').update(buffer).digest('hex')
    } catch {
      console.error(`错误: 找不到产物文件 ${filePath}`)
      process.exit(1)
    }

    platforms[target] = {
      signature,
      url,
      name: file,
      size,
      sha256,
    }
  } else {
    platforms[target] = {
      signature,
      url,
    }
  }
}

if (Object.keys(platforms).length === 0) {
  console.error('错误: 没有任何可用平台产物')
  process.exit(1)
}

const latest = {
  version: tag.replace(/^v/, ''),
  notes: `${name} ${tag}`,
  pub_date: new Date().toISOString(),
  platforms,
}

writeFileSync(resolve(output), JSON.stringify(latest, null, 2))
console.log(
  `已生成 ${output}:${Object.keys(platforms)
    .map((k) => `\n  ${k} -> ${platforms[k].url}`)
    .join('')}`,
)
