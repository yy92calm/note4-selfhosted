#!/usr/bin/env python3
"""
NOTE4 自研服务端（小智协议精简实现）

作用：
  1. GET /xiaozhi/ota/   —— 设备开机问「我该连哪台服务器」，我们直接告诉它本机 WS 地址
  2. WS  /xiaozhi/v1/    —— 对话通道：收 hello / listen / 音频帧，回 stt + tts 文本（文本会自动上屏）
  3. GET/POST /api/progress —— 阅读进度（书/章/页），给主机端阅读器用
  4. POST /api/display/* —— 阅读器上传整书页位图（1bpp），设备按键翻页时经 WS 推送显示

设计原则：不依赖任何厂商云，跑在你自己的一台服务器上；对话内容全量落日志，
后续接 ASR / LLM / TTS 只需替换 handle_user_utterance()。

启动：
    python3 app.py --port 12900
"""
import argparse
import asyncio
import base64
import glob
import hashlib
import json
import logging
import os
import time
import uuid
from datetime import datetime, timezone, timedelta

from aiohttp import web, WSMsgType

BASE_DIR = os.path.dirname(os.path.abspath(__file__))
LOG_DIR = os.path.join(BASE_DIR, "logs")
DATA_DIR = os.path.join(BASE_DIR, "data")
PAGE_CACHE_DIR = os.path.join(DATA_DIR, "page_cache")
os.makedirs(LOG_DIR, exist_ok=True)
os.makedirs(DATA_DIR, exist_ok=True)

PROGRESS_FILE = os.path.join(DATA_DIR, "progress.json")

DISPLAY_WIDTH = 400
DISPLAY_HEIGHT = 300

logging.basicConfig(
    level=logging.INFO,
    format="%(asctime)s %(levelname)-7s %(message)s",
    datefmt="%H:%M:%S",
)
log = logging.getLogger("note4")

CST = timezone(timedelta(hours=8))


def ts() -> str:
    return datetime.now(CST).strftime("%Y-%m-%d %H:%M:%S")


def event_log(name: str, payload: dict) -> None:
    """事件流落盘，一天一个文件，方便事后查「哪天说了什么」"""
    path = os.path.join(LOG_DIR, f"{datetime.now(CST):%Y-%m-%d}.jsonl")
    with open(path, "a", encoding="utf-8") as f:
        f.write(json.dumps({"ts": ts(), "event": name, **payload}, ensure_ascii=False) + "\n")


# ---------------------------------------------------------------- OTA

async def handle_ota(request: web.Request) -> web.Response:
    """设备 OTA 检查：返回服务器地址 + 时间。没有 activation 字段 = 不需要激活流程。"""
    device_id = request.headers.get("Device-Id", "")
    client_id = request.headers.get("Client-Id", "")
    ua = request.headers.get("User-Agent", "")
    body = await request.read()

    log.info("OTA 请求 device=%s client=%s ua=%s body=%s", device_id, client_id, ua,
             body[:200].decode("utf-8", "replace"))
    event_log("ota", {"device_id": device_id, "user_agent": ua})

    payload = {
        # 版本号和固件里一致 → 不会触发升级
        "firmware": {"version": "2.4.0"},
        "websocket": {
            "url": request.app["ws_url"],
            "token": "",
            "version": 1,
        },
        "server_time": {
            "timestamp": int(time.time() * 1000),
            "timezone_offset": 480,
        },
    }
    return web.json_response(payload)


async def handle_health(request: web.Request) -> web.Response:
    return web.json_response({"ok": True, "ts": ts(), "clients": len(request.app["clients"])})


# ---------------------------------------------------------------- 进度
#
# progress.json 与主机端阅读器（reader/src/progress.rs）同构：
#   [ {"book": 路径, "chapter": int, "page": int}, ... ]
# 读写这里的绝对状态；设备按键只发相对事件（page_turn ±1），由服务端折算。

def load_progress_list() -> list:
    if not os.path.exists(PROGRESS_FILE):
        return []
    try:
        with open(PROGRESS_FILE, encoding="utf-8") as f:
            data = json.load(f)
        return data if isinstance(data, list) else []
    except (json.JSONDecodeError, OSError) as e:
        log.warning("进度文件读取失败，按空处理: %s", e)
        return []


