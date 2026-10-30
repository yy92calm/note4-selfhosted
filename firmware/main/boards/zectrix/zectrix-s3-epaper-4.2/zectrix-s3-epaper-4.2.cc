#include <stdio.h>
#include <driver/i2c_master.h>
#include <driver/gpio.h>
#include <esp_log.h>
#include <esp_timer.h>
#include <esp_ota_ops.h>
#include <esp_partition.h>

#include "application.h"
#include "button.h"
#include "codecs/es8311_audio_codec.h"
#include "config.h"
#include "wifi_board.h"
#include "board_power_bsp.h"
#include "charge_status.h"
#include "custom_lcd_display.h"
#include "rtc_pcf8563.h"
#include "sleep_manager.h"
#include "../zectrix_nfc.h"
#include "power_save_timer.h"
#include "settings.h"
#include "mcp_server.h"

#define TAG "zectrix_s3_epaper_4_2"

/**
 * @brief 检测双按钮（上翻页+下翻页）同时按下，切换OTA启动分区
 * 
 * 在设备启动早期检测GPIO39和GPIO18是否同时被按下（持续500ms），
 * 如果检测到则切换到另一个OTA分区并重启设备。
 * 用于在小智固件和Zectrix固件之间切换。
 */
static void CheckDualButtonForOtaSwitch() {
    // 配置按钮GPIO为输入，启用上拉
    gpio_config_t io_conf = {};
    io_conf.intr_type = GPIO_INTR_DISABLE;
    io_conf.mode = GPIO_MODE_INPUT;
    io_conf.pin_bit_mask = (1ULL << TODO_UP_BUTTON_GPIO) | (1ULL << TODO_DOWN_BUTTON_GPIO);
    io_conf.pull_down_en = GPIO_PULLDOWN_DISABLE;
    io_conf.pull_up_en = GPIO_PULLUP_ENABLE;
    gpio_config(&io_conf);

    // 等待GPIO稳定
    vTaskDelay(pdMS_TO_TICKS(50));

    // 检测两个按钮是否同时按下（低电平有效）
    bool up_pressed = (gpio_get_level(TODO_UP_BUTTON_GPIO) == 0);
    bool down_pressed = (gpio_get_level(TODO_DOWN_BUTTON_GPIO) == 0);

    if (up_pressed && down_pressed) {
        ESP_LOGI(TAG, "Dual buttons detected, waiting for confirmation...");
        
        // 等待500ms确认不是误触
        vTaskDelay(pdMS_TO_TICKS(500));
        
        // 再次检查按钮状态
        up_pressed = (gpio_get_level(TODO_UP_BUTTON_GPIO) == 0);
        down_pressed = (gpio_get_level(TODO_DOWN_BUTTON_GPIO) == 0);
        
        if (up_pressed && down_pressed) {
            ESP_LOGI(TAG, "Dual button switch confirmed, switching OTA partition...");
            
            // 获取当前运行的分区
            const esp_partition_t *running_partition = esp_ota_get_running_partition();
            ESP_LOGI(TAG, "Current running partition: %s", running_partition->label);
            
            // 获取下一个OTA分区
            const esp_partition_t *next_partition = esp_ota_get_next_update_partition(NULL);
            
            if (next_partition != NULL) {
                ESP_LOGI(TAG, "Switching to partition: %s", next_partition->label);
                
                // 设置下一个启动分区
                esp_err_t err = esp_ota_set_boot_partition(next_partition);
                if (err == ESP_OK) {
                    ESP_LOGI(TAG, "Boot partition set successfully, rebooting in 2 seconds...");
                    
                    // 延迟2秒让用户看到日志
                    vTaskDelay(pdMS_TO_TICKS(2000));
                    
                    // 重启设备
                    esp_restart();
                } else {
                    ESP_LOGE(TAG, "Failed to set boot partition: %s", esp_err_to_name(err));
                }
            } else {
                ESP_LOGE(TAG, "No next update partition found");
            }
        } else {
            ESP_LOGI(TAG, "Dual button press not confirmed, continuing normal boot");
        }
    }
    
    // 恢复GPIO为输入+上拉。不能用 gpio_reset_pin：它会 gpio_input_disable 并撤销
    // GPIO 占用，废掉本函数之前 Button(iot_button) 已初始化的引脚配置——
    // 上键 GPIO39 因此失去上拉浮空为低，永远判定为「按下」，点击事件全丢。
    gpio_config_t restore_conf = {};
    restore_conf.intr_type = GPIO_INTR_DISABLE;
    restore_conf.mode = GPIO_MODE_INPUT;
    restore_conf.pin_bit_mask = (1ULL << TODO_UP_BUTTON_GPIO) | (1ULL << TODO_DOWN_BUTTON_GPIO);
    restore_conf.pull_down_en = GPIO_PULLDOWN_DISABLE;
    restore_conf.pull_up_en = GPIO_PULLUP_ENABLE;
    gpio_config(&restore_conf);
}

