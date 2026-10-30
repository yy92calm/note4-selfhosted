# NOTE4 墨水屏小说阅读器（自托管）

把 **ZECTRIX NOTE4**（ESP32-S3 + 4.2 寸 400×300 黑白墨水屏）改造成一台**自托管小说阅读器**：书在电脑上排版渲染，设备通过 Wi-Fi 从你自己的服务器取每一页，只管按键翻页；不依赖任何厂商云。

> 注意：它**不是离线阅读器**——设备不存书，每页都要联网从服务器拉取；离线时只能保留断电前那一屏。

> 固件基于开源语音助手固件 [xiaozhi-zectrix](https://github.com/cattei/xiaozhi-zectrix)（MIT）改造，沿用它的 OTA / WebSocket 连接与显示驱动。**没有接入 AI 对话**：ASR / LLM / TTS 均未实现，语音相关代码只是占位。

## 功能

- **整书阅读**：TXT 自动切章、排版分页（避头尾、段首缩进、页眉页码+全书百分比），每页渲染为 400×300 1bpp 位图
- **设备按键翻页**：↑ 上一页、↓ 下一页，跨章自动衔接
- **待机页**：开机默认界面与关机画面均为「书名 + 操作说明」，墨水屏断电后画面保留
- **进度记忆**：设备按键 → 服务端折算 (书, 章, 页) 并持久化；重新推书或换电脑都能续读
- **完全自托管**：OTA 引导、WebSocket 通道、页面缓存全部跑在你自己的服务器上，无第三方 API

## 架构

```
┌────────────┐  排版渲染（电脑）    ┌──────────────────────┐
│  reader/   │ ─── HTTP 上传 ────▶ │  server/（自托管）     │
│  Rust CLI  │   400×300 1bpp 位图  │  页缓存 + 进度         │
└────────────┘                     │  OTA 引导 + WS 通道    │
                                   └──────────┬───────────┘
                                              │ WS 下发位图 / 按键事件
                                   ┌──────────▼───────────┐
                                   │  NOTE4 固件（ota_1）  │
                                   │  按键 → page_turn     │
                                   │  收图 → 墨水屏刷新     │
                                   └──────────────────────┘
```

三块分工：

| 组件 | 职责 | 跑在哪 |
|---|---|---|
| `reader/` | TXT → 切章 → 排版 → 1bpp 位图 → 上传 | 你的电脑 |
| `server/` | 存页面/待机页/进度，设备连接时下发 | 你的服务器（单文件 aiohttp） |
| `firmware/` | 收图刷屏、按键上报、待机页缓存与重绘 | 设备（只写 `ota_1`，不动 `ota_0`） |

服务器**不解析 TXT**，只存渲染后的位图（约 15KB/页），也不做任何自然语言处理。

## 设备操作

| 按键 | 动作 |
|---|---|
| 确认键 短按 | 开始阅读（从服务器拉取当前页） |
| ↑ / ↓ | 上一页 / 下一页 |
| 确认键 按住 | 预留（语音未接入，仅回占位文本） |
| ↑ + ↓ 开机按住 0.5s | 切回 `ota_0` 厂商固件 |

## 快速开始

### 1. 部署服务端

任意有公网地址的 Linux 主机（或内网 + 端口映射）：

```bash
pip install -r server/requirements.txt
python3 server/app.py --port 12900 --public-host <你的公网IP或域名>
```

### 2. 上传书籍（电脑上）

```bash
cd reader
cargo run --release -- ../books/示例书.txt --serve --sync http://<你的服务器>:12900
```

阅读器会完成切章、排版、整书渲染（每页 400×300 1bpp），上传页面与待机页；重复执行时若服务端缓存已是最新会跳过重传。也可用环境变量 `NOTE4_SERVER` 代替 `--sync`。

### 3. 构建并刷写固件

1. 修改 `firmware/sdkconfig.zectrix` 中的 `CONFIG_OTA_URL` 指向你的服务器
2. 按 [`firmware/FORK.md`](firmware/FORK.md) 编译
3. **先备份整片 Flash**，再只写 `ota_1` 分区（`0x610000`）——`ota_0` 厂商固件保持不动

### 4. 阅读

设备开机显示待机页 → 短按确认键出当前页 → ↑/↓ 翻页。

## 目录

```
reader/                 # 主机侧阅读器（Rust）
  src/{book,layout,render,progress,sync,main}.rs
server/                 # 自托管服务端（单文件 aiohttp）
  app.py
  deploy/               # systemd / rsync / restart 示例
  tools/push_demo.py    # PNG → 1bpp → 上传的最小示例
firmware/               # 固件（xiaozhi-zectrix fork，含 NOTE4 板卡）
  FORK.md               # 相对上游的改动 + 构建/刷写说明
assets/fonts/           # 阅读器排版字体（Noto Sans SC，OFL）
scripts/read.sh         # 阅读器快捷脚本
docs/
  protocol.md           # 扩展协议：display_image / standby_image / 按键事件 / HTTP API
  firmware-changes.md   # 固件改动清单
```

## 已知限制

- **无 AI 对话**：语音链路只保留占位回复，ASR / LLM / TTS 均未接入
- 服务端无鉴权、明文 HTTP/WS：仅建议内网使用，公网请套反向代理并加认证
- 阅读器目前只支持纯文本 TXT（UTF-8）
- 排版参数（字号、行距）在阅读器侧，改动后需重新上传书籍

## 许可

- 固件 fork 自 [cattei/xiaozhi-zectrix](https://github.com/cattei/xiaozhi-zectrix)，MIT，见 `LICENSE`
- 本项目其余代码同 MIT；字体 Noto Sans SC 采用 SIL Open Font License 1.1（见 `assets/fonts/OFL.txt`）