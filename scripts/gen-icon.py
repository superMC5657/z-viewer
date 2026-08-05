"""一键生成应用图标（黄色高清图库风格）并输出 icons 目录下全套文件。

设计：黄色垂直渐变圆角底 + 白色相框（内部黄色渐变）+ 白色太阳与两座山。
覆盖：根目录 PNG / icon.ico / icon.icns / ios / android（含自适应前景）。
"""
from PIL import Image, ImageDraw, ImageFilter
import os, struct, io

W = H = 1024
ICONS = os.path.normpath(os.path.join(os.path.dirname(__file__), "..", "src-tauri", "icons"))

# ---------- 颜色 ----------
BG_TOP = (255, 222, 89)      # 背景渐变顶部（亮黄）
BG_BOT = (255, 179, 0)       # 背景渐变底部（金黄）
IN_TOP = (255, 227, 138)     # 相框内部渐变顶部
IN_BOT = (255, 193, 7)       # 相框内部渐变底部（Amber）
WHITE = (255, 255, 255)


def lerp(a, b, t):
    return tuple(round(a[i] + (b[i] - a[i]) * t) for i in range(3))


def vgrad(size, top, bottom):
    """垂直渐变图（RGB）"""
    im = Image.new("RGB", size)
    d = ImageDraw.Draw(im)
    for y in range(size[1]):
        d.line([(0, y), (size[0], y)], fill=lerp(top, bottom, y / max(1, size[1] - 1)))
    return im


def draw_frame(bg_color_bottom=IN_BOT):
    """白色相框 + 内部黄色渐变 + 太阳与两座山（透明背景 RGBA）"""
    frame = Image.new("RGBA", (W, H), (0, 0, 0, 0))
    d = ImageDraw.Draw(frame)
    # 相框外白边
    d.rounded_rectangle([192, 262, 832, 762], radius=80, fill=WHITE + (255,))
    # 内部黄色渐变
    inner = vgrad((W, H), IN_TOP, bg_color_bottom).convert("RGBA")
    imask = Image.new("L", (W, H), 0)
    ImageDraw.Draw(imask).rounded_rectangle([232, 302, 792, 722], radius=48, fill=255)
    inner.putalpha(imask)
    frame.alpha_composite(inner)
    # 太阳
    d.ellipse([596, 364, 684, 452], fill=WHITE + (255,))
    # 大山（右）
    d.polygon([(288, 692), (652, 500), (806, 692)], fill=WHITE + (255,))
    # 小山（左前）
    d.polygon([(280, 692), (398, 584), (544, 692)], fill=WHITE + (255,))
    return frame


def build_master():
    """1024 圆角透明主图"""
    # 背景渐变
    bg = vgrad((W, H), BG_TOP, BG_BOT).convert("RGBA")
    # 顶部高光
    hl = Image.new("RGBA", (W, H), (0, 0, 0, 0))
    dhl = ImageDraw.Draw(hl)
    for y in range(int(H * 0.42)):
        a = round(64 * (1 - y / (H * 0.42)))
        dhl.line([(0, y), (W, y)], fill=(255, 255, 255, a))
    bg.alpha_composite(hl)
    # 圆角 mask
    mask = Image.new("L", (W, H), 0)
    ImageDraw.Draw(mask).rounded_rectangle([0, 0, W - 1, H - 1], radius=224, fill=255)
    # 相框投影
    shadow = Image.new("RGBA", (W, H), (0, 0, 0, 0))
    ImageDraw.Draw(shadow).rounded_rectangle([192, 274, 832, 784], radius=80, fill=(0, 0, 0, 70))
    shadow = shadow.filter(ImageFilter.GaussianBlur(14))
    bg.alpha_composite(shadow)
    bg.putalpha(mask)
    bg.alpha_composite(draw_frame())
    return bg


def build_square():
    """全幅方形（iOS 用，无圆角透明）"""
    bg = vgrad((W, H), BG_TOP, BG_BOT).convert("RGBA")
    hl = Image.new("RGBA", (W, H), (0, 0, 0, 0))
    dhl = ImageDraw.Draw(hl)
    for y in range(int(H * 0.42)):
        a = round(64 * (1 - y / (H * 0.42)))
        dhl.line([(0, y), (W, y)], fill=(255, 255, 255, a))
    bg.alpha_composite(hl)
    shadow = Image.new("RGBA", (W, H), (0, 0, 0, 0))
    ImageDraw.Draw(shadow).rounded_rectangle([192, 274, 832, 784], radius=80, fill=(0, 0, 0, 70))
    shadow = shadow.filter(ImageFilter.GaussianBlur(14))
    bg.alpha_composite(shadow)
    bg.alpha_composite(draw_frame())
    return bg


def build_round():
    """圆形（Android round 图标）"""
    bg = vgrad((W, H), BG_TOP, BG_BOT).convert("RGBA")
    hl = Image.new("RGBA", (W, H), (0, 0, 0, 0))
    dhl = ImageDraw.Draw(hl)
    for y in range(int(H * 0.42)):
        a = round(64 * (1 - y / (H * 0.42)))
        dhl.line([(0, y), (W, y)], fill=(255, 255, 255, a))
    bg.alpha_composite(hl)
    shadow = Image.new("RGBA", (W, H), (0, 0, 0, 0))
    ImageDraw.Draw(shadow).rounded_rectangle([192, 274, 832, 784], radius=80, fill=(0, 0, 0, 70))
    shadow = shadow.filter(ImageFilter.GaussianBlur(14))
    bg.alpha_composite(shadow)
    mask = Image.new("L", (W, H), 0)
    ImageDraw.Draw(mask).ellipse([0, 0, W - 1, H - 1], fill=255)
    bg.putalpha(mask)
    bg.alpha_composite(draw_frame())
    return bg


