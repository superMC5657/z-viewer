#!/usr/bin/env bash
# ============================================================
# 一键发版（gh 本地发版，不依赖 CI）
#
# 用法:
#   pnpm release            # 用当前三处一致的版本号发版
#   pnpm release 0.2.0      # 先统一三处版本号为 0.2.0 再发版
#
# 流程: 版本号 -> 建仓(如无) -> 本地构建+签名 -> 提交推送
#       -> 生成 latest.json / latest-cn.json -> gh 创建 Release 并上传
#
# 注意: 本脚本不推送 v* tag（避免触发 CI 与本地 Release 重复发版）。
#       密钥来自项目根 .env（已 gitignore），source 后供 Tauri CLI 签名。
# ============================================================
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

REPO="superMC5657/image-viewer"

# ---------- 1. 版本号 ----------
set_version() {
  local v="$1"
  node -e "
    const fs = require('fs');
    const pj = JSON.parse(fs.readFileSync('package.json','utf8'));
    pj.version = '$v';
    fs.writeFileSync('package.json', JSON.stringify(pj, null, 2) + '\n');
    const tc = JSON.parse(fs.readFileSync('src-tauri/tauri.conf.json','utf8'));
    tc.version = '$v';
    fs.writeFileSync('src-tauri/tauri.conf.json', JSON.stringify(tc, null, 2) + '\n');
  "
  sed -i "0,/^version = /s/^version = .*/version = \"$v\"/" src-tauri/Cargo.toml
  echo "→ 三处版本号已统一为 $v"
}

if [ $# -ge 1 ]; then set_version "$1"; fi

VERSION="$(node -p "require('./package.json').version")"
TAG="v$VERSION"
CONF_VER="$(node -p "JSON.parse(require('fs').readFileSync('src-tauri/tauri.conf.json','utf8')).version")"
CARGO_VER="$(sed -n 's/^version = "\(.*\)"/\1/p' src-tauri/Cargo.toml | head -1)"

if [ "$VERSION" != "$CONF_VER" ] || [ "$VERSION" != "$CARGO_VER" ]; then
  echo "✗ 版本号不一致: package.json=$VERSION tauri.conf.json=$CONF_VER Cargo.toml=$CARGO_VER" >&2
  exit 1
fi
echo "→ 发版版本: $TAG"

# ---------- 2. 仓库不存在则创建（幂等） ----------
if gh repo view "$REPO" >/dev/null 2>&1; then
  echo "→ 仓库 $REPO 已存在"
else
  echo "→ 仓库 $REPO 不存在，创建中 ..."
  gh repo create "$REPO" --public || true
  # 创建失败但名字已存在（并发/已建）也算通过
  gh repo view "$REPO" >/dev/null 2>&1 || { echo "✗ 无法访问仓库 $REPO" >&2; exit 1; }
fi

# 确保 origin 指向本仓库（已存在则不动）
if ! git remote get-url origin >/dev/null 2>&1; then
  git remote add origin "https://github.com/$REPO.git"
elif [ "$(git remote get-url origin)" != "https://github.com/$REPO.git" ]; then
  git remote set-url origin "https://github.com/$REPO.git"
fi

# ---------- 3. 本地构建 + 签名 ----------
[ -f .env ] || { echo "✗ 缺少项目根 .env（含签名密钥）" >&2; exit 1; }
echo "→ 本地构建 $TAG（release，含签名）..."
source .env && pnpm tauri build

# ---------- 4. 提交并推送 main（不推 tag） ----------
git add -A
if git diff --cached --quiet; then
  echo "→ 无代码改动，跳过提交"
else
  git commit -m "chore: release $VERSION"
  git push origin main
fi

# ---------- 5. 组装更新器元数据 ----------
NSIS_DIR="src-tauri/target/release/bundle/nsis"
SETUP_EXE="$(ls "$NSIS_DIR"/*_x64-setup.exe 2>/dev/null | head -1)"
[ -n "$SETUP_EXE" ] || { echo "✗ 未找到 NSIS 安装包" >&2; exit 1; }
SIG_FILE="${SETUP_EXE}.sig"
[ -f "$SIG_FILE" ] || { echo "✗ 缺少签名文件 $(basename "$SIG_FILE")" >&2; exit 1; }

EXE_NAME="$(basename "$SETUP_EXE")"
SIGNATURE="$(tr -d '\n' < "$SIG_FILE")"
DOWNLOAD_URL="https://github.com/$REPO/releases/download/$TAG/$EXE_NAME"

node -e "
  const fs = require('fs');
  const latest = {
    version: '$VERSION',
    notes: 'See the assets to download and install this version.',
    pub_date: new Date().toISOString(),
    platforms: {
      'windows-x86_64': {
        signature: '$SIGNATURE',
        url: '$DOWNLOAD_URL'
      }
    }
  };
  fs.writeFileSync('$NSIS_DIR/latest.json', JSON.stringify(latest, null, 2) + '\n');
"
echo "→ latest.json 已生成（url: $DOWNLOAD_URL）"

# 国内镜像版本：url 改写为 gh-proxy 前缀
sed "s|https://github.com/$REPO/releases/download/|https://gh-proxy.com/https://github.com/$REPO/releases/download/|g" \
  "$NSIS_DIR/latest.json" > "$NSIS_DIR/latest-cn.json"

# ---------- 6. gh 创建/更新 Release 并上传 ----------
if gh release view "$TAG" >/dev/null 2>&1; then
  echo "→ Release $TAG 已存在，更新资产 ..."
  gh release upload "$TAG" "$SETUP_EXE" "$SIG_FILE" --clobber
  gh release upload "$TAG" "$NSIS_DIR/latest.json" "$NSIS_DIR/latest-cn.json" --clobber
else
  echo "→ 创建 Release $TAG ..."
  gh release create "$TAG" "$SETUP_EXE" "$SIG_FILE" \
    "$NSIS_DIR/latest.json" "$NSIS_DIR/latest-cn.json" \
    --repo "$REPO" \
    --title "Image Viewer $VERSION" \
    --notes "See the assets to download and install this version."
fi

echo ""
echo "✓ 发版完成: https://github.com/$REPO/releases/tag/$TAG"
echo "  更新器 endpoints 会自动拉取 latest.json（镜像优先）"
