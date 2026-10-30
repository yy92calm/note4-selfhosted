#include <driver/gpio.h>
#include <driver/i2c_master.h>
#include <esp_log.h>
#include <freertos/FreeRTOS.h>
#include <freertos/semphr.h>

#include "config.h"

namespace {
constexpr char kTag[] = "I2cHook";

// 递归锁：NFC 传输会话持有锁期间触发的总线恢复会再次进入（同一线程）。
SemaphoreHandle_t BusMutex() {
    static SemaphoreHandle_t mutex = xSemaphoreCreateRecursiveMutex();
    return mutex;
}
}  // namespace

extern "C" void BoardI2cForcePowerOn() {
    const gpio_num_t pin = static_cast<gpio_num_t>(Audio_PWR_PIN);
    gpio_hold_dis(pin);
    gpio_set_level(pin, AUDIO_PWR_FORCE_LEVEL);
    gpio_hold_en(pin);
}

extern "C" esp_err_t BoardI2cBusLockAcquire(const char* reason) {
    SemaphoreHandle_t mutex = BusMutex();
    if (mutex == nullptr) {
        return ESP_ERR_NO_MEM;
    }
    if (xSemaphoreTakeRecursive(mutex, pdMS_TO_TICKS(500)) != pdTRUE) {
        return ESP_ERR_TIMEOUT;
    }
    return ESP_OK;
}

extern "C" void BoardI2cBusLockRelease() {
    SemaphoreHandle_t mutex = BusMutex();
    if (mutex != nullptr) {
        xSemaphoreGiveRecursive(mutex);
    }
}

extern "C" esp_err_t BoardI2cResetBus(const char* reason) {
    SemaphoreHandle_t mutex = BusMutex();
    esp_err_t err = ESP_OK;
    bool locked = false;
    if (mutex != nullptr && xSemaphoreTakeRecursive(mutex, pdMS_TO_TICKS(500)) == pdTRUE) {
        locked = true;
    }
    i2c_master_bus_handle_t bus = nullptr;
    err = i2c_master_get_bus_handle(0, &bus);
    if (err != ESP_OK || bus == nullptr) {
        ESP_LOGW(kTag, "ResetBus(%s): no bus handle err=%s",
                 reason ? reason : "unknown", esp_err_to_name(err));
        err = (err == ESP_OK) ? ESP_ERR_INVALID_STATE : err;
    } else {
        err = i2c_master_bus_reset(bus);
        ESP_LOGW(kTag, "ResetBus(%s) ret=%s",
                 reason ? reason : "unknown", esp_err_to_name(err));
    }
    if (locked) {
        xSemaphoreGiveRecursive(mutex);
    }
    return err;
}
