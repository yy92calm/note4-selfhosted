# NOTE4 自托管方案（服务端 + 固件）

把 ZECTRIX NOTE4（ESP32-S3 + 4.2" 400×300 黑白墨水屏）从厂商云解放出来的最小可行方案：

- **服务端**：单文件 `aiohttp` 服务，实现小智（xiaozhi）协议的 OTA 引导 + WebSocket 对话通道，外加阅读进度与整书页缓存 API。不依赖任何厂商云。
- **固件**：基于 [cattei/xiaozhi-zectrix](https://github.com/cattei/xiaozhi-zectrix)（MIT）的 fork，补齐 ZECTRIX NOTE4 板卡接线，新增 `display_image` 阅读页显示通路与 `device_event` 按键上报。只写 `ota_1` 槽位，**不动 `ota_0` 官方固件**，可随时切回。

## 它现在能做什么

| 能力 | 说明 |
|---|---|
| 待机页 | 开机默认界面与关机画面均为书名 + 操作说明（阅读器渲染的位图，与书页同通路下发） |
| 短按确认键 | 开始阅读：拉取并显示当前阅读页 |
| 按住说话 | 确认键（GPIO0）按住 400ms 开麦、松手结束，音频 Opus 帧经 WS 上送服务端（服务端目前不解码，回复为占位文本） |
| 按键翻页看书 | 设备 ↓/↑ 键 → WS `device_event` → 服务端折算进度 → 同一条连接下发 400×300 1bpp 页位图 → 墨水屏整页刷新 |
| 进度记忆 | 服务端持久化 (书, 章, 页)，跨章翻页自动折算；任意主机工具可通过 HTTP API 读写 |
| 双键回退官方固件 | 开机时同时按住 ↑ + ↓ 约 0.5s，切回 `ota_0`（厂商固件） |

语音对话目前是**空心管线**：设备能发音频、服务端能回文字，但 ASR→LLM→TTS 未接入。参见 `server/app.py` 的 `handle_user_utterance()`。

## 架构

```
┌──────────┐   OTA 引导(HTTP)    ┌──────────────────────┐
│  NOTE4   │ ──────────────────▶ │      服务端 (aiohttp) │
│ 固件(ota_1)│  WS hello/listen    │  /xiaozhi/ota/       │
│          │ ◀─────────────────▶ │  /xiaozhi/v1/        │
│  按键 ↓  │  page_turn(JSON)    │   ├ device_event 折算 │
│  墨水屏  │ ◀────────────────── │   ├ display_image 推页│
└──────────┘  display_image(JSON) │  /api/progress       │
                                  │  /api/display/*      │
┌──────────┐      HTTP 上传       │                      │
│ 主机工具  │ ──────────────────▶ │  页缓存(1bpp 位图)    │
└──────────┘                     └──────────────────────┘
```

单条 WS 连接承担三件事：对话、按键事件、阅读页下发。设备平时不保持连接，**按键时按需建立会话**（约 300ms 完成握手）。

## 目录

```
reader/                     # 主机侧阅读器（Rust）：TXT → 排版 → 1bpp 位图 → 上传服务端
server/                     # 自托管服务端（单文件 aiohttp）
  app.py
  deploy/                   # systemd / 部署脚本示例
  tools/push_demo.py        # 把 PNG 转 1bpp 并上传的最小示例
firmware/                   # 固件（cattei/xiaozhi-zectrix fork，含 NOTE4 板卡）
  FORK.md                   # 相对上游的改动清单 + 构建/刷写说明
assets/fonts/               # 阅读器排版字体（Noto Sans SC，OFL）
scripts/read.sh             # 阅读器快捷脚本
docs/
  protocol.md               # 扩展协议：device_event / display_image / standby_image / API
  firmware-changes.md       # 改动细节（中文版）
```

三块的分工：**阅读器**负责排版渲染（书在电脑上，服务器不解析 TXT）；**服务端**只存位图与进度并负责下发；**固件**只负责收图刷屏与按键。

## 快速开始

1. **部署服务端**（任意有公网 IP 的 Linux 主机，或内网 + 端口映射）：

   ```bash
   pip install -r server/requirements.txt
   python3 server/app.py --port 12900 --public-host <你的公网IP或域名>
   ```

2. **上传书籍**（在电脑上）：

   ```bash
   cd reader
   cargo run --release -- ../books/示例书.txt --serve --sync http://<你的服务器>:12900
   ```

   阅读器会完成切章、排版、整书渲染（每页 400×300 1bpp），并把页面与待机页上传服务端；重复执行时缓存命中会跳过重传。

3. **构建固件**：修改 `firmware/sdkconfig.zectrix` 里的 `CONFIG_OTA_URL` 指向你的服务器，按 `firmware/FORK.md` 编译。

4. **刷写**：只写 `ota_1` 分区（见 `firmware/FORK.md` 的刷写与回退章节）。

5. **阅读**：设备开机显示待机页 → 短按确认键出当前页 → ↑/↓ 翻页；按住确认键说话。进度存在服务端，换设备/重装都能续读。

## 安全提示

- 服务端 **没有鉴权**：对话、进度、页缓存接口都是裸奔的。请只在内网使用，或套反向代理 + 认证后再暴露公网。
- 设备与服务器之间是**明文 HTTP/WS**，不含任何厂商 API Key。请勿在仓库中提交自己的部署地址、私钥或设备标识。
- 与厂商云（极趣云）无关：本方案不调用、也不依赖厂商的任何接口。

## 许可

固件 fork 自 MIT 协议的 [cattei/xiaozhi-zectrix](https://github.com/cattei/xiaozhi-zectrix)，见 `LICENSE`。服务端为本仓库原创，同 MIT。