class CustomBoard : public WifiBoard {
private:
    i2c_master_bus_handle_t i2c_bus_;
    Button boot_button_;
    Button up_button_;
    Button down_button_;
    CustomLcdDisplay *display_ = nullptr;
    BoardPowerBsp *power_ = nullptr;
    ChargeStatus charge_status_;
    std::unique_ptr<RtcPcf8563> rtc_;
    std::unique_ptr<ZectrixNfc> nfc_;
    PowerSaveTimer *power_save_timer_ = nullptr;

    // 确认键短按/长按判定：按住 400ms 才进入说话，短按松开 = 开始阅读
    esp_timer_handle_t boot_hold_timer_ = nullptr;
    volatile bool boot_hold_fired_ = false;


    adc_oneshot_unit_handle_t adc1_handle = nullptr;
    adc_cali_handle_t cali_handle = nullptr;

    void InitializeI2c() {
        i2c_master_bus_config_t i2c_bus_cfg = {};
        i2c_bus_cfg.i2c_port          = (i2c_port_t) 0;
        i2c_bus_cfg.sda_io_num        = AUDIO_CODEC_I2C_SDA_PIN;
        i2c_bus_cfg.scl_io_num        = AUDIO_CODEC_I2C_SCL_PIN;
        i2c_bus_cfg.clk_source        = I2C_CLK_SRC_DEFAULT;
        i2c_bus_cfg.glitch_ignore_cnt = 7;
        i2c_bus_cfg.intr_priority     = 0;
        i2c_bus_cfg.trans_queue_depth = 0;
        i2c_bus_cfg.flags.enable_internal_pullup = 1;
        ESP_ERROR_CHECK(i2c_new_master_bus(&i2c_bus_cfg, &i2c_bus_));
    }

    void InitializeButtons() {
        // 确认键双语义：
        //   短按（400ms 内松开）→ 开始阅读，服务端回推当前页
        //   按住（≥400ms）      → 按住说话（PTT），松开结束
        esp_timer_create_args_t timer_args = {};
        timer_args.callback = [](void* arg) {
            static_cast<CustomBoard*>(arg)->boot_hold_fired_ = true;
            Application::GetInstance().Schedule([]() {
                Application::GetInstance().StartListening();
            });
        };
        timer_args.arg = this;
        timer_args.name = "boot_hold";
        ESP_ERROR_CHECK(esp_timer_create(&timer_args, &boot_hold_timer_));

        boot_button_.OnClick([this]() {
            auto &app = Application::GetInstance();
            if (app.GetDeviceState() == kDeviceStateStarting) {
                if (display_) {
                    display_->SetBookMode(false);  // 配网画面需要聊天 UI
                }
                EnterWifiConfigMode();
            }
        });

        boot_button_.OnPressDown([this]() {
            auto &app = Application::GetInstance();
            if (app.GetDeviceState() == kDeviceStateStarting) {
                return;
            }
            boot_hold_fired_ = false;
            esp_timer_start_once(boot_hold_timer_, 400 * 1000);
        });

        boot_button_.OnPressUp([this]() {
            auto &app = Application::GetInstance();
            if (app.GetDeviceState() == kDeviceStateStarting) {
                return;
            }
            esp_timer_stop(boot_hold_timer_);
            if (boot_hold_fired_) {
                boot_hold_fired_ = false;
                if (app.GetDeviceState() == kDeviceStateListening) {
                    app.StopListening();
                }
            } else {
                app.RequestReadingPage();
            }
        });

        // 上/下键：翻页。通道未打开时自动建立会话并上报 device_event，
        // 服务端折算进度后推送对应书页位图回来显示。
        // 启动阶段忽略：否则开机时按翻页键会误入配网模式（配网只留给确认键）。
        up_button_.OnClick([this]() {
            auto &app = Application::GetInstance();
            if (app.GetDeviceState() == kDeviceStateStarting) {
                return;
            }
            app.SendPageTurnEvent(-1, "up");
        });

        down_button_.OnClick([this]() {
            auto &app = Application::GetInstance();
            if (app.GetDeviceState() == kDeviceStateStarting) {
                return;
            }
            app.SendPageTurnEvent(1, "down");
        });
    }

