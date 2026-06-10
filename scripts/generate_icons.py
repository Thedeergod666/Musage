"""Generate Musage icons.

- 主图标：红底 + 加粗 M（M 居中略下移）
- 托盘底图（tray-base.png）：与主图标一致，托盘运行时会再叠彩色圆 + 文字
"""
import sys, os
sys.stdout.reconfigure(encoding="utf-8")

from PIL import Image, ImageDraw, ImageFont

OUT = os.path.join(os.path.dirname(os.path.dirname(os.path.abspath(__file__))), "src-tauri", "icons")
os.makedirs(OUT, exist_ok=True)

# 配色：红底 + 白色加粗 M
BG = (220, 38, 38, 255)         # 主红
BG_GRAD_TOP = (240, 60, 60, 255)  # 顶部稍亮
BG_GRAD_BOT = (200, 30, 30, 255)  # 底部稍暗
RING = (255, 255, 255, 70)        # 外圈细环
FG = (255, 255, 255, 255)


def find_font(size: int):
    """返回 (font, is_bold)。优先找 bold 字体，失败兜底 regular + stroke 加粗。"""
    bold_paths = [
        # Windows
        "C:/Windows/Fonts/arialbd.ttf",
        "C:/Windows/Fonts/segoeuib.ttf",
        "C:/Windows/Fonts/calibrib.ttf",
        # macOS
        "/System/Library/Fonts/Helvetica.ttc",
        "/System/Library/Fonts/Supplemental/Arial Bold.ttf",
        "/Library/Fonts/Arial Bold.ttf",
        # Linux
        "/usr/share/fonts/truetype/dejavu/DejaVuSans-Bold.ttf",
        "/usr/share/fonts/truetype/liberation/LiberationSans-Bold.ttf",
    ]
    for path in bold_paths:
        if os.path.exists(path):
            try:
                return ImageFont.truetype(path, size), True
            except Exception:
                pass

    # 兜底 regular —— 后面会用 stroke 模拟加粗
    regular_paths = [
        "C:/Windows/Fonts/seguiemj.ttf",
        "C:/Windows/Fonts/segoeui.ttf",
        "C:/Windows/Fonts/arial.ttf",
        "/System/Library/Fonts/Helvetica.ttc",
        "/System/Library/Fonts/SFNS.ttf",
        "/Library/Fonts/Arial.ttf",
        "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
    ]
    for path in regular_paths:
        if os.path.exists(path):
            try:
                return ImageFont.truetype(path, size), False
            except Exception:
                pass

    return None, False


def make_icon(size: int) -> Image.Image:
    img = Image.new("RGBA", (size, size), (0, 0, 0, 0))
    d = ImageDraw.Draw(img)

    # 圆角矩形（macOS 风格 app icon）
    r = int(size * 0.225)
    # 简单纵向渐变：分两段绘制近似
    d.rounded_rectangle([(0, 0), (size - 1, int(size * 0.55))], radius=r, fill=BG_GRAD_TOP)
    d.rounded_rectangle([(0, int(size * 0.45)), (size - 1, size - 1)], radius=r, fill=BG_GRAD_BOT)
    # 重新画一遍完整底（盖住接缝，圆角会按外轮廓剪）
    d.rounded_rectangle([(0, 0), (size - 1, size - 1)], radius=r, fill=BG)
    # 顶部到底部细渐变：再叠一层 alpha 渐变
    overlay = Image.new("RGBA", (size, size), (0, 0, 0, 0))
    od = ImageDraw.Draw(overlay)
    for y in range(size):
        a = int(20 * (y / size))  # 顶部到底部 alpha 0→20 一点点暗
        od.line([(0, y), (size, y)], fill=(0, 0, 0, a))
    img = Image.alpha_composite(img, overlay)
    d = ImageDraw.Draw(img)

    # 外圈细环（白色低 alpha）—— 装饰
    ring_margin = int(size * 0.18)
    d.ellipse(
        [(ring_margin, ring_margin), (size - ring_margin, size - ring_margin)],
        outline=RING, width=max(1, size // 32),
    )

    # 中心 "M" 字 —— 加粗 + 居中略下移
    if size >= 16:
        font, is_bold = find_font(int(size * 0.52))
        if font is not None:
            bbox = d.textbbox((0, 0), "M", font=font)
            tw, th = bbox[2] - bbox[0], bbox[3] - bbox[1]
            # 居中（去掉原脚本向上偏移的 5%），让 M 落在圆心
            cx = (size - tw) / 2 - bbox[0]
            cy = (size - th) / 2 - bbox[1] + size * 0.02
            if is_bold:
                d.text((cx, cy), "M", font=font, fill=FG)
            else:
                # 兜底：用 stroke 模拟加粗
                sw = max(1, int(size * 0.045))
                d.text((cx, cy), "M", font=font, fill=FG, stroke_width=sw, stroke_fill=FG)
    return img


# 1. PNG 各尺寸
for size, name in [(32, "32x32.png"), (128, "128x128.png"), (256, "128x128@2x.png")]:
    make_icon(size).save(os.path.join(OUT, name))
    print(f"[ok] {name}")

# tray-base.png —— 与主图标风格一致，托盘会再叠彩色 + 文字
make_icon(32).save(os.path.join(OUT, "tray-base.png"))
print("[ok] tray-base.png")

# icon.ico (multi-size) —— PIL 已知问题：append_images+不同 size 不会真的
# 写出多尺寸。直接传 sizes 列表给第一张图就行（PIL 会从那张图 resize）。
ico_sizes = [(16, 16), (32, 32), (48, 48), (64, 64), (128, 128), (256, 256)]
ico_base = make_icon(256)  # 用最大那张作基础，PIL 内部 resize 到各 size
ico_base.save(
    os.path.join(OUT, "icon.ico"),
    format="ICO",
    sizes=ico_sizes,
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
