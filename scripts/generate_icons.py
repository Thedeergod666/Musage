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

# 画布外圈 padding：macOS HIG 模板留 13/16 = 0.8125 art（≈ 9.4% padding），
# Reddit r/PWA 共识也是 ~10%。我们取 7% —— 比 12% 大一截（白底 ≈ 86%
# 画布），更接近 VSCode/WPS 在 dock 上的视觉密度；又比 5% 安全区大一圈，
# 避免 macOS 26 Tahoe 的 "squircle jail" 把它塞进更小的灰盒。
ICON_PADDING_RATIO = 0.07
# 字号占边长比例。0.50 加上 7% padding 后 M = 50% 画布 = 58% 白底。
M_SCALE = 0.50
# Ring 装饰：相对于**白底**的边距。0.08 = 白底边距 8%，贴在白底内侧
# 不顶到圆角边；之前的 0.12 让 ring 离边太远、视觉上"空"。
RING_MARGIN = 0.08
# Ring 描边
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

    Layout（≥32）：
      - 1024 canvas 周边留 ~7% 透明 padding（macOS HIG / Apple 官方推荐）
        Dock 渲染时白底不撑满槽位，跟 VSCode/WPS 视觉密度对齐
      - 内层放圆角矩形白底 + 居中 ring + 居中 M
        ring 相对白底 8% inset，不顶到圆角边

    Layout（≤24，Win 任务栏/标题栏 100/150% DPI 帧）：
      - **无 padding**（full-bleed），**轻圆角**（10%），**大 M**（0.66）
      - 原 7% padding + 22% radius + 50% M 在 16x16 让 M 只占 ~8 px，
        笔画细到一锯齿就糊；小尺寸切「无 padding + 大 M」让笔画粗+
        边缘清，不带 ring（细到 1 px 时 inset+stroke 会糊成虚线）。
      - 32+ 保留原 dock 风格（padding + ring + M=50%）。
    """
    img = Image.new("RGBA", (size, size), (0, 0, 0, 0))
    d = ImageDraw.Draw(img)

    is_small = size <= 24
    if is_small:
        pad = 0
        sq_size = size
        r = max(1, int(sq_size * 0.10))
        m_scale = 0.66
    else:
        pad = int(size * ICON_PADDING_RATIO)
        sq_size = size - 2 * pad
        r = int(sq_size * 0.225)
        m_scale = M_SCALE

    # 圆角白底
    d.rounded_rectangle(
        [(pad, pad), (pad + sq_size - 1, pad + sq_size - 1)],
        radius=r, fill=BG,
    )

    # Ring 装饰：仅 ≥32 画。≤24 帧上 ring 会糊成虚线，不画反而干净。
    if RING_MARGIN > 0 and size >= 32:
        ring_offset = int(sq_size * RING_MARGIN)
        rx0 = pad + ring_offset
        ry0 = pad + ring_offset
        rx1 = pad + sq_size - 1 - ring_offset
        ry1 = pad + sq_size - 1 - ring_offset
        d.ellipse(
            [(rx0, ry0), (rx1, ry1)],
            outline=RING, width=max(1, int(size * RING_STROKE)),
        )

    # 中心 "M" —— anchor="mm" 真正像素级居中
    if size >= 16:
        font, is_bold = find_font(int(size * m_scale))
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
# PIL 的 ICO 插件有个坑：save(sizes=..., append_images=...) 时，sizes
# 用来 resize **第一张**图到那些尺寸，append_images 里的图被忽略 → 实际
# 写出的 .ico 只含第一张图的尺寸（之前 16x16 在前 → Windows 拿到 16x16，
# dock/任务栏全糊）。修法：把**最大**的图当 base，append_images 按大小
# 降序塞后面，**不要传 sizes=**。PIL 会用每张图的原生尺寸编码进 ICO。
ico_sizes = [(16, 16), (24, 24), (32, 32), (48, 48), (64, 64), (128, 128), (256, 256)]
ico_images = [make_icon(s) for s, _ in ico_sizes]
# 大到小排列 —— 大图作 base，小图 append 进 ICO 容器
ico_images_sorted = sorted(ico_images, key=lambda img: -img.size[0])
ico_images_sorted[0].save(
    os.path.join(OUT, "icon.ico"),
    format="ICO",
    append_images=ico_images_sorted[1:],
)
print(f"[ok] icon.ico (native sizes: {[img.size for img in ico_images_sorted]})")

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
