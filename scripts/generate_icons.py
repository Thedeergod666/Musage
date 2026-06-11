"""Generate Musage icons.

设计：
- 主图标：白底 + 加粗 M（M 用 anchor="mm" 居中，留出 padding 避免 macOS 看着偏大）
- 托盘底图（tray-base.png）：与主图标一致，托盘运行时会再叠彩色圆 + 文字
- ICO：每个尺寸**原生渲染**（不降采样）—— 避免 Windows 模糊
"""
import sys, os
sys.stdout.reconfigure(encoding="utf-8")

from PIL import Image, ImageDraw, ImageFont

OUT = os.path.join(os.path.dirname(os.path.dirname(os.path.abspath(__file__))), "src-tauri", "icons")
os.makedirs(OUT, exist_ok=True)

# 配色：白底 + 黑色加粗 M + 黑色细环（极简，避免和网易云撞色）
BG = (255, 255, 255, 255)     # 纯白
RING = (0, 0, 0, 200)          # 黑环（半透明一点不那么硬）
FG = (0, 0, 0, 255)            # 黑色 M

# 字号占边长比例。0.50 比之前的 0.58 留出明显 padding，
# 避免 macOS 看着偏大；小尺寸（16px）下 M 笔画也不会因为贴边而糊。
M_SCALE = 0.50
# Ring 距离边距比例。0.18 给 M 留 ~8% 视觉余量。
RING_MARGIN = 0.18
# Ring 描边比例。1/48 = 2.08% 边粗，足够细不喧宾夺主。
RING_STROKE = 1 / 48


def find_font(size: int):
    """返回 (font, is_bold)。优先 Arial Black / Heavy 类粗体，失败兜底 regular + 多层 stroke。"""
    bold_paths = [
        # Windows —— Black/Heavy 最重
        "C:/Windows/Fonts/ariblk.ttf",     # Arial Black
        "C:/Windows/Fonts/arialbd.ttf",    # Arial Bold
        "C:/Windows/Fonts/segoeuib.ttf",   # Segoe UI Bold
        "C:/Windows/Fonts/calibrib.ttf",   # Calibri Bold
        # macOS —— Black/Heavy 最重
        "/System/Library/Fonts/Supplemental/Arial Black.ttf",
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
    """生成 Musage 图标（指定尺寸原生渲染，不用降采样）。

    Layout：
      - 圆角矩形白底
      - 黑色细环装饰
      - 中心 "M"，用 anchor="mm" 保证完美居中（不受字体 bbox 偏移影响）
    """
    img = Image.new("RGBA", (size, size), (0, 0, 0, 0))
    d = ImageDraw.Draw(img)

    # macOS 风格圆角矩形 —— 纯白底
    r = int(size * 0.225)
    d.rounded_rectangle([(0, 0), (size - 1, size - 1)], radius=r, fill=BG)

    # 外圈细环（黑色，装饰）
    ring_margin = int(size * RING_MARGIN)
    d.ellipse(
        [(ring_margin, ring_margin), (size - ring_margin, size - ring_margin)],
        outline=RING, width=max(1, int(size * RING_STROKE)),
    )

    # 中心 "M" —— anchor="mm" 真正像素级居中
    if size >= 16:
        font, is_bold = find_font(int(size * M_SCALE))
        if font is not None:
            if is_bold:
                d.text((size / 2, size / 2), "M", font=font, fill=FG, anchor="mm")
            else:
                # 兜底：stroke 模拟 Black 粗体
                sw = max(1, int(size * 0.06))
                d.text(
                    (size / 2, size / 2), "M", font=font, fill=FG,
                    stroke_width=sw, stroke_fill=FG, anchor="mm",
                )
                d.text((size / 2, size / 2), "M", font=font, fill=FG, anchor="mm")
    return img


# ── 1. PNG 各尺寸（每个尺寸原生渲染）──
png_targets = [
    (32, "32x32.png"),
    (128, "128x128.png"),
    (256, "128x128@2x.png"),
]
for size, name in png_targets:
    make_icon(size).save(os.path.join(OUT, name))
    print(f"[ok] {name}")

# tray-base.png —— 与主图标风格一致，托盘会再叠彩色 + 文字
make_icon(32).save(os.path.join(OUT, "tray-base.png"))
print("[ok] tray-base.png")

# ── 2. icon.ico（多尺寸，**每个尺寸原生渲染**）──
# PIL 的 save(sizes=) 是把第一张图缩到所有列出的尺寸；想要原生态辨率
# 需要 append_images，且每张图都要对应到 sizes 中的一项。
ico_sizes = [(16, 16), (24, 24), (32, 32), (48, 48), (64, 64), (128, 128), (256, 256)]
ico_images = [make_icon(s) for s, _ in ico_sizes]
ico_images[0].save(
    os.path.join(OUT, "icon.ico"),
    format="ICO",
    sizes=ico_sizes,
    append_images=ico_images[1:],
)
print(f"[ok] icon.ico (native sizes: {[s for s, _ in ico_sizes]})")

# ── 3. icon.png (master for settings UI / fallback) ──
make_icon(1024).save(os.path.join(OUT, "icon.png"))
print("[ok] icon.png (1024x1024 master)")

# ── 4. icon.icns —— macOS 上用 iconutil 拼一个真 .icns ──
import subprocess, shutil, tempfile

icns_path = os.path.join(OUT, "icon.icns")
if sys.platform == "darwin" and shutil.which("iconutil"):
    with tempfile.TemporaryDirectory() as tmp:
        iconset = os.path.join(tmp, "icon.iconset")
        os.makedirs(iconset)
        sizes = [
            (16, "icon_16x16.png"),
            (32, "icon_16x16@2x.png"),     # 16pt @2x
            (32, "icon_32x32.png"),
            (64, "icon_32x32@2x.png"),     # 32pt @2x
            (128, "icon_128x128.png"),
            (256, "icon_128x128@2x.png"),  # 128pt @2x
            (256, "icon_256x256.png"),
            (512, "icon_256x256@2x.png"),  # 256pt @2x
            (512, "icon_512x512.png"),
            (1024, "icon_512x512@2x.png"), # 512pt @2x
        ]
        for size, name in sizes:
            make_icon(size).save(os.path.join(iconset, name))
        try:
            subprocess.run(
                ["iconutil", "-c", "icns", iconset, "-o", icns_path],
                check=True, capture_output=True,
            )
            print(f"[ok] icon.icns (proper macOS icns, {os.path.getsize(icns_path)} bytes)")
        except subprocess.CalledProcessError as e:
            print(f"[warn] iconutil failed: {e.stderr.decode(errors='ignore')}")
            make_icon(512).save(icns_path)
            print(f"[warn] icon.icns is PNG fallback")
else:
    make_icon(512).save(icns_path)
    print(f"[warn] icon.icns is PNG placeholder (macOS-only iconutil used for real icns)")

print(f"\nAll icons generated -> {OUT}")
