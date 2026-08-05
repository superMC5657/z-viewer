"""生成 test-images/ 测试图库：3 个同级文件夹，验证自然排序与跨文件夹浏览。

- A/: 1, 2, 3, 10, 11（验证 natord：1,2,3,10,11 而非 1,10,11,2,3）
- B/: img_1..img_3（含 GIF/BMP/JPG/WEBP 各一张，验证常见格式）
- C/: c01, c02
"""
from PIL import Image, ImageDraw, ImageFont
import os

BASE = os.path.join(os.path.dirname(__file__), "..", "test-images")

FONT_CANDIDATES = [
    r"C:\Windows\Fonts\segoeui.ttf",
    r"C:\Windows\Fonts\arial.ttf",
    r"C:\Windows\Fonts\msyh.ttc",
    r"C:\Windows\Fonts\DejaVuSans.ttf",
]


def find_font(size):
    for f in FONT_CANDIDATES:
        if os.path.exists(f):
            try:
                return ImageFont.truetype(f, size)
            except Exception:
                continue
    return ImageFont.load_default()


def make_image(path, w, h, hue_shift, label, sublabel, fmt="PNG"):
    img = Image.new("RGB", (w, h))
    d = ImageDraw.Draw(img)
    # 垂直渐变（基于色相偏移的蓝紫/青/橙系）
    top = _hsv(hue_shift, 0.55, 0.34)
    bot = _hsv((hue_shift + 40) % 360, 0.65, 0.62)
    for y in range(h):
        t = y / max(1, h - 1)
        c = tuple(round(top[i] + (bot[i] - top[i]) * t) for i in range(3))
        d.line([(0, y), (w, y)], fill=c)

    # 中央大数字
    font = find_font(max(96, min(w, h) // 4))
    text = label
    bbox = d.textbbox((0, 0), text, font=font)
    tw, th = bbox[2] - bbox[0], bbox[3] - bbox[1]
    d.text(((w - tw) / 2 - bbox[0], (h - th) / 2 - bbox[1] - 20), text, font=font,
           fill=(255, 255, 255))
    # 底部说明
    font2 = find_font(max(20, min(w, h) // 14))
    bbox2 = d.textbbox((0, 0), sublabel, font=font2)
    d.text(((w - (bbox2[2] - bbox2[0])) / 2 - bbox2[0], h - 90), sublabel, font=font2,
           fill=(255, 255, 255))

    if fmt == "PNG":
        img.save(path, "PNG")
    elif fmt == "JPEG":
        img.save(path, "JPEG", quality=90)
    elif fmt == "BMP":
        img.save(path, "BMP")
    elif fmt == "WEBP":
        img.save(path, "WEBP", quality=85)
    elif fmt == "GIF":
        # 简单两帧动画，验证 GIF 显示
        frames = [img]
        f2 = img.copy()
        d2 = ImageDraw.Draw(f2)
        d2.rectangle([0, 0, w - 1, h - 1], outline=(255, 255, 255), width=10)
        frames.append(f2)
        frames[0].save(path, "GIF", save_all=True, append_images=frames[1:], duration=400, loop=0)
    print(f"  {os.path.relpath(path, BASE)}  ({w}x{h})")


def _hsv(h, s, v):
    import colorsys
    r, g, b = colorsys.hsv_to_rgb(h / 360, s, v)
    return (round(r * 255), round(g * 255), round(b * 255))


def main():
    os.makedirs(BASE, exist_ok=True)
    # 清空旧内容
    for f in os.listdir(BASE):
        p = os.path.join(BASE, f)
        if os.path.isdir(p):
            import shutil
            shutil.rmtree(p)

    # A：自然排序验证（1,2,3,10,11）
    a = os.path.join(BASE, "A")
    os.makedirs(a)
    print("生成 A/（自然排序验证）...")
    for name in ["1", "2", "3", "10", "11"]:
        make_image(os.path.join(a, f"{name}.png"), 640 + int(name) * 40, 480 + int(name) * 20,
                   hue_shift=210, label=name, sublabel=f"A/{name}.png")

    # B：常见格式覆盖
    b = os.path.join(BASE, "B")
    os.makedirs(b)
    print("生成 B/（格式覆盖）...")
    make_image(os.path.join(b, "img_1.jpg"), 1200, 800, 40, "JPG", "B/img_1.jpg", fmt="JPEG")
    make_image(os.path.join(b, "img_2.gif"), 480, 360, 100, "GIF", "B/img_2.gif (动画)", fmt="GIF")
    make_image(os.path.join(b, "img_3.webp"), 900, 600, 320, "WEBP", "B/img_3.webp", fmt="WEBP")
    make_image(os.path.join(b, "img_4.bmp"), 700, 700, 260, "BMP", "B/img_4.bmp", fmt="BMP")
    make_image(os.path.join(b, "img_5.png"), 800, 533, 150, "PNG", "B/img_5.png")

    # C：跨文件夹衔接的第三个文件夹
    c = os.path.join(BASE, "C")
    os.makedirs(c)
    print("生成 C/ ...")
    make_image(os.path.join(c, "c01.png"), 1024, 640, 280, "C1", "C/c01.png")
    make_image(os.path.join(c, "c02.png"), 640, 960, 330, "C2", "C/c02.png")

    print("完成。测试目录结构：")
    for root, _, files in os.walk(BASE):
        rel = os.path.relpath(root, BASE)
        for f in sorted(files):
            print(f"  {os.path.join(rel, f)}")


if __name__ == "__main__":
    main()
