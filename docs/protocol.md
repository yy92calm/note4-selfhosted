# 扩展协议

在标准小智 WebSocket 协议之上，本方案增加了两类消息与一组 HTTP API。所有消息都是 UTF-8 JSON 文本帧；设备 → 服务端的音频仍是二进制帧（Opus）。

## 1. 连接生命周期

设备**不常连**：对话（按住确认键）或翻页按键时才建立 WS，空闲后由设备自行断开。因此服务端不能假设连接长期存在，任何「需要推给设备」的状态都应落盘，在下次 hello 后补发。

```
设备                                  服务端
 │── WS connect /xiaozhi/v1/ ─────────▶│  (Header: Device-Id, Client-Id, ...)
 │── {"type":"hello", ...} ───────────▶│
 │◀────────────── {"type":"hello"} ────│
 │── {"device_event":"standby_start"} ▶│  ← 开机后拉待机页
 │◀────────── {"type":"standby_image"} │
 │── {"device_event":"read_start"} ───▶│  ← 短按确认键开始阅读
 │◀────────── {"type":"display_image"} │
 │── {"type":"device_event", ...} ────▶│  ← page_turn ±1
 │◀────────── {"type":"display_image"} │  ← 折算后推对应页
 │◀──────────── {"type":"tts", ...} ───│  ← 页缓存未命中时用文字提示
```

## 2. `device_event`（设备 → 服务端）

设备侧动作上报。目前定义了翻页：

```json
{"session_id":"...","type":"device_event","name":"page_turn","value":1,"text":"down"}
```

- `value`：相对页偏移（`+1` 下一页，`-1` 上一页）
- `text`：`"down"` / `"up"`，仅用于日志可读性

服务端把它折算成**绝对进度** `(book, chapter, page)` 并持久化。折算依赖 `/api/display/book` 登记的每章页数，因此可以跨章翻页：

- `page + value` 超出本章末页 → 进位到下一章第 0 页
- `page + value` 小于 0 → 退到上一章末页

## 2.1 `read_start` / `standby_start`（设备 → 服务端）

```json
{"type":"device_event","name":"read_start","value":0}
{"type":"device_event","name":"standby_start","value":0}
```

- `read_start`：短按确认键「开始阅读」。服务端回推当前阅读页（`display_image`）；无书/无缓存时回 `tts` 文字提示。
- `standby_start`：设备开机后拉取待机页（书名+操作说明）。服务端回 `standby_image`。

## 3. `display_image`（服务端 → 设备）

整页墨水屏位图。设备收到后进入「书页模式」：暂停 LVGL 聊天 UI 的刷新（避免状态栏覆盖书页），写 framebuffer 并触发电子墨水屏刷新；直到收到新的聊天文本（`tts sentence_start` / `stt`）才退出书页模式、恢复聊天 UI。

```json
{
  "type": "display_image",
  "width": 400,
  "height": 300,
  "chapter": 3,
  "page": 12,
  "title": "示例书",
  "data": "<base64>"
}
```

`data` 解码后为裸位图，格式：

| 项目 | 约定 |
|---|---|
| 尺寸 | `width × height` = 400×300 |
| 位深 | 1bpp（每字节 8 像素，行首对齐到字节） |
| 行序 | 行优先，从上到下 |
| 位序 | 每字节 MSB 在左 |
| 极性 | **bit=1 为黑**，bit=0 为白 |
| 大小 | 400×300/8 = **15000 字节** |

> 该格式与固件 `CustomLcdDisplay::WriteRaw1bpp()` 的入参一致，可直接 `memcpy` 进 1bpp framebuffer。

## 3.1 `standby_image`（服务端 → 设备）

待机页（书名 + 操作说明），与 `display_image` **完全同格式**（400×300 1bpp base64），只是不带 chapter/page：

```json
{"type": "standby_image", "width": 400, "height": 300, "data": "<base64>"}
```

固件收到后缓存到内存并立即显示；**设备关机前会重绘这张图**（墨水屏断电后画面保留），所以息屏画面就是书名待机页。开机时设备自动进入「待机模式」：LVGL 聊天 UI 不刷屏，直到真正收到对话消息（`stt`/`tts sentence_start`）才退出。

## 4. HTTP API

### 4.1 进度

```http
GET  /api/progress              → [{"book": "...", "chapter": 1, "page": 12}, ...]
POST /api/progress              ← {"book":"...", "chapter":1, "page":12}
```

`chapter` / `page` 均为 0 基。POST 同时把该书设为「活跃书」——`page_turn` 事件作用在活跃书上。数组结构便于一个部署服务多本书。

### 4.2 页缓存

```http
POST /api/display/book          ← {"book":"...", "chapters":[30, 42, 18, ...]}
POST /api/display/pages         ← {"book":"...", "chapter":3,
                                   "pages":[{"page":0,"data":"<base64>"}, ...]}
GET  /api/display/status?book=  → {"book":"...", "chapters":[...], "cached_pages":N}
POST /api/display/clear         ← {"book":"..."}   # 清空该书页缓存
POST /api/display/standby       ← {"book":"...", "data":"<base64>"}  # 待机页位图
```

- `book` 是调用方自定义的稳定标识（建议用书的绝对路径或 ISBN），所有接口以此关联。
- `chapters` 是每章页数数组，服务端用它做跨章折算；重新登记会覆盖旧值。
- `pages[].data` 是 15000 字节裸位图的 base64；单次请求建议 ≤ 20 页（约 400KB）。

## 5. 与标准小智协议的差异

| 消息 | 状态 |
|---|---|
| `hello` / `listen` / `abort` / `tts` / `stt` / `llm` / `mcp` / `alert` / `system` | 沿用上游实现 |
| `device_event` | 本方案新增（设备 → 服务端） |
| `display_image` | 本方案新增（服务端 → 设备） |
| 音频编解码 | 服务端**未实现**：只计数不解码，回复文本为占位 |