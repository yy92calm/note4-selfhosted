#!/usr/bin/env python3
"""推页示例：PNG → 400×300 1bpp 位图 → 服务端页缓存。

用法：
    python3 tools/push_demo.py --base http://127.0.0.1:12900 --book demo \
        --png page0.png --png page1.png ...

会把所有 PNG（自动缩放到 400×300、按灰度阈值 128 二值化）登记为一本
单章书（book）的第 0..N-1 页，并把进度置为第 0 章第 0 页。
之后在设备上按 ↓ 键即可翻页阅读。

依赖：pillow
"""
import argparse
import base64
import json
import sys
import urllib.request

from PIL import Image

WIDTH, HEIGHT = 400, 300


def to_raw1bpp(path: str) -> bytes:
    img = Image.open(path).convert("L").resize((WIDTH, HEIGHT))
    px = img.load()
    bytes_per_row = (WIDTH + 7) // 8
    buf = bytearray(bytes_per_row * HEIGHT)
    for y in range(HEIGHT):
        for x in range(WIDTH):
            if px[x, y] < 128:
                buf[y * bytes_per_row + x // 8] |= 0x80 >> (x % 8)
    return bytes(buf)


def post(base: str, path: str, payload: dict):
    req = urllib.request.Request(
        base.rstrip("/") + path,
        data=json.dumps(payload).encode(),
        headers={"Content-Type": "application/json"},
        method="POST",
    )
    with urllib.request.urlopen(req, timeout=30) as resp:
        return json.loads(resp.read())


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--base", default="http://127.0.0.1:12900")
    ap.add_argument("--book", default="demo", help="书标识（自定，建议绝对路径或 ISBN）")
    ap.add_argument("--png", action="append", required=True, help="400×300 PNG，可重复")
    ap.add_argument("--standby", help="可选：待机页 PNG（书名+操作说明），关机/开机默认画面")
    args = ap.parse_args()

    pages = [(i, base64.b64encode(to_raw1bpp(p)).decode()) for i, p in enumerate(args.png)]
    print(f"上传 {len(pages)} 页 → {args.base} / book={args.book}")

    post(args.base, "/api/display/book", {"book": args.book, "chapters": [len(pages)]})
    # 每批 20 页，避免单请求过大
    for i in range(0, len(pages), 20):
        batch = pages[i:i + 20]
        post(args.base, "/api/display/pages", {
            "book": args.book,
            "chapter": 0,
            "pages": [{"page": p, "data": d} for p, d in batch],
        })
        print(f"  已上传第 {batch[0][0]}..{batch[-1][0]} 页")

    if args.standby:
        standby = base64.b64encode(to_raw1bpp(args.standby)).decode()
        post(args.base, "/api/display/standby", {"book": args.book, "data": standby})
        print("待机页已上传")

    post(args.base, "/api/progress", {"book": args.book, "chapter": 0, "page": 0})
    print("完成。现在在设备上短按确认键开始阅读，↑/↓ 翻页。")
    return 0


if __name__ == "__main__":
    sys.exit(main())