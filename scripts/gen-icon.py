"""生成应用图标源图（1024x1024，深色渐变底 + accent 渐变圆角卡片 + 太阳与山）"""
from PIL import Image, ImageDraw
import os

W = H = 1024
OUT = os.path.join(os.path.dirname(__file__), "..", "src-tauri", "icons", "source.png")


def main():
    img = Image.new("RGB", (W, H))

    # 背景：垂直渐变 #0a0a0d -> #16161b
    d = ImageDraw.Draw(img)
    for y in range(H):
        t = y / (H - 1)
        c = (round(10 + 12 * t), round(11 + 14 * t), round(13 + 18 * t))
        d.line([(0, y), (W, y)], fill=c)

    # 圆角卡片：水平渐变 #4DA3FF -> #0A84FF
    grad = Image.new("RGB", (W, H))
    gd = ImageDraw.Draw(grad)
    for x in range(W):
        t = max(0.0, min(1.0, (x - 192) / 640))
        c = (
            round(77 + (10 - 77) * t),
            round(163 + (132 - 163) * t),
            255,
        )
        gd.line([(x, 0), (x, H)], fill=c)
    mask = Image.new("L", (W, H), 0)
    md = ImageDraw.Draw(mask)
    md.rounded_rectangle([192, 252, 832, 772], radius=120, fill=255)
    img.paste(grad, (0, 0), mask)

    # 白色元素：太阳 + 两层山
    d = ImageDraw.Draw(img)
    d.ellipse([372, 342, 512, 482], fill=(255, 255, 255))
    d.polygon([(330, 640), (500, 448), (680, 640)], fill=(255, 255, 255))
    d.polygon([(502, 640), (672, 416), (836, 640)], fill=(228, 240, 255))

    os.makedirs(os.path.dirname(OUT), exist_ok=True)
    img.save(OUT, "PNG")
    print(f"icon source saved: {OUT}")


if __name__ == "__main__":
    main()
