//! 前台应用检测：遍历 /proc 判断游戏名单内是否有进程处于前台
//!
//! 判定依据 oom_score_adj：Android 前台 Activity 所在进程 adj ≈ 0，
//! 可见但非前台 ≈ 100，后台 ≥ 200。命中游戏名单且 adj ≤ 100 视为游戏处于活跃状态。

use std::collections::HashSet;
use std::fs;
use std::io::Read;

use crate::apply_affinity::read_cmdline;

/// 前台/可见判定阈值：≤ 此值视为用户正在使用（前台活动或分屏可见）
const FOREGROUND_ADJ_MAX: i32 = 100;

/// 读取进程 oom_score_adj，读取失败（权限/已退出）返回 None
fn read_oom_score_adj(pid: i32) -> Option<i32> {
    let mut buf = [0u8; 16];
    let mut f = fs::File::open(format!("/proc/{}/oom_score_adj", pid)).ok()?;
    let n = f.read(&mut buf).ok()?;
    let s = std::str::from_utf8(&buf[..n]).ok()?;
    s.trim().parse().ok()
}

/// 游戏名单内是否有进程处于前台；名单为空/未命中返回 false
pub fn game_foreground(game_pkgs: &HashSet<String>) -> bool {
    let Ok(entries) = fs::read_dir("/proc") else {
        return false;
    };
    for entry in entries.flatten() {
        let Ok(pid) = entry.file_name().to_string_lossy().parse::<i32>() else {
            continue;
        };
        let Some(pkg) = read_cmdline(pid) else {
            continue;
        };
        if !game_pkgs.contains(&pkg) {
            continue;
        }
        if read_oom_score_adj(pid).is_some_and(|adj| adj <= FOREGROUND_ADJ_MAX) {
            return true;
        }
    }
    false
}