def build_foreground():
    """Android 自适应图标前景层：仅白色相框图形，背景透明"""
    fg = Image.new("RGBA", (W, H), (0, 0, 0, 0))
    fg.alpha_composite(draw_frame())
    return fg


def save_resized(src, path, size):
    os.makedirs(os.path.dirname(path), exist_ok=True)
    src.resize((size, size), Image.LANCZOS).save(path, "PNG")
    print(f"  {os.path.relpath(path, ICONS)}  {size}x{size}")


def build_icns(src):
    """手工打包完整 ICNS：@1x 与 @2x 全部帧（PIL 自动保存会缺 16/32 @1x）"""
    frames = [
        ("ic04", 16), ("icp4", 32),   # 16@1x, 16@2x
        ("ic05", 32), ("ic11", 64),   # 32@1x, 32@2x
        ("ic07", 128), ("ic12", 128),  # 128@1x, 64@2x(128)
        ("ic08", 256), ("ic13", 512),  # 256@1x, 256@2x
        ("ic09", 512), ("ic14", 1024),  # 512@1x, 512@2x
        ("ic10", 1024),                 # 1024@1x
    ]
    chunks = b""
    for typ, size in frames:
        buf = io.BytesIO()
        src.resize((size, size), Image.LANCZOS).save(buf, "PNG")
        data = buf.getvalue()
        chunks += struct.pack(">4sI", typ.encode(), 8 + len(data)) + data
    return b"icns" + struct.pack(">I", 8 + len(chunks)) + chunks


def main():
    master = build_master()
    square = build_square()
    round_img = build_round()
    fg = build_foreground()

    root_pngs = {
        "32x32.png": 32, "64x64.png": 64, "128x128.png": 128, "128x128@2x.png": 256,
        "icon.png": 512, "source.png": 1024,
        "Square30x30Logo.png": 30, "Square44x44Logo.png": 44, "Square71x71Logo.png": 71,
        "Square89x89Logo.png": 89, "Square107x107Logo.png": 107, "Square142x142Logo.png": 142,
        "Square150x150Logo.png": 150, "Square284x284Logo.png": 284, "Square310x310Logo.png": 310,
        "StoreLogo.png": 50,
    }
    ios_pngs = {
        "AppIcon-20x20@1x.png": 20, "AppIcon-20x20@2x-1.png": 40, "AppIcon-20x20@2x.png": 40,
        "AppIcon-20x20@3x.png": 60, "AppIcon-29x29@1x.png": 29, "AppIcon-29x29@2x-1.png": 58,
        "AppIcon-29x29@2x.png": 58, "AppIcon-29x29@3x.png": 87, "AppIcon-40x40@1x.png": 40,
        "AppIcon-40x40@2x-1.png": 80, "AppIcon-40x40@2x.png": 80, "AppIcon-40x40@3x.png": 120,
        "AppIcon-60x60@2x.png": 120, "AppIcon-60x60@3x.png": 180, "AppIcon-76x76@1x.png": 76,
        "AppIcon-76x76@2x.png": 152, "AppIcon-83.5x83.5@2x.png": 167, "AppIcon-512@2x.png": 1024,
    }
    dens = {"mdpi": 48, "hdpi": 72, "xhdpi": 96, "xxhdpi": 144, "xxxhdpi": 192}
    fg_dens = {"mdpi": 108, "hdpi": 162, "xhdpi": 216, "xxhdpi": 324, "xxxhdpi": 432}

    print("== 根目录 PNG ==")
    for name, size in root_pngs.items():
        save_resized(master, os.path.join(ICONS, name), size)

    print("== icon.ico ==")
    master.save(os.path.join(ICONS, "icon.ico"),
                sizes=[(16, 16), (24, 24), (32, 32), (48, 48), (64, 64), (128, 128), (256, 256)])
    print("  icon.ico")

    print("== icon.icns ==")
    with open(os.path.join(ICONS, "icon.icns"), "wb") as f:
        f.write(build_icns(master))
    print("  icon.icns")

    print("== iOS ==")
    for name, size in ios_pngs.items():
        save_resized(square, os.path.join(ICONS, "ios", name), size)

    print("== Android ==")
    for dens_name, size in dens.items():
        base = os.path.join(ICONS, "android", f"mipmap-{dens_name}")
        save_resized(master, os.path.join(base, "ic_launcher.png"), size)
        save_resized(round_img, os.path.join(base, "ic_launcher_round.png"), size)
    for dens_name, size in fg_dens.items():
        # 自适应前景：图形缩放到画布中心 66/108 ≈ 0.611 的安全区内
        fg_resized = fg.resize((size, size), Image.LANCZOS)
        scale = 0.60
        fg_crop = fg_resized.resize((round(size * scale),) * 2, Image.LANCZOS)
        canvas = Image.new("RGBA", (size, size), (0, 0, 0, 0))
        canvas.alpha_composite(fg_crop, ((size - fg_crop.width) // 2, (size - fg_crop.height) // 2))
        path = os.path.join(ICONS, "android", f"mipmap-{dens_name}", "ic_launcher_foreground.png")
        canvas.save(path, "PNG")
        print(f"  android/mipmap-{dens_name}/ic_launcher_foreground.png  {size}x{size}")

    print("完成。")


if __name__ == "__main__":
    main()
