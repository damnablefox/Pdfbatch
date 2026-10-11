"""生成 pdfbatch 的应用图标（可重跑；改颜色/构图就改这里）。

风格对齐 PdfCraft 的图标规则：满幅圆角方块、平涂、不用渐变与描边。
图形：纸页（PDF）+ 浅蓝填充块（填充层）+ 橙色书签丝带（书签）。

用法：
    python assets/app-icon/make_icons.py

产物（同目录）：
    pdfbatch-1024.png   主图
    pdfbatch-256.png    窗口图标（被 crates/ui 用 include_bytes 引用）
    pdfbatch.ico        Windows 可执行文件图标，16–256 多尺寸（被 apps/pdfbatch/build.rs 引用）
    candidates/         候选对比图（仅用于挑选方案，可随时删除）
"""

import os
from PIL import Image, ImageDraw, ImageFont

S = 1024
ROOT = os.path.dirname(os.path.abspath(__file__))
OUT = os.path.join(ROOT, "candidates")
# 预览图上的中文标签用项目自带的霞鹜文楷；找不到就退回默认字体（不影响图标本身）。
FONT = os.path.join(ROOT, "..", "..", "..", "craft-fonts", "fonts", "lxgw-wenkai", "LXGWWenKai-Regular.ttf")
os.makedirs(OUT, exist_ok=True)

INK = (27, 27, 30, 255)            # #1B1B1E 深墨
BLUE = (27, 99, 224, 255)          # #1B63E0 界面强调蓝
BLUE_DEEP = (21, 80, 180, 255)
BLUE_SOFT = (163, 189, 241, 255)   # 蓝 30% 叠在白纸上，模拟半透明填充
PAPER = (250, 250, 247, 255)
PAPER_DIM = (232, 232, 228, 255)
AMBER = (224, 138, 27, 255)        # #E08A1B


def px(v):
    return int(round(v * S))


def tile(base):
    img = Image.new("RGBA", (S, S), (0, 0, 0, 0))
    ImageDraw.Draw(img).rounded_rectangle([0, 0, S - 1, S - 1], radius=px(0.22), fill=base)
    return img


def rect(d, box, r, fill):
    d.rounded_rectangle([px(box[0]), px(box[1]), px(box[2]), px(box[3])], radius=px(r), fill=fill)


def ribbon(d, x0, y0, x1, y1, fill, notch_base):
    """书签丝带：矩形 + 底部中央 V 形缺口。"""
    d.rectangle([px(x0), px(y0), px(x1), px(y1)], fill=fill)
    w = px(x1) - px(x0)
    depth = int(w * 0.62)
    cx = (px(x0) + px(x1)) // 2
    d.polygon(
        [(px(x0), px(y1) - depth + 1), (px(x1), px(y1) - depth + 1), (cx, px(y1))],
        fill=notch_base,
    )


def candidate_a():
    """深底 + 错位两张纸 + 一条填充色带（强调"批量"）。"""
    img = tile(INK)
    d = ImageDraw.Draw(img)
    rect(d, (0.31, 0.21, 0.77, 0.85), 0.035, PAPER_DIM)
    rect(d, (0.23, 0.15, 0.69, 0.79), 0.035, PAPER)
    rect(d, (0.29, 0.42, 0.63, 0.56), 0.02, BLUE)
    return img


def candidate_b():
    """深底 + 单页 + 半透明填充块 + 书签。"""
    img = tile(INK)
    d = ImageDraw.Draw(img)
    rect(d, (0.26, 0.15, 0.74, 0.85), 0.035, PAPER)
    rect(d, (0.31, 0.55, 0.69, 0.79), 0.02, BLUE_SOFT)
    ribbon(d, 0.585, 0.15, 0.665, 0.43, AMBER, PAPER)
    return img


def candidate_c(fill=BLUE_DEEP):
    """蓝底 + 单页 + 填充块 + 书签。"""
    img = tile(BLUE)
    d = ImageDraw.Draw(img)
    rect(d, (0.26, 0.15, 0.74, 0.85), 0.035, PAPER)
    rect(d, (0.31, 0.55, 0.69, 0.79), 0.02, fill)
    ribbon(d, 0.585, 0.15, 0.665, 0.43, AMBER, PAPER)
    return img


# —— 最终方案：蓝底 + 浅蓝填充块（品牌感强，填充块与底色区分清楚） ——
final = candidate_c(fill=BLUE_SOFT)
final.save(os.path.join(ROOT, "pdfbatch-1024.png"), format="PNG")
final.resize((256, 256), Image.Resampling.LANCZOS).save(os.path.join(ROOT, "pdfbatch-256.png"), format="PNG")
final.save(
    os.path.join(ROOT, "pdfbatch.ico"),
    format="ICO",
    sizes=[(16, 16), (24, 24), (32, 32), (48, 48), (64, 64), (128, 128), (256, 256)],
)

# —— 候选对比图（换方案时先看这里） ——
names = [("a", candidate_a()), ("b", candidate_b()), ("c", candidate_c())]
for key, img in names:
    img.resize((256, 256), Image.Resampling.LANCZOS).save(os.path.join(OUT, f"{key}-256.png"))

try:
    font = ImageFont.truetype(FONT, 30)
except OSError:
    font = ImageFont.load_default()

cw, pad, gap = 256, 24, 28
W = pad * 2 + cw * 3 + gap * 2
H = pad + 300 + 24 + 96 + pad
sheet = Image.new("RGBA", (W, H), (255, 255, 255, 255))
sd = ImageDraw.Draw(sheet)
for i, (key, img) in enumerate(names):
    x = pad + i * (cw + gap)
    big = img.resize((256, 256), Image.Resampling.LANCZOS)
    sheet.paste(big, (x, pad + 34), big)
    small = img.resize((32, 32), Image.Resampling.LANCZOS).resize((96, 96), Image.Resampling.NEAREST)
    sheet.paste(small, (x + 80, pad + 300 + 24), small)
    sd.text((x + 8, pad + 4), f"{key.upper()}  256px / 实际 32px", font=font, fill=(40, 40, 45, 255))
sheet.convert("RGB").save(os.path.join(OUT, "preview.png"))
print("final   ->", ROOT)
print("preview ->", OUT)