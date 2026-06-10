"""Generate Musage icons.

- 主图标：红底 + 加粗 M（M 居中略下移）
- 托盘底图（tray-base.png）：与主图标一致，托盘运行时会再叠彩色圆 + 文字
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

    # 圆角矩形（macOS 风格 app icon）—— 纯白底
    r = int(size * 0.225)
    d.rounded_rectangle([(0, 0), (size - 1, size - 1)], radius=r, fill=BG)

    # 外圈细环（黑色，装饰）—— 比之前更细一点
    ring_margin = int(size * 0.20)
    d.ellipse(
        [(ring_margin, ring_margin), (size - ring_margin, size - ring_margin)],
        outline=RING, width=max(1, size // 40),
    )

    # 中心 "M" 字 —— 加粗 + 居中略下移
    if size >= 16:
        font, is_bold = find_font(int(size * 0.55))
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
                sw = max(1, int(size * 0.05))
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

# icon.icns —— 在 macOS 上用 iconutil 从多尺寸 PNG 拼一个真 .icns
# 其他平台没这个工具就退化成 PNG（足够 Tauri 编译通过）
import subprocess, shutil, tempfile

icns_path = os.path.join(OUT, "icon.icns")
if sys.platform == "darwin" and shutil.which("iconutil"):
    with tempfile.TemporaryDirectory() as tmp:
        iconset = os.path.join(tmp, "icon.iconset")
        os.makedirs(iconset)
        # 必需的多尺寸 PNG
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
    # 非 macOS 或没 iconutil：保持 PNG 兜底
    make_icon(512).save(icns_path)
    print(f"[warn] icon.icns is PNG placeholder (macOS-only iconutil used for real icns)")

print(f"\nAll icons generated -> {OUT}")
