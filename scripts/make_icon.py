# -*- coding: utf-8 -*-
# 生成应用图标源图（1024x1024）：深色圆角方块 + 终端提示符 ">_"
from PIL import Image, ImageDraw, ImageFont
import os

SIZE = 1024
img = Image.new("RGBA", (SIZE, SIZE), (0, 0, 0, 0))
draw = ImageDraw.Draw(img)

# 圆角方块背景（深蓝灰）
bg = (24, 26, 38, 255)
radius = 180
draw.rounded_rectangle([16, 16, SIZE - 16, SIZE - 16], radius=radius, fill=bg)

# 顶部三个"窗口圆点"
dot_y = 120
for i, color in enumerate([(255, 95, 86, 255), (255, 189, 46, 255), (39, 201, 63, 255)]):
    cx = 150 + i * 110
    draw.ellipse([cx - 28, dot_y - 28, cx + 28, dot_y + 28], fill=color)

# 终端提示符 ">_"
try:
    font = ImageFont.truetype("C:/Windows/Fonts/consola.ttf", 420)
except OSError:
    font = ImageFont.load_default()
draw.text((200, 380), ">_", fill=(0, 229, 160, 255), font=font)

out = os.path.join(os.path.dirname(__file__), "..", "src-tauri", "icons")
os.makedirs(out, exist_ok=True)
img.save(os.path.join(out, "icon-source.png"))
print("图标已生成:", os.path.abspath(os.path.join(out, "icon-source.png")))
