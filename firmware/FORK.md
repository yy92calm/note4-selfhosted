# 固件构建与刷写

本目录是 [cattei/xiaozhi-zectrix](https://github.com/cattei/xiaozhi-zectrix) 的 fork，基线 commit `177c032`。相对上游的改动见 [`../docs/firmware-changes.md`](../docs/firmware-changes.md)。

## 环境

- ESP-IDF **v6.0.2**（上游要求；本方案实测通过）
- Python 3.11
- USB 数据线连接设备（CH340/原生 USB-JTAG，设备出现 `/dev/cu.usbmodem*` 或 `COM*`）

```bash
# 首次：获取 ESP-IDF 并安装工具链（略，见 ESP-IDF 官方文档）
source <你的路径>/esp-idf/export.sh
idf.py --version   # 期望 v6.0.2
```

## 配置

编辑 `sdkconfig.zectrix`，把 OTA 地址改成你自己的服务端：

```c
CONFIG_OTA_URL="http://<你的服务器IP或域名>:12900/xiaozhi/ota/"
```

## 编译

```bash
cd firmware
export SDKCONFIG_DEFAULTS="sdkconfig.defaults;sdkconfig.defaults.esp32s3;sdkconfig.zectrix"
CMAKE_POLICY_VERSION_MINIMUM=3.5 \
  idf.py -B build_ota1 -DIDF_TARGET=esp32s3 build
```

产物：`build_ota1/xiaozhi.bin`（约 2.7MB，`ota_1` 槽位余量 55%）。

> 首次编译会自动拉取 `managed_components` 依赖，耗时较长。

## 刷写（只写 ota_1）

**先备份整片 Flash（唯一兜底，务必执行一次）：**

```bash
esptool --chip esp32s3 -p <串口> --baud 921600 read_flash 0 0x1000000 factory-16mb.bin
```

再刷入自研固件：

```bash
esptool --chip esp32s3 -p <串口> --baud 921600 \
  write_flash 0x610000 build_ota1/xiaozhi.bin
```

`0x610000` 是 `ota_1` 起始地址（见 `partitions/16m_zectrix.csv`）。**不要**用 `idf.py flash` 整片刷写——那会覆盖 `ota_0` 官方固件和厂商数据。

确认启动槽位：

```bash
# otadata 已指向 ota_1 时，设备会从自研固件启动；
# 若需要手动切换（工具在 ESP-IDF components/partition_table 下）：
PYTHONPATH=$IDF_PATH/components/partition_table \
  python otatool.py --port <串口> --partition-table-file build_ota1/partition_table/partition-table.bin \
  switch_ota_partition --slot 1
```

## 回退官方固件

两种方式，任选其一：

1. **双键切换**：设备开机时同时按住 ↑ + ↓ 约 0.5s → 切回 `ota_0`（厂商固件）。
2. **整片恢复**：`esptool write_flash 0 0x0 factory-16mb.bin`（覆盖全部，含厂商数据）。

## 验证

正常启动串口日志应包含：

```
I (..) Ota: Running partition: ota_1
I (..) StateMachine: State: activating -> idle
I (..) WifiBoard: Connected to WiFi: <ssid>
```

按键 ↓ 后应出现（服务端已登记书与页缓存的前提下）：

```
I (..) StateMachine: State: idle -> connecting
I (..) WS: Session ID: ...
I (..) Application: display_image 400x300 (15000 bytes)
```

## 串口抓日志注意

ESP32-S3 原生 USB-JTAG：串口工具打开端口后**必须立即拉低 DTR/RTS 的断言**（`dtr=False; rts=False`），否则设备会被复位进入下载模式。Python 示例：

```python
import serial
p = serial.Serial("/dev/cu.usbmodem2101", 115200, timeout=0.2)
p.dtr = False
p.rts = False
```