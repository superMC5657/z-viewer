#!/usr/bin/env node
/**
 * 一键同步项目版本号到四个文件:
 *   package.json · src-tauri/Cargo.toml · src-tauri/Cargo.lock · src-tauri/tauri.conf.json
 *
 * 用法(项目名通过 --name 传入;package.json 的 bump:version 已内置):
 *   node scripts/bump-version.mjs --name <appName> 1.2.3            # 直接设置版本
 *   node scripts/bump-version.mjs --name <appName> --patch          # 0.1.0 -> 0.1.1
 *   node scripts/bump-version.mjs --name <appName> 1.2.3 --dry-run  # 只预览,不写文件
 *
 * 说明:
 *   - 以 package.json 的 version 为基准读取当前版本;写前会校验各文件是否一致,
 *     不一致时告警并一并修正。
 *   - 采用文本级替换,保持各文件原有格式与注释不变(不经过 JSON/Toml 序列化)。
 *   - Cargo.toml / Cargo.lock 只改 --name 指定的 package 段,
 *     不会误伤其它依赖的 version 行。
 *   - 项目名由 --name 传入,脚本本身与项目解耦,可跨项目直接迁移。
 */
import { readFileSync, writeFileSync } from 'node:fs'
import { dirname, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'

const ROOT = resolve(dirname(fileURLToPath(import.meta.url)), '..')

const SEMVER = /^\d+\.\d+\.\d+(-[0-9A-Za-z.-]+)?$/

// JSON 顶层 "version": "x.y.z"(取文件中第一个匹配,即根节点 version)
const jsonGet = (s) => s.match(/^(\s*"version":\s*")([^"]+)(")/m)?.[2]
const jsonSet = (s, v) => s.replace(/^(\s*"version":\s*")[^"]+(")/m, `$1${v}$2`)

// Cargo 系:package 段 name 行紧邻 version 行(name 由 --name 注入)
let appName = ''
const cargoPattern = () => new RegExp(`name = "${appName}"\nversion = "([^"]+)"`)
const cargoGet = (s) => s.match(cargoPattern())?.[1]
const cargoSet = (s, v) =>
    s.replace(cargoPattern(), `name = "${appName}"\nversion = "${v}"`)

const TARGETS = [
  { file: 'package.json', label: 'package.json', get: jsonGet, set: jsonSet },
  { file: 'src-tauri/Cargo.toml', label: 'Cargo.toml', get: cargoGet, set: cargoSet },
  { file: 'src-tauri/Cargo.lock', label: 'Cargo.lock', get: cargoGet, set: cargoSet },
  { file: 'src-tauri/tauri.conf.json', label: 'tauri.conf.json', get: jsonGet, set: jsonSet },
]

function usage() {
  return [
    '用法:',
    '  node scripts/bump-version.mjs --name <appName> <x.y.z>            直接设置版本',
    '  node scripts/bump-version.mjs --name <appName> --version <x.y.z>  同上(显式写法)',
    '  node scripts/bump-version.mjs --name <appName> --patch|--minor|--major  基于当前版本递增',
    '  node scripts/bump-version.mjs <...> --dry-run    只预览,不写文件',
    '',
    '--name: Cargo.toml / Cargo.lock 中 package 段的 name(即项目名);',
    '        package.json 的 bump:version 已内置,跨项目迁移时只需改那一处。',
  ].join('\n')
}

function parseArgs(argv) {
  const args = { name: null, version: null, bump: null, dryRun: false, help: false }
  for (let i = 0; i < argv.length; i++) {
    const a = argv[i]
    if (a === '--name') args.name = argv[++i]
    else if (a === '--version') args.version = argv[++i]
    else if (a === '--patch' || a === '--minor' || a === '--major') args.bump = a.slice(2)
    else if (a === '--dry-run') args.dryRun = true
    else if (a === '--help' || a === '-h') args.help = true
    else if (a.startsWith('-')) {
      console.error(`未知参数: ${a}\n\n${usage()}`)
      process.exit(1)
    } else args.version = a
  }
  return args
}

function loadTargets() {
  return TARGETS.map((t) => {
    const abs = resolve(ROOT, t.file)
    const text = readFileSync(abs, 'utf8')
    const current = t.get(text)
    if (!current) {
      console.error(
        `错误: 无法在 ${t.file} 中定位版本号(文件格式可能已变化,或 --name "${appName}" 不是其中的 package 名)`,
      )
      process.exit(1)
    }
    return { ...t, abs, text, current }
  })
}

function bump(current, type) {
  const m = current.match(/^(\d+)\.(\d+)\.(\d+)/)
  if (!m) {
    console.error(`错误: 无法解析当前版本 "${current}",无法自动递增`)
    process.exit(1)
  }
  let [, major, minor, patch] = m.map(Number)
  if (type === 'major') {
    major += 1
    minor = 0
    patch = 0
  } else if (type === 'minor') {
    minor += 1
    patch = 0
  } else patch += 1
  return `${major}.${minor}.${patch}`
}

const args = parseArgs(process.argv.slice(2))
if (args.help) {
  console.log(usage())
  process.exit(0)
}

appName = args.name?.trim() ?? ''
if (!appName) {
  console.error(`错误: 缺少 --name <appName>(Cargo 系文件 package 段的名称)。\n\n${usage()}`)
  process.exit(1)
}

const targets = loadTargets()
const current = targets[0].current

let next
if (args.version) {
  if (!SEMVER.test(args.version)) {
    console.error(`错误: 非法版本号 "${args.version}",需符合 semver(如 1.2.3 或 1.2.3-rc.1)`)
    process.exit(1)
  }
  next = args.version
} else if (args.bump) {
  next = bump(current, args.bump)
} else {
  console.error(`错误: 未指定版本。\n\n${usage()}`)
  process.exit(1)
}

// 写前校验:各文件当前版本是否与 package.json 一致
const outOfSync = targets.filter((t) => t.current !== current)
if (outOfSync.length) {
  console.warn(`警告: 以下文件当前版本与 package.json(${current})不一致,将一并修正为 ${next}:`)
  for (const t of outOfSync) console.warn(`  - ${t.label}: ${t.current}`)
}

console.log(`版本 ${current} -> ${next}${args.dryRun ? '  (dry-run,不写文件)' : ''}`)

for (const t of targets) {
  const updated = t.set(t.text, next)
  if (updated === t.text) {
    console.log(`  = ${t.label.padEnd(16)}已是 ${next},跳过`)
    continue
  }
  console.log(`  ✎ ${t.label.padEnd(16)}${t.current} -> ${next}`)
  if (!args.dryRun) writeFileSync(t.abs, updated)
}

// 写后复核
if (!args.dryRun) {
  const failed = targets.filter((t) => {
    const text = readFileSync(t.abs, 'utf8')
    return t.get(text) !== next
  })
  if (failed.length) {
    console.error(`错误: 复核未通过 →${failed.map((t) => ` ${t.label}`).join(',')}`)
    process.exit(1)
  }
  console.log(`✔ 四个文件已同步为 ${next}`)
}