def save_progress_list(entries: list) -> None:
    with open(PROGRESS_FILE, "w", encoding="utf-8") as f:
        json.dump(entries, f, ensure_ascii=False, indent=2)


def upsert_progress(book: str, chapter: int, page: int) -> list:
    entries = load_progress_list()
    for slot in entries:
        if slot.get("book") == book:
            slot["chapter"], slot["page"] = chapter, page
            break
    else:
        entries.append({"book": book, "chapter": chapter, "page": page})
    save_progress_list(entries)
    return entries


async def handle_progress_get(request: web.Request) -> web.Response:
    return web.json_response(load_progress_list())


async def handle_progress_post(request: web.Request) -> web.Response:
    data = await request.json()
    book = data.get("book")
    if not book:
        return web.json_response({"error": "book required"}, status=400)
    chapter = int(data.get("chapter", 0))
    page = int(data.get("page", 0))
    entries = upsert_progress(book, chapter, page)
    request.app["active_book"] = book
    event_log("progress", {"book": book, "chapter": chapter, "page": page})
    log.info("进度更新: %s ch%d p%d（共 %d 本）", book, chapter, page, len(entries))
    return web.json_response({"ok": True, "progress": entries})


# ---------------------------------------------------------------- 页面缓存
#
# 阅读器把整书渲染成 400×300 1bpp 位图上传；设备按键翻页时按 (chapter, page)
# 取缓存经 WS 推给设备显示。目录按书名 SHA1 散列，meta.json 存每章页数。

def book_cache_dir(book: str) -> str:
    h = hashlib.sha1(book.encode("utf-8")).hexdigest()[:16]
    return os.path.join(PAGE_CACHE_DIR, h)


def load_book_meta(book: str) -> dict:
    path = os.path.join(book_cache_dir(book), "meta.json")
    if not os.path.exists(path):
        return {}
    try:
        with open(path, encoding="utf-8") as f:
            meta = json.load(f)
        return meta if isinstance(meta, dict) else {}
    except (json.JSONDecodeError, OSError) as e:
        log.warning("书目缓存读取失败: %s", e)
        return {}


def save_book_meta(book: str, meta: dict) -> None:
    os.makedirs(book_cache_dir(book), exist_ok=True)
    path = os.path.join(book_cache_dir(book), "meta.json")
    with open(path, "w", encoding="utf-8") as f:
        json.dump(meta, f, ensure_ascii=False)


def page_file(book: str, chapter: int, page: int) -> str:
    return os.path.join(book_cache_dir(book), f"ch{chapter}_p{page}.bin")


def book_title(book: str) -> str:
    """书名：取路径文件名去扩展名（设备待机页显示用）"""
    return os.path.splitext(os.path.basename(book))[0] or book


def fold_page_turn(entry: dict, value: int, chapters: list) -> None:
    """相对页码折算成绝对 (chapter, page)，知道每章页数即可跨章翻页。"""
    chapter = int(entry.get("chapter", 0))
    page = int(entry.get("page", 0)) + value
    if not chapters:
        entry["page"] = max(0, page)
        return
    while True:
        if chapter < 0:
            chapter, page = 0, 0
        elif chapter >= len(chapters):
            chapter = len(chapters) - 1
            page = max(0, chapters[chapter] - 1)
        elif page >= chapters[chapter]:
            chapter, page = chapter + 1, 0
            continue
        elif page < 0:
            if chapter == 0:
                page = 0
            else:
                chapter -= 1
                page = chapters[chapter] - 1
                continue
        break
    entry["chapter"], entry["page"] = chapter, page


async def push_book_page(ws, book: str, chapter: int, page: int) -> bool:
    """把缓存中的一页位图经 WS 推给设备。缓存未命中返回 False。"""
    path = page_file(book, chapter, page)
    if not os.path.exists(path):
        return False
    with open(path, "rb") as f:
        raw = f.read()
    await send_json(ws, {
        "type": "display_image",
        "width": DISPLAY_WIDTH,
        "height": DISPLAY_HEIGHT,
        "chapter": chapter,
        "page": page,
        "title": book_title(book),
        "data": base64.b64encode(raw).decode("ascii"),
    })
    return True


def standby_file(book: str) -> str:
    return os.path.join(book_cache_dir(book), "standby.bin")


