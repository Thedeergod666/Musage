"""Generate Musage placeholder icons."""
import sys, os
sys.stdout.reconfigure(encoding="utf-8")

from PIL import Image, ImageDraw

OUT = os.path.join(os.path.dirname(os.path.dirname(os.path.abspath(__file__))), "src-tauri", "icons")
os.makedirs(OUT, exist_ok=True)

# Colors
BG = (78, 56, 196, 255)
FG = (255, 255, 255, 255)
HIGHLIGHT = (140, 100, 255, 255)


def make_icon(size: int) -> Image.Image:
    img = Image.new("RGBA", (size, size), (0, 0, 0, 0))
    d = ImageDraw.Draw(img)
    # 圆角矩形
    r = int(size * 0.22)
    d.rounded_rectangle([(0, 0), (size - 1, size - 1)], radius=r, fill=BG)
    # 进度条圆环（外圈）
    margin = int(size * 0.18)
    d.ellipse(
        [(margin, margin), (size - margin, size - margin)],
        outline=HIGHLIGHT, width=max(1, size // 16),
    )
    # 中心 "M" 字（Musage 标识）
    if size >= 32:
        try:
            from PIL import ImageFont
            font = None
            for path in [
                "C:/Windows/Fonts/seguiemj.ttf",
                "C:/Windows/Fonts/segoeui.ttf",
                "C:/Windows/Fonts/arial.ttf",
            ]:
                if os.path.exists(path):
                    font = ImageFont.truetype(path, int(size * 0.5))
                    break
            if font:
                bbox = d.textbbox((0, 0), "M", font=font)
                tw, th = bbox[2] - bbox[0], bbox[3] - bbox[1]
                d.text(
                    ((size - tw) / 2 - bbox[0], (size - th) / 2 - bbox[1] - size * 0.05),
                    "M", font=font, fill=FG,
                )
        except Exception as e:
            print(f"[warn] font: {e}")
    return img


# 1. PNG 各尺寸
for size, name in [(32, "32x32.png"), (128, "128x128.png"), (256, "128x128@2x.png")]:
    make_icon(size).save(os.path.join(OUT, name))
    print(f"[ok] {name}")

# tray-base.png
make_icon(32).save(os.path.join(OUT, "tray-base.png"))
print("[ok] tray-base.png")

# icon.ico (multi-size)
ico_sizes = [(16, 16), (32, 32), (48, 48), (64, 64), (128, 128), (256, 256)]
ico_imgs = [make_icon(s[0]) for s in ico_sizes]
ico_imgs[0].save(
    os.path.join(OUT, "icon.ico"),
    format="ICO",
    sizes=ico_sizes,
    append_images=ico_imgs[1:],
)
print("[ok] icon.ico")

# icon.png (master)
make_icon(1024).save(os.path.join(OUT, "icon.png"))
print("[ok] icon.png (1024x1024 master)")

# icon.icns placeholder
icns_placeholder = os.path.join(OUT, "icon.icns")
if not os.path.exists(icns_placeholder):
    make_icon(512).save(icns_placeholder)
    print("[warn] icon.icns is PNG placeholder (macOS build needs png2icns)")

print(f"\nAll icons generated -> {OUT}")
