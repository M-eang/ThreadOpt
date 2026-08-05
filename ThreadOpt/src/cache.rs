use std::collections::HashMap;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::apply_affinity::affinity_set;
use threadopt::config::AppConfig;
use threadopt::cpuset::{CpuSet, CpuTopology};
use threadopt::rule_match::{comm_to_pkg, thread_affinity};

/// 绑核日志路径（配置文件同目录 logs/apply.log），未初始化时为 None（日志禁用）
static APPLY_LOG_PATH: OnceLock<Option<PathBuf>> = OnceLock::new();

/// 初始化绑核日志路径，日志写入 <配置文件目录>/logs/apply.log（目录不存在则创建）
pub fn init_apply_log(config_file: &str) {
    let path = Path::new(config_file)
        .parent()
        .map(|p| p.join("logs").join("apply.log"));
    if let Some(p) = &path {
        if let Some(dir) = p.parent() {
            let _ = fs::create_dir_all(dir);
        }
    }
    let _ = APPLY_LOG_PATH.set(path);
}

/// 记录一次绑核动作（失败静默，不影响主逻辑；超 1MB 滚动清空）
pub fn log_apply(tid: i32, comm: &str, pkg: &str, cpus: &str, is_thread_rule: bool) {
    let Some(Some(path)) = APPLY_LOG_PATH.get() else {
        return;
    };
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    let (h, m, s) = unsafe {
        let t: libc::time_t = now.as_secs() as libc::time_t;
        let mut tm: libc::tm = std::mem::zeroed();
        libc::localtime_r(&t, &mut tm);
        (tm.tm_hour, tm.tm_min, tm.tm_sec)
    };
    let line = format!(
        "[{:02}:{:02}:{:02}.{:03}] tid={} comm={} pkg={} cpus={} {}\n",
        h,
        m,
        s,
        now.subsec_millis(),
        tid,
        comm,
        pkg,
        cpus,
        if is_thread_rule { "thread" } else { "package" }
    );
    if fs::metadata(&path)
        .map(|m| m.len() > 1024 * 1024)
        .unwrap_or(false)
    {
        let _ = fs::write(&path, b"");
    }
    if let Ok(mut f) = OpenOptions::new().create(true).append(true).open(&path) {
        let _ = f.write_all(line.as_bytes());
    }
}

/// 线程条目
pub struct TaskEntry {
    pub pid: i32,
    pub pkg: String,
    pub cpus: CpuSet,
    pub cpuset_dir: String,
    pub is_thread_rule: bool,
}

/// 双模式共用进程缓存，eBPF 事件驱动增量维护，proc 模式触发全量重建
pub struct ProcCache {
    pub tasks: HashMap<i32, TaskEntry>,
}

impl ProcCache {
    pub fn new() -> Self {
        Self {
            tasks: HashMap::new(),
        }
    }

    pub fn clear(&mut self) {
        self.tasks.clear();
    }

    pub fn task_del(&mut self, tid: i32) {
        self.tasks.remove(&tid);
    }

    /// comm 匹配包名，线程名时回退主线程条目
    pub fn pkg_lookup_comm(&self, pid: i32, comm: &str, cfg: &AppConfig) -> Option<(String, bool)> {
        let pkg = comm_to_pkg(comm, cfg).or_else(|| self.tasks.get(&pid).map(|e| e.pkg.clone()))?;
        let htr = cfg.has_thread_rules.contains(&pkg);
        Some((pkg, htr))
    }

    /// 计算并应用线程亲和性，保护已有线程规则绑定防止降级
    pub fn task_apply<F>(
        &mut self,
        tid: i32,
        pid: i32,
        pkg: &str,
        comm: &str,
        has_thread_rules: bool,
        cfg: &AppConfig,
        apply_fn: F,
    ) -> bool
    where
        F: FnOnce(i32, &CpuSet, &str) -> bool,
    {
        let thread_name = if has_thread_rules { comm } else { "" };
        let Some(result) = thread_affinity(pkg, thread_name, cfg) else {
            return false;
        };

        if !result.is_thread_rule && self.tasks.get(&tid).is_some_and(|old| old.is_thread_rule) {
            return true;
        }

        let dead = apply_fn(tid, &result.cpus, &result.cpuset_dir);
        if dead {
            self.tasks.remove(&tid);
            return false;
        }

        log_apply(
            tid,
            comm,
            pkg,
            &result.cpus.to_range_string(),
            result.is_thread_rule,
        );

        self.tasks.insert(
            tid,
            TaskEntry {
                pid,
                pkg: pkg.to_string(),
                cpus: result.cpus,
                cpuset_dir: result.cpuset_dir,
                is_thread_rule: result.is_thread_rule,
            },
        );
        true
    }

    /// 遍历 tasks 应用亲和性，返回 dead_tids 供 eBPF 调用方清理 APPLIED_MAP
    pub fn affinity_sync(&mut self, topo: &CpuTopology) -> Vec<i32> {
        let dead_tids: Vec<i32> = self
            .tasks
            .iter()
            .filter_map(|(tid, e)| {
                if affinity_set(*tid, &e.cpus, &e.cpuset_dir, topo) {
                    Some(*tid)
                } else {
                    None
                }
            })
            .collect();
        for tid in &dead_tids {
            self.task_del(*tid);
        }
        dead_tids
    }
}
