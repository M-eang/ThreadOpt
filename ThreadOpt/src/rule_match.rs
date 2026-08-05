use crate::MAX_THREAD_LEN;
use crate::config::AppConfig;
use crate::cpuset::{CpuSet, ensure_cpuset_dir};

/// 线程亲和性计算结果
pub struct AffinityResult {
    pub cpus: CpuSet,
    pub cpuset_dir: String,
    pub is_thread_rule: bool,
}

/// 线程规则 CPU 累加，无线程匹配走包级 fallback，仍无则返回 None
pub fn thread_affinity(pkg: &str, thread: &str, cfg: &AppConfig) -> Option<AffinityResult> {
    let mut cpus = CpuSet::new();
    let mut cpuset_dir = String::new();
    let mut matched = false;

    if !thread.is_empty() {
        for rule in &cfg.rules {
            if rule.pkg != pkg || rule.thread.is_empty() {
                continue;
            }
            if fnmatch_c(&rule.thread_pattern, thread) {
                cpus.or(&rule.cpus);
                matched = true;
            }
        }
        // 按合并后的 CPU 集合重算 cpuset 目录，确保与亲和性一致
        if matched {
            cpuset_dir = ensure_cpuset_dir(&cpus, &cfg.topo);
        }
    }

    if !matched {
        let mut fallback_seen = false;
        for rule in &cfg.rules {
            if rule.pkg != pkg || !rule.thread.is_empty() {
                continue;
            }
            cpus.or(&rule.cpus);
            if !fallback_seen {
                cpuset_dir = rule.cpuset_dir.clone();
                fallback_seen = true;
            } else {
                cpuset_dir.clear();
            }
        }
    }

    if cpus.count() == 0 {
        if cfg.has_thread_rules.contains(pkg) {
            return Some(AffinityResult {
                cpus: cfg.topo.present_cpus.clone(),
                cpuset_dir: String::new(),
                is_thread_rule: false,
            });
        }
        None
    } else {
        Some(AffinityResult {
            cpus,
            cpuset_dir,
            is_thread_rule: matched,
        })
    }
}

/// 线程名模式匹配（类 fnmatch 语义，跨平台自研实现）
///
/// 支持的语法（与 README 文档一致）：
/// - `*` 匹配任意字符序列（含空）
/// - `?` 匹配恰好一个字符
/// - `[范围]` 匹配集合中任一字符，支持 `a-z` 范围与 `!` 取反
pub fn fnmatch_c(pattern: &str, string: &str) -> bool {
    if string.len() >= MAX_THREAD_LEN {
        return false;
    }
    match_pattern(pattern.as_bytes(), string.as_bytes())
}

fn match_pattern(p: &[u8], s: &[u8]) -> bool {
    match p.first() {
        None => s.is_empty(),
        Some(b'*') => {
            // 跳过连续 `*`，回溯匹配其余部分
            let mut rest = &p[1..];
            while rest.first() == Some(&b'*') {
                rest = &rest[1..];
            }
            let mut i = 0;
            loop {
                if match_pattern(rest, &s[i..]) {
                    return true;
                }
                if i >= s.len() {
                    return false;
                }
                i += 1;
            }
        }
        Some(b'?') => !s.is_empty() && match_pattern(&p[1..], &s[1..]),
        Some(b'[') => {
            if s.is_empty() {
                return false;
            }
            let Some(after) = match_class(p, s[0]) else {
                return false;
            };
            match_pattern(after, &s[1..])
        }
        Some(&c) => !s.is_empty() && s[0] == c && match_pattern(&p[1..], &s[1..]),
    }
}