async def send_standby(ws, book: str) -> bool:
    """下发待机页位图（书名+操作说明，阅读器渲染）。未上传过返回 False。"""
    path = standby_file(book)
    if not os.path.exists(path):
        return False
    with open(path, "rb") as f:
        raw = f.read()
    await send_json(ws, {
        "type": "standby_image",
        "width": DISPLAY_WIDTH,
        "height": DISPLAY_HEIGHT,
        "data": base64.b64encode(raw).decode("ascii"),
    })
    return True


def resolve_active_book(request: web.Request) -> str | None:
    """解析当前活跃书：优先 app 记录的，其次进度列表最后一条。"""
    entries = load_progress_list()
    active = request.app.get("active_book")
    if active is None and entries:
        active = entries[-1].get("book")
        request.app["active_book"] = active
    return active


async def handle_display_book_post(request: web.Request) -> web.Response:
    """阅读器登记书目结构：{book, chapters:[每章页数]}"""
    data = await request.json()
    book = data.get("book")
    chapters = data.get("chapters")
    if not book or not isinstance(chapters, list) or not chapters:
        return web.json_response({"error": "book and chapters required"}, status=400)
    save_book_meta(book, {"chapters": [int(c) for c in chapters], "updated": ts()})
    request.app["active_book"] = book
    event_log("display_book", {"book": book, "chapters": len(chapters)})
    log.info("书目登记: %s（%d 章，%d 页）", book, len(chapters), sum(int(c) for c in chapters))
    return web.json_response({"ok": True})


async def handle_display_pages_post(request: web.Request) -> web.Response:
    """阅读器上传某章的页位图：{book, chapter, pages:[{page, data_b64}]}"""
    data = await request.json()
    book = data.get("book")
    chapter = data.get("chapter")
    pages = data.get("pages") or []
    if not book or chapter is None or not pages:
        return web.json_response({"error": "book/chapter/pages required"}, status=400)
    chapter = int(chapter)
    max_bytes = DISPLAY_WIDTH * DISPLAY_HEIGHT // 8
    os.makedirs(book_cache_dir(book), exist_ok=True)
    stored = 0
    for p in pages:
        try:
            raw = base64.b64decode(p.get("data") or "")
        except (ValueError, TypeError):
            return web.json_response({"error": "invalid base64"}, status=400)
        if not 0 < len(raw) <= max_bytes:
            return web.json_response(
                {"error": f"page {p.get('page')} bad size {len(raw)}"}, status=400)
        with open(page_file(book, chapter, int(p.get("page", 0))), "wb") as f:
            f.write(raw)
        stored += 1
    event_log("display_pages", {"book": book, "chapter": chapter, "count": stored})
    log.info("页面上传: %s ch%d +%d 页", book, chapter, stored)
    return web.json_response({"ok": True, "stored": stored})


async def handle_display_standby_post(request: web.Request) -> web.Response:
    """阅读器上传待机页位图：{book, data_b64}（400×300 1bpp，与书页同格式）"""
    data = await request.json()
    book = data.get("book")
    try:
        raw = base64.b64decode(data.get("data") or "")
    except (ValueError, TypeError):
        return web.json_response({"error": "invalid base64"}, status=400)
    max_bytes = DISPLAY_WIDTH * DISPLAY_HEIGHT // 8
    if not book or not 0 < len(raw) <= max_bytes:
        return web.json_response({"error": "book/data required"}, status=400)
    os.makedirs(book_cache_dir(book), exist_ok=True)
    with open(standby_file(book), "wb") as f:
        f.write(raw)
    event_log("display_standby", {"book": book, "bytes": len(raw)})
    log.info("待机页上传: %s（%d 字节）", book, len(raw))
    return web.json_response({"ok": True})


async def handle_display_status_get(request: web.Request) -> web.Response:
    """阅读器查询某书的缓存状态，决定还需要上传哪些章。"""
    book = request.query.get("book", "")
    if not book:
        return web.json_response({"error": "book required"}, status=400)
    meta = load_book_meta(book)
    d = book_cache_dir(book)
    cached = len(glob.glob(os.path.join(d, "ch*_p*.bin")))
    return web.json_response({"book": book, "chapters": meta.get("chapters", []),
                              "cached_pages": cached})


