#ifndef ZECTRIX_I2C_BUS_LOCK_H
#define ZECTRIX_I2C_BUS_LOCK_H

#include <esp_err.h>

// 板级 I2C 会话锁：把"电源循环 + 探测 + 读写"这类多步事务绑成一个原子的总线会话。
// 由板卡（i2c_power_hook.cc）提供真正的互斥实现；未实现时按"锁不可用"降级，
// 调用方检查 locked() 后走各自的失败路径。
extern "C" esp_err_t __attribute__((weak)) BoardI2cBusLockAcquire(const char* reason) {
    return ESP_ERR_NOT_SUPPORTED;
}
extern "C" void __attribute__((weak)) BoardI2cBusLockRelease() {}

class ScopedI2cBusLock {
public:
    explicit ScopedI2cBusLock(const char* reason) {
        status_ = BoardI2cBusLockAcquire(reason);
        locked_ = (status_ == ESP_OK);
    }
    ~ScopedI2cBusLock() {
        if (locked_) {
            BoardI2cBusLockRelease();
        }
    }
    ScopedI2cBusLock(const ScopedI2cBusLock&) = delete;
    ScopedI2cBusLock& operator=(const ScopedI2cBusLock&) = delete;

    bool locked() const { return locked_; }
    esp_err_t status() const { return status_; }

private:
    bool locked_ = false;
    esp_err_t status_ = ESP_OK;
};

#endif  // ZECTRIX_I2C_BUS_LOCK_H