/// 匹配 `[..]` 字符集，返回字符集之后的模式切片；语法不合法时返回 None
///
/// 支持 `[abc]`、`[a-z]`、`[!abc]` 取反；`]` 位于集首时按字面量处理（对齐 glibc）
fn match_class(p: &[u8], c: u8) -> Option<&[u8]> {
    let mut i = 1;
    let mut negate = false;
    if i < p.len() && (p[i] == b'!' || p[i] == b'^') {
        negate = true;
        i += 1;
    }
    let mut matched = false;
    let mut first = true;
    while i < p.len() {
        if p[i] == b']' && !first {
            // 集结束：字符命中且未取反才算匹配，否则整体不匹配
            return if matched != negate {
                Some(&p[i + 1..])
            } else {
                None
            };
        }
        first = false;
        // 范围 a-z
        if i + 2 < p.len() && p[i + 1] == b'-' && p[i + 2] != b']' {
            if p[i] <= c && c <= p[i + 2] {
                matched = true;
            }
            i += 3;
        } else {
            if p[i] == c {
                matched = true;
            }
            i += 1;
        }
    }
    None
}

/// 通过内核 comm 匹配配置包名
pub fn comm_to_pkg(comm: &str, cfg: &AppConfig) -> Option<String> {
    if cfg.pkgs.contains(comm) {
        return Some(comm.to_string());
    }
    if comm.len() >= 15 {
        for pkg in &cfg.pkgs {
            if pkg.starts_with(comm) {
                return Some(pkg.clone());
            }
        }
        for pkg in &cfg.pkgs {
            if pkg.ends_with(comm) {
                return Some(pkg.clone());
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn topo() -> crate::cpuset::CpuTopology {
        let mut present = CpuSet::new();
        for c in 0..8 {
            present.set(c);
        }
        let mut e = CpuSet::new();
        e.set(0);
        e.set(1);
        let mut p = CpuSet::new();
        p.set(2);
        p.set(3);
        let mut hp = CpuSet::new();
        for c in 4..8 {
            hp.set(c);
        }
        crate::cpuset::CpuTopology {
            present_cpus: present,
            present_str: "0-7".to_string(),
            mems_str: "0".to_string(),
            cpuset_enabled: false,
            e_core: e,
            p_core: p,
            hp_core: hp,
        }
    }

    /// 构造单条规则的配置
    fn cfg_with(pkg: &str, thread: &str, cpus_spec: &str) -> AppConfig {
        let mut rules = Vec::new();
        let topo = topo();
        // 直接复用 config::add_rule 的等价逻辑：用 parse_cpu_spec 构建
        let cpus = crate::cpuset::parse_cpu_spec(cpus_spec, &topo);
        rules.push(crate::config::AffinityRule {
            pkg: pkg.to_string(),
            thread: thread.to_string(),
            thread_pattern: thread.to_string(),
            cpuset_dir: String::new(),
            cpus,
        });
        AppConfig {
            pkgs: rules.iter().map(|r| r.pkg.clone()).collect(),
            has_thread_rules: rules
                .iter()
                .filter(|r| !r.thread.is_empty())
                .map(|r| r.pkg.clone())
                .collect(),
            rules,
            topo,
            config_file: String::new(),
        }
    }

    #[test]
    fn fnmatch_star() {
        assert!(fnmatch_c("render_*", "render_thread"));
        assert!(fnmatch_c("render_*", "render_"));
        // `*` 前的字面部分必须存在
        assert!(!fnmatch_c("render_*", "render"));
        // `*` 可匹配空序列
        assert!(fnmatch_c("render*", "render"));
        assert!(fnmatch_c("*", "anything"));
        assert!(fnmatch_c("*", ""));
        assert!(!fnmatch_c("render_*", "other_thread"));
    }

    #[test]
    fn fnmatch_question() {
        assert!(fnmatch_c("worker_?", "worker_1"));
        assert!(fnmatch_c("worker_?", "worker_A"));
        // `?` 匹配任意单字符（含下划线）
        assert!(fnmatch_c("worker_?", "worker__"));
        assert!(!fnmatch_c("worker_?", "worker_"));
        assert!(!fnmatch_c("worker_?", "worker_12"));
    }

    #[test]
    fn fnmatch_char_class() {
        assert!(fnmatch_c("thread_[0-9]", "thread_5"));
        assert!(fnmatch_c("thread_[0-9]", "thread_0"));
        assert!(!fnmatch_c("thread_[0-9]", "thread_a"));
        assert!(fnmatch_c("thread_[a-f]", "thread_c"));
        assert!(fnmatch_c("thread_[abc]", "thread_b"));
        assert!(fnmatch_c("thread_[!0-9]", "thread_x"));
        assert!(!fnmatch_c("thread_[!0-9]", "thread_3"));
    }

    #[test]
    fn fnmatch_exact_and_edge() {
        assert!(fnmatch_c("main", "main"));
        assert!(!fnmatch_c("main", "mainx"));
        assert!(!fnmatch_c("main", ""));
        assert!(fnmatch_c("", ""));
        assert!(!fnmatch_c("", "x"));
        // 超长线程名直接拒绝
        let long = "x".repeat(MAX_THREAD_LEN);
        assert!(!fnmatch_c("*", &long));
        assert!(fnmatch_c("*", &"x".repeat(MAX_THREAD_LEN - 1)));
    }

    #[test]
    fn fnmatch_unclosed_class() {
        // 未闭合的 [ 视为不匹配（配置不合法）
        assert!(!fnmatch_c("thread_[0-9", "thread_5"));
    }

    #[test]
    fn thread_rule_matches() {
        let cfg = cfg_with("com.example.game", "render_*", "4-5");
        let r = thread_affinity("com.example.game", "render_thread", &cfg).unwrap();
        assert!(r.is_thread_rule);
        assert_eq!(r.cpus.to_range_string(), "4-5");
        assert!(!r.cpus.is_set(3));
    }

    #[test]
    fn thread_rule_merges_multiple() {
        let mut cfg = cfg_with("com.example.game", "a*", "0-1");
        cfg.rules.push(crate::config::AffinityRule {
            pkg: "com.example.game".to_string(),
            thread: "*b".to_string(),
            thread_pattern: "*b".to_string(),
            cpuset_dir: String::new(),
            cpus: {
                let mut c = CpuSet::new();
                c.set(6);
                c.set(7);
                c
            },
        });
        let r = thread_affinity("com.example.game", "ab", &cfg).unwrap();
        assert_eq!(r.cpus.to_range_string(), "0-1,6-7");
    }

    #[test]
    fn pkg_fallback_when_no_thread_match() {
        let cfg = cfg_with("com.example.app", "", "2-3");
        let r = thread_affinity("com.example.app", "some_thread", &cfg).unwrap();
        assert!(!r.is_thread_rule);
        assert_eq!(r.cpus.to_range_string(), "2-3");
    }

    #[test]
    fn no_rule_returns_none() {
        let cfg = cfg_with("com.example.app", "", "2-3");
        assert!(thread_affinity("com.other.app", "t", &cfg).is_none());
    }

    #[test]
    fn thread_rules_exist_fallback_to_present() {
        // 包有线程规则但当前线程无匹配 → 兜底返回全部 present CPU
        let cfg = cfg_with("com.example.game", "render_*", "4-5");
        let r = thread_affinity("com.example.game", "bg_worker", &cfg).unwrap();
        assert_eq!(r.cpus.to_range_string(), "0-7");
    }

    #[test]
    fn comm_exact_match() {
        let cfg = cfg_with("com.example.app", "", "0");
        assert_eq!(
            comm_to_pkg("com.example.app", &cfg),
            Some("com.example.app".to_string())
        );
    }

    #[test]
    fn comm_prefix_and_suffix() {
        let cfg = cfg_with("com.example.myapplication", "", "0");
        // 内核 comm 截断 15 字符时按前缀/后缀匹配
        assert_eq!(
            comm_to_pkg("com.example.myap", &cfg),
            Some("com.example.myapplication".to_string())
        );
        assert_eq!(
            comm_to_pkg("xample.myapplication", &cfg),
            Some("com.example.myapplication".to_string())
        );
    }

    #[test]
    fn comm_short_no_match() {
        let cfg = cfg_with("com.example.application", "", "0");
        // 短 comm 不启用前后缀匹配
        assert_eq!(comm_to_pkg("app", &cfg), None);
        assert_eq!(comm_to_pkg("e.application", &cfg), None);
    }
}
