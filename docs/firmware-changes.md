# 固件改动清单（相对上游 `cattei/xiaozhi-zectrix` @ 177c032）

本 fork 的目标：让 NOTE4 在**不依赖厂商云**的前提下，跑通「按住说话 + 按键翻页看书 + 进度同步」，且只写 `ota_1` 槽位。

## 1. 板卡接入（新增）

上游仓库 `main/boards/zectrix/` 下有板卡目录但 NOTE4 4.2" 型号未接线（缺电源/屏幕/按键/I2C 会话锁/睡眠仲裁），本 fork 补齐：

| 文件 | 内容 |
|---|---|
| `main/boards/zectrix/zectrix-s3-epaper-4.2/` | 板卡实现：电源时序（EPD/Audio/VBAT 三路使能）、SPI 墨水屏驱动（SSD2683）、按键、RTC PCF8563、NFC、电量检测 |
| `main/boards/zectrix/zectrix-s3-epaper-4.2/sleep_manager.h` | 睡眠仲裁：屏刷新期间阻塞关机（原子 busy 位 + kick 超时），防止刷新中途掉电白屏 |
| `main/boards/zectrix/i2c_bus_lock.h` | I2C 会话锁接口（弱默认实现），板级强实现见 `i2c_power_hook.cc`；RTC/NFC 共用 I2C0 时避免总线互踩 |
| `main/boards/zectrix/zectrix-s3-epaper-4.2/i2c_power_hook.cc` | 强实现：总线锁（递归互斥）、强制上电、总线复位 `BoardI2cResetBus()` |
| `partitions/16m_zectrix.csv` | 16MB Flash 分区表：`ota_0`/`ota_1` 各 6080KB、`assets` 4096KB，与设备实测对齐 |
| `sdkconfig.zectrix` | 板级 Kconfig 覆盖（目标 esp32s3、板型、分区表、OTA 地址、中文界面） |

## 2. OTA 只写 ota_1

- 设备官方固件在 `ota_0`（0x20000），本固件固定写入 `ota_1`（0x610000），**任何流程都不写 `ota_0`**。
- 开机时同时按住 ↑ + ↓ 约 0.5s 可切回 `ota_0` 官方固件（上游 `CheckDualButtonForOtaSwitch` 行为）。
- **首次刷写前务必备份整片 Flash**（`esptool read_flash 0 0x1000000`），这是唯一的兜底。

## 3. 阅读页显示（本方案核心新增）

| 位置 | 改动 |
|---|---|
| `main/application.cc` | 新增 `display_image` 消息处理：JSON 里 base64 解码 1bpp 位图 → `Schedule` 到主任务 → `display->ShowBookPage()`。新增 `SendPageTurnEvent()`：通道未打开时先建会话（不进入听音状态），随后上报 `page_turn` |
| `main/application.h` | 声明 `SendPageTurnEvent()` |
| `main/display/display.h` | `Display` 基类新增虚方法 `ShowBookPage(const uint8_t*, size_t)` |
| `custom_lcd_display.h/.cc` | 实现 `ShowBookPage()`：写 framebuffer + 触发刷新；「书页模式」下屏蔽 LVGL 局部刷新（防聊天状态栏覆盖书页）；`SetChatMessage()` 收到新聊天文本时退出书页模式并整屏重绘 |
| `zectrix-s3-epaper-4.2.cc` | ↑/↓ 按键改为 `SendPageTurnEvent()`；启动阶段（`starting`）忽略翻页键，避免开机时误入配网模式 |
| `application.cc` + `custom_lcd_display.*` | **待机页**：开机自动拉取 `standby_image`（阅读器渲染的 1bpp 位图，与书页同格式），缓存到 PSRAM；关机前重绘该图，墨水屏断电后显示书名+操作说明。上电即进入待机模式，屏蔽 LVGL 聊天 UI 抢屏 |
| `application.cc` / `application.h` | **确认键双语义**：短按（<400ms）= `read_start` 开始阅读；按住（≥400ms）= 按住说话（PTT） |
| `application.cc` | 新增 `read_start` / `standby_start` 事件；`display_image` 携带书名（`title`）并存 NVS |
| `custom_lcd_display.cc` | 修复 `render_text_to_buffer` 对 4bpp 字体的错误解包（按 glyph `format` 取位深）与 `adv_w` 单位（1/16 像素） |
| `custom_lcd_display.cc` | `SetChatMessage("system", ...)` 不再退出书页/待机模式（启动状态类消息不抢屏），仅 user/assistant 对话退出 |

位图格式见 `docs/protocol.md`：400×300、1bpp、行优先、MSB 先行、bit=1 黑、15000 字节。

## 4. 其他修复

| 文件 | 问题 |
|---|---|
| `zectrix-s3-epaper-4.2.cc` | `Power_Init()` 原来无超时死等 `VBAT_PWR_GPIO`（与下翻页键共用 GPIO18，开机检查双键后被 reset 成浮空）→ 改为配输入上拉 + 300ms 超时 |
| `zectrix-s3-epaper-4.2.cc` | **上键（GPIO39）永远判定按下、无法翻页**：`CheckDualButtonForOtaSwitch()` 在 Button（iot_button）初始化之后调用 `gpio_reset_pin()`，该函数会 `gpio_input_disable` 并撤销 GPIO 占用，使 39 失去上拉浮空为低 → 改为用 `gpio_config(输入+上拉)` 恢复 |
| `main/CMakeLists.txt` | `zectrix_nfc.cc` 未进构建（glob 只扫板卡目录）→ 显式加入 `SOURCES` |
| `zectrix_nfc.cc/.h` | 补齐缺失成员 `i2c_bus_` / `device_address_` |
| `custom_lcd_display.cc` | `sleep_manager.h` include 路径修正 |
| `rtc_pcf8563.cc` | 总线复位改走 `BoardI2cResetBus()` 弱符号钩子 |

## 5. 已知限制

- 语音对话只有「设备发音频 + 服务端回占位文本」，ASR/LLM/TTS 未接。
- `display_image` 走 JSON + base64（20KB/页），Wi-Fi 弱信号下首屏可能延迟 1–2 秒。
- 服务端未实现鉴权，仅建议内网/反代后使用。