async def handle_display_clear_post(request: web.Request) -> web.Response:
    """清空某书的页面缓存（换书/回收磁盘用）。"""
    data = await request.json()
    book = data.get("book")
    if not book:
        return web.json_response({"error": "book required"}, status=400)
    d = book_cache_dir(book)
    removed = 0
    if os.path.isdir(d):
        for path in glob.glob(os.path.join(d, "ch*_p*.bin")):
            os.remove(path)
            removed += 1
    log.info("页面缓存清空: %s（%d 页）", book, removed)
    return web.json_response({"ok": True, "removed": removed})


# ---------------------------------------------------------------- 对话

async def send_json(ws, obj: dict) -> None:
    text = json.dumps(obj, ensure_ascii=False)
    log.info("→ %s", text[:300])
    await ws.send_str(text)


def handle_user_utterance(text: str) -> str:
    """把用户的输入变成回复。目前是回声；后续接 ASR/LLM 换这里。"""
    text = text.strip()
    if not text:
        return "在的，你说。"
    return f"收到：{text}"


async def reply_tts(ws, text: str) -> None:
    """走 TTS 文本通道：sentence_start 里的 text 会直接显示在墨水屏上。"""
    await send_json(ws, {"type": "tts", "state": "start", "session_id": ws["session_id"]})
    await send_json(ws, {
        "type": "tts",
        "state": "sentence_start",
        "text": text,
        "session_id": ws["session_id"],
    })
    await send_json(ws, {"type": "tts", "state": "stop", "session_id": ws["session_id"]})


async def handle_read_start(request: web.Request, ws) -> None:
    """设备短按确认键「开始阅读」：回推当前页；没书/没缓存时给文字提示。"""
    entries = load_progress_list()
    active = resolve_active_book(request)
    entry = next((e for e in entries if e.get("book") == active), None)
    if entry is None:
        log.info("read_start：服务端没有任何书籍进度")
        await reply_tts(ws, "还没有装载书籍，请先在电脑端推书。")
        return
    if await push_book_page(ws, entry["book"], entry["chapter"], entry["page"]):
        log.info("read_start → 推送 %s ch%s p%s", entry["book"],
                 entry["chapter"], entry["page"])
    else:
        await reply_tts(ws, "这一页还没有装载，请在电脑端运行 read.sh 同步整书。")


async def handle_device_event(request: web.Request, ws, data: dict) -> None:
    """设备侧事件（按键翻页等）：相对事件折算成绝对进度并持久化，再把对应页位图推回设备。"""
    name = data.get("name") or ""
    value = data.get("value", 0)
    text = data.get("text") or ""
    event_log("device_event", {"device_id": ws["device_id"], "name": name,
                               "value": value, "text": text})

    if name == "read_start":
        await handle_read_start(request, ws)
        return

    if name == "standby_start":
        active = resolve_active_book(request)
        if active and await send_standby(ws, active):
            log.info("standby_start → 下发待机页（%s）", active)
        return

    if name != "page_turn":
        log.info("device_event（仅记录）: name=%s value=%s text=%s", name, value, text)
        return

    entries = load_progress_list()
    active = request.app.get("active_book")
    entry = next((e for e in entries if e.get("book") == active), None)
    if entry is None and entries:
        entry = entries[-1]
        active = entry.get("book")
        request.app["active_book"] = active
        log.info("无活跃书，回退到最近一条: %s", active)
    if entry is None:
        log.warning("page_turn 丢弃：服务端没有任何书籍进度")
        return

    meta = load_book_meta(active)
    fold_page_turn(entry, int(value), meta.get("chapters", []))
    save_progress_list(entries)
    log.info("page_turn %+d → %s ch%s p%s", value, active,
             entry.get("chapter"), entry.get("page"))
    event_log("page_folded", {"book": active, "chapter": entry.get("chapter"),
                              "page": entry.get("page")})

    # 同一条 WS 连接把新页位图推回设备显示
    if await push_book_page(ws, active, entry["chapter"], entry["page"]):
        log.info("已推送书页 ch%s p%s", entry["chapter"], entry["page"])
    else:
        await reply_tts(ws, "这一页还没有装载，请在电脑端运行 read.sh 同步整书。")