    void InitializePowerSaveTimer() {
        power_save_timer_ = new PowerSaveTimer(-1, 20, 300);
        power_save_timer_->OnEnterSleepMode([this]() {
            ESP_LOGI(TAG, "Entering sleep mode");
        });
        power_save_timer_->OnExitSleepMode([this]() {
            ESP_LOGI(TAG, "Exiting sleep mode");
        });
        power_save_timer_->OnShutdownRequest([this]() {
            if (sm_block_sleep()) {
                ESP_LOGI(TAG, "Shutdown deferred: display busy");
                return;
            }
            // 关机前重绘缓存的待机页位图，墨水屏断电后画面保留
            if (display_) {
                display_->DrawCachedStandby();
            }
            for (int i = 0; i < 45 && display_ && display_->IsRefreshPending(); i++) {
                vTaskDelay(pdMS_TO_TICKS(100));
            }
            power_->PowerAudioOff();
            power_->PowerEpdOff();
            power_->VbatPowerOff();
        });
        power_save_timer_->SetEnabled(true);
    }

    void InitializeTools() {
        auto &mcp_server = McpServer::GetInstance();
        mcp_server.AddTool("self.disp.network", "重新配网", PropertyList(), [this](const PropertyList &) -> ReturnValue {
            EnterWifiConfigMode();
            return true;
        });
    }

    void InitializeLcdDisplay() {
        custom_lcd_spi_t lcd_spi_data = {};
        lcd_spi_data.cs               = EPD_CS_PIN;
        lcd_spi_data.dc               = EPD_DC_PIN;
        lcd_spi_data.rst              = EPD_RST_PIN;
        lcd_spi_data.busy             = EPD_BUSY_PIN;
        lcd_spi_data.mosi             = EPD_MOSI_PIN;
        lcd_spi_data.scl              = EPD_SCK_PIN;
        lcd_spi_data.power            = EPD_PWR_PIN;
        lcd_spi_data.spi_host         = EPD_SPI_NUM;
        lcd_spi_data.buffer_len       = EXAMPLE_LCD_WIDTH * EXAMPLE_LCD_HEIGHT / 8;
        display_ = new CustomLcdDisplay(NULL, NULL, EXAMPLE_LCD_WIDTH, EXAMPLE_LCD_HEIGHT,
                                        DISPLAY_OFFSET_X, DISPLAY_OFFSET_Y,
                                        DISPLAY_MIRROR_X, DISPLAY_MIRROR_Y, DISPLAY_SWAP_XY,
                                        lcd_spi_data);
    }

    void Power_Init() {
        int64_t now_ms = esp_timer_get_time() / 1000;
        charge_status_.Init(static_cast<gpio_num_t>(CHARGE_DETECT_GPIO),
                           static_cast<gpio_num_t>(CHARGE_FULL_GPIO),
                           now_ms);

        power_ = new BoardPowerBsp(EPD_PWR_PIN, Audio_PWR_PIN, Audio_AMP_PIN, VBAT_PWR_PIN,
                                   &charge_status_);
        power_->VbatPowerOn();
        power_->PowerAudioOn();
        power_->PowerEpdOn();
        // VBAT_PWR_GPIO 与下翻页键共用 GPIO18；CheckDualButtonForOtaSwitch 已把它
        // reset 成浮空输入，这里先配回输入上拉再轮询，并加超时兜底防止死等
        gpio_config_t vbat_conf = {};
        vbat_conf.intr_type = GPIO_INTR_DISABLE;
        vbat_conf.mode = GPIO_MODE_INPUT;
        vbat_conf.pin_bit_mask = 1ULL << VBAT_PWR_GPIO;
        vbat_conf.pull_down_en = GPIO_PULLDOWN_DISABLE;
        vbat_conf.pull_up_en = GPIO_PULLUP_ENABLE;
        gpio_config(&vbat_conf);
        int wait_ms = 0;
        do {
            vTaskDelay(pdMS_TO_TICKS(10));
            wait_ms += 10;
            if (wait_ms >= 300) {
                ESP_LOGW(TAG, "VBAT power confirm timeout (GPIO%d low), continuing", VBAT_PWR_GPIO);
                break;
            }
        } while (!gpio_get_level(static_cast<gpio_num_t>(VBAT_PWR_GPIO)));
    }

    void InitializeRtc() {
        rtc_ = std::make_unique<RtcPcf8563>(i2c_bus_, RTC_I2C_ADDR);
        rtc_->Init(static_cast<gpio_num_t>(RTC_INT_GPIO));
        ESP_LOGI(TAG, "RTC PCF8563 initialized");
    }

