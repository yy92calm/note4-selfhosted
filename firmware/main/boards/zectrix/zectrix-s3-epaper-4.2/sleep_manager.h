#ifndef ZECTRIX_SLEEP_MANAGER_H
#define ZECTRIX_SLEEP_MANAGER_H

#include <atomic>
#include <cstdint>

#include <esp_timer.h>

// 板级睡眠仲裁：墨水屏刷新期间置 busy（sm_set_busy）并用 kick 延长保持唤醒窗口
// （sm_kick）；板级休眠/关机动作前查询 sm_block_sleep()，忙则推迟，
// 避免把刷新到一半的屏幕断电。
enum class SleepBusySrc {
    Display = 0,
};

inline std::atomic<uint32_t>& SmBusyMask() {
    static std::atomic<uint32_t> mask{0};
    return mask;
}

inline std::atomic<int64_t>& SmBusyUntilUs() {
    static std::atomic<int64_t> until{0};
    return until;
}

inline void sm_set_busy(SleepBusySrc src, bool busy) {
    const uint32_t bit = 1u << static_cast<uint32_t>(src);
    if (busy) {
        SmBusyMask().fetch_or(bit, std::memory_order_acq_rel);
    } else {
        SmBusyMask().fetch_and(~bit, std::memory_order_acq_rel);
    }
}

inline void sm_kick(uint32_t ms, const char* reason) {
    (void)reason;
    const int64_t candidate = esp_timer_get_time() + static_cast<int64_t>(ms) * 1000;
    int64_t current = SmBusyUntilUs().load(std::memory_order_relaxed);
    while (candidate > current &&
           !SmBusyUntilUs().compare_exchange_weak(current, candidate,
                                                  std::memory_order_acq_rel,
                                                  std::memory_order_relaxed)) {
    }
}

inline bool sm_block_sleep() {
    if (SmBusyMask().load(std::memory_order_acquire) != 0) {
        return true;
    }
    return esp_timer_get_time() < SmBusyUntilUs().load(std::memory_order_acquire);
}

#endif  // ZECTRIX_SLEEP_MANAGER_H