async def handle_ws(request: web.Request) -> web.WebSocketResponse:
    ws = web.WebSocketResponse(heartbeat=30, max_msg_size=0)
    await ws.prepare(request)

    device_id = request.headers.get("Device-Id", "unknown")
    ws["session_id"] = str(uuid.uuid4())
    ws["device_id"] = device_id
    request.app["clients"].add(ws)
    log.info("WS 连接建立 device=%s session=%s", device_id, ws["session_id"])
    event_log("ws_open", {"device_id": device_id, "session_id": ws["session_id"]})

    audio_frames = 0
    async for msg in ws:
        if msg.type == WSMsgType.TEXT:
            try:
                data = json.loads(msg.data)
            except json.JSONDecodeError:
                log.warning("非法 JSON: %s", msg.data[:200])
                continue

            mtype = data.get("type")
            log.info("← %s", msg.data[:400])
            event_log("in_json", {"device_id": device_id, "data": data})

            if mtype == "hello":
                audio = data.get("audio_params") or {}
                log.info("客户端 hello: version=%s audio=%s features=%s",
                         data.get("version"), audio, data.get("features"))
                await send_json(ws, {
                    "type": "hello",
                    "transport": "websocket",
                    "session_id": ws["session_id"],
                    "audio_params": {
                        "format": "opus",
                        "sample_rate": audio.get("sample_rate", 16000),
                        "channels": 1,
                        "frame_duration": audio.get("frame_duration", 60),
                    },
                })
            elif mtype == "listen":
                state = data.get("state")
                if state == "start":
                    log.info("开始听（mode=%s）", data.get("mode"))
                elif state == "stop":
                    log.info("停止听，收到音频帧 %d 个（%.1f KB）", audio_frames, audio_frames * 0.06)
                    text = data.get("text") or ""
                    reply = handle_user_utterance(text)
                    await reply_tts(ws, reply)
            elif mtype == "abort":
                log.info("客户端中止")
            elif mtype == "device_event":
                await handle_device_event(request, ws, data)
            elif mtype == "mcp":
                # 设备侧 MCP 消息：先原样记下来，后面做工具调用再用
                event_log("mcp", {"device_id": device_id, "payload": data.get("payload")})
        elif msg.type == WSMsgType.BINARY:
            audio_frames += 1
            if audio_frames == 1:
                log.info("收到第一帧音频（%d 字节）", len(msg.data))
        elif msg.type == WSMsgType.ERROR:
            log.error("WS 错误: %s", ws.exception())
            break

    request.app["clients"].discard(ws)
    log.info("WS 关闭 device=%s 音频帧=%d", device_id, audio_frames)
    event_log("ws_close", {"device_id": device_id, "audio_frames": audio_frames})
    return ws


# ---------------------------------------------------------------- 启动

def build_app(port: int, public_host: str) -> web.Application:
    app = web.Application(client_max_size=64 * 1024 * 1024)
    app["clients"] = set()
    app["ws_url"] = f"ws://{public_host}:{port}/xiaozhi/v1/"
    app["active_book"] = None
    app.router.add_get("/xiaozhi/ota/", handle_ota)
    app.router.add_post("/xiaozhi/ota/", handle_ota)
    app.router.add_get("/xiaozhi/v1/", handle_ws)
    app.router.add_get("/healthz", handle_health)
    app.router.add_get("/api/progress", handle_progress_get)
    app.router.add_post("/api/progress", handle_progress_post)
    app.router.add_post("/api/display/book", handle_display_book_post)
    app.router.add_post("/api/display/pages", handle_display_pages_post)
    app.router.add_get("/api/display/status", handle_display_status_get)
    app.router.add_post("/api/display/clear", handle_display_clear_post)
    app.router.add_post("/api/display/standby", handle_display_standby_post)
    return app


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--port", type=int, default=int(os.environ.get("PORT", 12900)))
    parser.add_argument("--host", default="0.0.0.0")
    parser.add_argument("--public-host", default=os.environ.get("PUBLIC_HOST", "127.0.0.1"),
                        help="设备要连的公网地址，用来拼 websocket.url")
    args = parser.parse_args()

    log.info("NOTE4 服务端启动 http://%s:%d  OTA=/xiaozhi/ota/  WS=/xiaozhi/v1/",
             args.host, args.port)
    web.run_app(build_app(args.port, args.public_host), host=args.host, port=args.port, print=None)


if __name__ == "__main__":
    main()