    void InitializeNfc() {
        nfc_ = std::make_unique<ZectrixNfc>(i2c_bus_, NFC_I2C_ADDR,
                                            static_cast<gpio_num_t>(NFC_PWR_GPIO),
                                            static_cast<gpio_num_t>(NFC_FD_GPIO),
                                            NFC_FD_ACTIVE_LEVEL);
        if (nfc_->Init()) {
            ESP_LOGI(TAG, "NFC GT23SC6699 initialized");
        } else {
            ESP_LOGW(TAG, "NFC GT23SC6699 init failed");
        }
    }

    void ChargeStatusTick() {
        int64_t now_ms = esp_timer_get_time() / 1000;
        charge_status_.Tick(now_ms);

        if (power_save_timer_) {
            auto snap = charge_status_.Get();
            power_save_timer_->SetEnabled(!snap.charging);
        }
    }

    uint16_t BatteryGetVoltage() {
        static bool initialized = false;
        static adc_oneshot_unit_handle_t adc_handle = nullptr;
        static adc_cali_handle_t cal_handle = nullptr;

        if (!initialized) {
            adc_oneshot_unit_init_cfg_t init_config = {
                .unit_id = ADC_UNIT_1,
            };
            adc_oneshot_new_unit(&init_config, &adc_handle);

            adc_oneshot_chan_cfg_t ch_config = {
                .atten = ADC_ATTEN_DB_12,
                .bitwidth = ADC_BITWIDTH_12,
            };
            adc_oneshot_config_channel(adc_handle, ADC_CHANNEL_3, &ch_config);

            adc_cali_curve_fitting_config_t cali_config = {
                .unit_id = ADC_UNIT_1,
                .atten = ADC_ATTEN_DB_12,
                .bitwidth = ADC_BITWIDTH_12,
            };
            if (adc_cali_create_scheme_curve_fitting(&cali_config, &cal_handle) == ESP_OK) {
                initialized = true;
            }
        }

        if (initialized) {
            int raw_value = 0;
            int raw_voltage = 0;
            int voltage = 0;
            adc_oneshot_read(adc_handle, ADC_CHANNEL_3, &raw_value);
            adc_cali_raw_to_voltage(cal_handle, raw_value, &raw_voltage);
            voltage = raw_voltage * 2;
            return (uint16_t)voltage;
        }

        return 0;
    }

    uint8_t BatteryGetPercent() {
        int voltage = 0;
        for (uint8_t i = 0; i < 10; i++) {
            voltage += BatteryGetVoltage();
        }
        voltage /= 10;
        int percent = (-1 * voltage * voltage + 9016 * voltage - 19189000) / 10000;
        percent = (percent > 100) ? 100 : (percent < 0) ? 0 : percent;
        return (uint8_t)percent;
    }

public:
    CustomBoard()
        : boot_button_(BOOT_BUTTON_GPIO),
          up_button_(TODO_UP_BUTTON_GPIO),
          down_button_(TODO_DOWN_BUTTON_GPIO) {
        // 在最早期检测双按钮切换OTA分区
        CheckDualButtonForOtaSwitch();
        
        Power_Init();
        InitializeI2c();
        InitializeButtons();
        InitializePowerSaveTimer();
        InitializeTools();
        InitializeRtc();
        InitializeNfc();
        InitializeLcdDisplay();
    }

    virtual AudioCodec *GetAudioCodec() override {
        static Es8311AudioCodec audio_codec(
            i2c_bus_, I2C_NUM_0,
            AUDIO_INPUT_SAMPLE_RATE, AUDIO_OUTPUT_SAMPLE_RATE,
            AUDIO_I2S_GPIO_MCLK, AUDIO_I2S_GPIO_BCLK, AUDIO_I2S_GPIO_WS,
            AUDIO_I2S_GPIO_DOUT, AUDIO_I2S_GPIO_DIN,
            AUDIO_CODEC_PA_PIN, AUDIO_CODEC_ES8311_ADDR);
        return &audio_codec;
    }

    virtual Display *GetDisplay() override {
        return display_;
    }

    virtual bool GetBatteryLevel(int &level, bool &charging, bool &discharging) override {
        ChargeStatusTick();
        auto snap = charge_status_.Get();
        charging = snap.charging;
        discharging = !charging && snap.power_present;
        level = (int)BatteryGetPercent();
        return true;
    }

    virtual void SetPowerSaveLevel(PowerSaveLevel level) override {
        if (level != PowerSaveLevel::LOW_POWER) {
            if (power_save_timer_) {
                power_save_timer_->WakeUp();
            }
        }
        WifiBoard::SetPowerSaveLevel(level);
    }
};

DECLARE_BOARD(CustomBoard);
