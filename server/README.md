# 服务端

单文件 `aiohttp` 服务，实现：

| 端点 | 作用 |
|---|---|
| `GET/POST /xiaozhi/ota/` | OTA 引导：告诉设备 WebSocket 地址 |
| `GET /xiaozhi/v1/` | WebSocket 对话/事件通道 |
| `GET/POST /api/progress` | 阅读进度读写 |
| `POST /api/display/book` | 登记书目结构（每章页数） |
| `POST /api/display/pages` | 上传页位图（1bpp base64） |
| `GET /api/display/status` | 查询缓存状态 |
| `POST /api/display/clear` | 清空某书页缓存 |
| `POST /api/display/standby` | 上传待机页位图（书名+操作说明，与书页同格式） |
| `GET /healthz` | 健康检查 |

协议细节见 [`../docs/protocol.md`](../docs/protocol.md)。

## 运行

```bash
pip install -r requirements.txt
python3 app.py --port 12900 --public-host <公网IP或域名>
```

`--public-host` 决定 OTA 响应里下发给设备的 `websocket.url`，必须是设备能访问到的地址。也可用环境变量 `PUBLIC_HOST`。

数据目录（自动创建）：`data/progress.json`、`data/page_cache/<book-hash>/`、`logs/YYYY-MM-DD.jsonl`。

## 部署（示例）

- `deploy/deploy.sh.example`：本机 rsync 到服务器并重启
- `deploy/restart.sh.example`：服务器上的重启脚本（含 pkill 防自匹配写法）
- `deploy/note4-server.service`：systemd 单元示例

## 推页示例

```bash
pip install pillow
python3 tools/push_demo.py --base http://<服务器>:12900 \
    --book demo --png page0.png --png page1.png
```

把 400×300 的灰度/彩色 PNG 转成 1bpp 位图（阈值 128）上传，并登记为一本单章书。之后设备按 ↓ 键即可翻页。

## 说明

- 对话是**回声占位**：`handle_user_utterance()` 返回固定文本。接 ASR/LLM/TTS 时替换这一个函数即可（音频帧已在 WS 二进制通道里，但当前未解码）。
- 无鉴权、明文传输。公网部署请套反向代理并加认证。