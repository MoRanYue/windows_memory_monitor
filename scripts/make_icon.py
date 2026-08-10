"""由 assets/logo.png 生成 Windows EXE 图标 assets/icon.ico（多尺寸）。

仅依赖 Pillow（纯 Python，无原生依赖）。

用法：
    uv run --with pillow python scripts/make_icon.py
"""

import os

from PIL import Image

BASE_DIR = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
PNG_PATH = os.path.join(BASE_DIR, "assets", "logo.png")
ICO_PATH = os.path.join(BASE_DIR, "assets", "icon.ico")

# Windows 图标常用尺寸 (px)
SIZES = [16, 24, 32, 48, 64, 128, 256]

# ICO 内最大单帧尺寸（标准上限 256）
MAX_ICO_SIZE = 256


def main() -> None:
    if not os.path.exists(PNG_PATH):
        raise FileNotFoundError(f"未找到源文件: {PNG_PATH}")

    with Image.open(PNG_PATH) as src:
        base = src.convert("RGBA")

        # 若源图非正方形，居中裁剪为正方形
        side = min(base.size)
        if base.size != (side, side):
            left = (base.width - side) // 2
            top = (base.height - side) // 2
            base = base.crop((left, top, left + side, top + side))

        # 缩放到 ICO 最大帧尺寸，再由 Pillow 高质量降采样到各尺寸
        if max(base.size) != MAX_ICO_SIZE:
            base = base.resize((MAX_ICO_SIZE, MAX_ICO_SIZE), Image.Resampling.LANCZOS)

        base.save(ICO_PATH, format="ICO", sizes=[(s, s) for s in SIZES])

    with Image.open(ICO_PATH) as ico:
        print(f"已生成: {ICO_PATH}")
        print(f"包含尺寸: {sorted(s for s in ico.ico.sizes())}")
        print(f"文件大小: {os.path.getsize(ICO_PATH)} bytes")


if __name__ == "__main__":
    main()
