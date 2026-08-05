//! ThreadOpt 核心库：线程亲和性规则匹配与 CPU 拓扑处理
//!
//! 与平台无关的纯逻辑部分（规则解析、位图运算、模式匹配），
//! 可在 Windows 上编译与测试；Linux-only 的系统调用代码由 `cfg` 隔离，
//! 仅在 Android/Linux 目标编译。
//!
//! 可执行入口见二进制 `main.rs`（Linux/Android 专属，含 eBPF 与 /proc 轮询）。

pub mod config;
pub mod cpuset;
pub mod mode;
pub mod rule_match;

use std::sync::Mutex;
use std::sync::atomic::AtomicBool;

pub const MAX_PKG_LEN: usize = 128;
pub const MAX_THREAD_LEN: usize = 32;

pub static CONFIG_UPDATED: AtomicBool = AtomicBool::new(false);

pub fn lock_ignore_poison<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|e| {
        eprintln!("警告: 互斥锁中毒，尝试恢复...");
        e.into_inner()
    })
}
