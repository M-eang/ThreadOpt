#[cfg(any(target_os = "linux", target_os = "android"))]
use std::ffi::CString;
use std::fmt::Write as _;
#[cfg(any(target_os = "linux", target_os = "android"))]
use std::fs;
#[cfg(any(target_os = "linux", target_os = "android"))]
use std::io;

use std::sync::OnceLock;

pub const CPU_SETSIZE: usize = 1024;
pub const CPU_WORD_BITS: usize = 64;
pub const CPU_WORDS: usize = CPU_SETSIZE / CPU_WORD_BITS;

/// BASE_CPUSET 运行时路径，未设置时默认 /dev/cpuset/ThreadOpt
static BASE_CPUSET_PATH: OnceLock<String> = OnceLock::new();

pub const DEFAULT_CPUSET_NAME: &str = "ThreadOpt";

/// 设置 BASE_CPUSET 目录名，name 为空或含 / 时使用默认值
pub fn set_base_cpuset(name: &str) {
    if name.is_empty() || name.contains('/') {
        return;
    }
    let path = format!("/dev/cpuset/{}", name);
    let _ = BASE_CPUSET_PATH.set(path);
}

/// 获取 BASE_CPUSET 路径，未设置返回默认值
pub fn base_cpuset() -> &'static str {
    BASE_CPUSET_PATH
        .get()
        .map(|s| s.as_str())
        .unwrap_or("/dev/cpuset/ThreadOpt")
}

#[cfg(any(target_os = "linux", target_os = "android"))]
const _: () = assert!(std::mem::size_of::<CpuSet>() == std::mem::size_of::<libc::cpu_set_t>());

#[repr(C)]
#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub struct CpuSet {
    pub bits: [u64; CPU_WORDS],
}

impl std::fmt::Debug for CpuSet {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "CpuSet({})", self.to_range_string())
    }
}

impl CpuSet {
    pub fn new() -> Self {
        CpuSet::default()
    }

    pub fn set(&mut self, cpu: usize) {
        if cpu < CPU_SETSIZE {
            self.bits[cpu / CPU_WORD_BITS] |= 1u64 << (cpu % CPU_WORD_BITS);
        }
    }

    pub fn is_set(&self, cpu: usize) -> bool {
        cpu < CPU_SETSIZE && self.bits[cpu / CPU_WORD_BITS] & (1u64 << (cpu % CPU_WORD_BITS)) != 0
    }

    pub fn count(&self) -> usize {
        self.bits.iter().map(|&b| b.count_ones() as usize).sum()
    }

    pub fn or(&mut self, other: &CpuSet) {
        for (d, &s) in self.bits.iter_mut().zip(other.bits.iter()) {
            *d |= s;
        }
    }

    /// 转换为范围字符串
    pub fn to_range_string(&self) -> String {
        let mut result = String::new();
        let mut start: Option<usize> = None;
        let mut end: Option<usize> = None;
        let mut first = true;

        for (word_idx, &word) in self.bits.iter().enumerate() {
            if word == 0 {
                if start.is_some() {
                    push_range(&mut result, start, end, &mut first);
                    start = None;
                    end = None;
                }
                continue;
            }
            let base = word_idx * CPU_WORD_BITS;
            for bit in 0..CPU_WORD_BITS {
                if word & (1u64 << bit) != 0 {
                    let cpu = base + bit;
                    if start.is_none() {
                        start = Some(cpu);
                        end = Some(cpu);
                    } else if end.is_some_and(|e| cpu == e + 1) {
                        end = Some(cpu);
                    } else {
                        push_range(&mut result, start, end, &mut first);
                        start = Some(cpu);
                        end = Some(cpu);
                    }
                }
            }
        }
        push_range(&mut result, start, end, &mut first);
        result
    }

    #[cfg(any(target_os = "linux", target_os = "android"))]
    pub fn get_affinity(tid: i32) -> Option<CpuSet> {
        let mut curr = CpuSet::new();
        let ret = unsafe {
            libc::sched_getaffinity(
                tid,
                std::mem::size_of::<CpuSet>(),
                &mut curr as *mut CpuSet as *mut libc::cpu_set_t,
            )
        };
        if ret == -1 { None } else { Some(curr) }
    }

    #[cfg(any(target_os = "linux", target_os = "android"))]
    pub fn set_affinity(&self, tid: i32) -> io::Result<()> {
        let ret = unsafe {
            libc::sched_setaffinity(
                tid,
                std::mem::size_of::<CpuSet>(),
                self as *const CpuSet as *const libc::cpu_set_t,
            )
        };
        if ret == -1 {
            Err(io::Error::last_os_error())
        } else {
            Ok(())
        }
    }
}

fn push_range(s: &mut String, start: Option<usize>, end: Option<usize>, first: &mut bool) {
    if let (Some(lo), Some(hi)) = (start, end) {
        if !*first {
            s.push(',');
        }
        if lo == hi {
            let _ = write!(s, "{}", lo);
        } else {
            let _ = write!(s, "{}-{}", lo, hi);
        }
        *first = false;
    }
}

/// 解析 CPU 范围字符串
pub fn parse_cpu_ranges(spec: &str, present: Option<&CpuSet>) -> CpuSet {
    let mut set = CpuSet::new();
    if spec.is_empty() {
        return set;
    }
    for part in spec.split(',') {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        let (lo, hi) = if let Some(pos) = part.find('-') {
            let a: usize = part[..pos].parse().ok().unwrap_or(usize::MAX);
            let b: usize = part[pos + 1..].parse().ok().unwrap_or(a);
            if a == usize::MAX {
                continue;
            }
            if a > b { (b, a) } else { (a, b) }
        } else {
            let a: usize = part.parse().ok().unwrap_or(usize::MAX);
            if a == usize::MAX {
                continue;
            }
            (a, a)
        };
        for i in lo..=hi.min(CPU_SETSIZE - 1) {
            if let Some(present) = present {
                if !present.is_set(i) {
                    continue;
                }
            }
            set.set(i);
        }
    }
    set
}

/// 解析 CPU 规格，支持语义核心名(e-core/p-core/hp-core)与数字范围混合
pub fn parse_cpu_spec(spec: &str, topo: &CpuTopology) -> CpuSet {
    let mut set = CpuSet::new();
    if spec.is_empty() {
        return set;
    }
    for part in spec.split(',') {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        // 语义名展开为对应核心层，其余按数字范围解析
        let seg = match part {
            "e-core" => &topo.e_core,
            "p-core" => &topo.p_core,
            "hp-core" => &topo.hp_core,
            "all-core" => &topo.present_cpus,
            _ => {
                set.or(&parse_cpu_ranges(part, Some(&topo.present_cpus)));
                continue;
            }
        };
        set.or(seg);
    }
    set
}

/// 创建 cpuset 子目录并写入 cpus 与 mems（仅 Linux/Android）
#[cfg(any(target_os = "linux", target_os = "android"))]
pub(crate) fn create_cpuset_dir(path: &str, cpus: &str, mems: &str) -> bool {
    let c_path = CString::new(path).expect("cpuset path 受控输入，无 NUL");
    let ret = unsafe { libc::mkdir(c_path.as_ptr(), 0o755) };
    if ret != 0 {
        let err = io::Error::last_os_error();
        if err.raw_os_error() != Some(libc::EEXIST) {
            return false;
        }
    }
    if unsafe { libc::chmod(c_path.as_ptr(), 0o755) } != 0 {
        return false;
    }
    if unsafe { libc::chown(c_path.as_ptr(), 0, 0) } != 0 {
        return false;
    }
    let cpus_path = format!("{}/cpus", path);
    if fs::write(&cpus_path, cpus).is_err() {
        return false;
    }
    let mems_path = format!("{}/mems", path);
    fs::write(&mems_path, mems).is_ok()
}

/// 非 Linux 平台测试桩：cpuset 目录创建不可用，返回 false
#[cfg(not(any(target_os = "linux", target_os = "android")))]
pub(crate) fn create_cpuset_dir(_path: &str, _cpus: &str, _mems: &str) -> bool {
    false
}

/// 按合并后的 CPU 集合确保 cpuset 子目录存在，返回目录名（cpuset 未启用或创建失败返回空串）
#[cfg(any(target_os = "linux", target_os = "android"))]
pub fn ensure_cpuset_dir(cpus: &CpuSet, topo: &CpuTopology) -> String {
    if !topo.cpuset_enabled {
        return String::new();
    }
    let dir_name = cpus.to_range_string();
    let path = format!("{}/{}", base_cpuset(), dir_name);
    // create_cpuset_dir 已处理 EEXIST，重复调用幂等
    if create_cpuset_dir(&path, &dir_name, &topo.mems_str) {
        dir_name
    } else {
        String::new()
    }
}

/// 非 Linux 平台测试桩：cpuset 目录创建不可用，返回空串
#[cfg(not(any(target_os = "linux", target_os = "android")))]
pub fn ensure_cpuset_dir(_cpus: &CpuSet, _topo: &CpuTopology) -> String {
    String::new()
}

#[derive(Clone)]
pub struct CpuTopology {
    pub present_cpus: CpuSet,
    pub present_str: String,
    pub mems_str: String,
    pub cpuset_enabled: bool,
    /// 语义核心分层：最低频为 e-core，最高频为 hp-core，中间为 p-core
    pub e_core: CpuSet,
    pub p_core: CpuSet,
    pub hp_core: CpuSet,
}

/// 按 cpufreq 策略检测核心分层，按最高频率升序分组：首组为 e-core，末组为 hp-core，中间为 p-core（仅 Linux/Android）
#[cfg(any(target_os = "linux", target_os = "android"))]
fn detect_core_types() -> (CpuSet, CpuSet, CpuSet) {
    let mut groups: Vec<(u64, Vec<usize>)> = Vec::new();
    // 读取每个 policy 的 related_cpus 与 cpuinfo_max_freq，按频率合并同组
    if let Ok(entries) = fs::read_dir("/sys/devices/system/cpu/cpufreq") {
        for entry in entries.flatten() {
            let path = entry.path();
            if !path
                .file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with("policy"))
            {
                continue;
            }
            let freq: u64 = fs::read_to_string(path.join("cpuinfo_max_freq"))
                .ok()
                .and_then(|s| s.trim().parse().ok())
                .unwrap_or(0);
            if freq == 0 {
                continue;
            }
            let cpus: Vec<usize> = fs::read_to_string(path.join("related_cpus"))
                .ok()
                .map(|s| {
                    s.split_whitespace()
                        .filter_map(|c| c.parse().ok())
                        .collect()
                })
                .unwrap_or_default();
            if cpus.is_empty() {
                continue;
            }
            if let Some(g) = groups.iter_mut().find(|(f, _)| *f == freq) {
                g.1.extend(cpus);
            } else {
                groups.push((freq, cpus));
            }
        }
    }
    groups.sort_by_key(|(f, _)| *f);
    let mut e = CpuSet::new();
    let mut p = CpuSet::new();
    let mut h = CpuSet::new();
    let n = groups.len();
    for (i, (_, cpus)) in groups.iter().enumerate() {
        // 首组入 e-core，末组入 hp-core，其余入 p-core
        let target = if i == 0 {
            &mut e
        } else if i == n - 1 {
            &mut h
        } else {
            &mut p
        };
        for &cpu in cpus {
            target.set(cpu);
        }
    }
    (e, p, h)
}

/// 初始化 CPU 拓扑，检测 cpuset 可用性并创建 BASE_CPUSET 目录（仅 Linux/Android）
#[cfg(any(target_os = "linux", target_os = "android"))]
pub fn init_cpu_topo() -> CpuTopology {
    let mut topo = CpuTopology {
        present_cpus: CpuSet::new(),
        present_str: String::new(),
        mems_str: String::new(),
        cpuset_enabled: false,
        e_core: CpuSet::new(),
        p_core: CpuSet::new(),
        hp_core: CpuSet::new(),
    };

    if let Ok(content) = fs::read_to_string("/sys/devices/system/cpu/present") {
        topo.present_str = content.trim().to_string();
    }
    topo.present_cpus = parse_cpu_ranges(&topo.present_str, None);
    let (e, p, h) = detect_core_types();
    topo.e_core = e;
    topo.p_core = p;
    topo.hp_core = h;

    let cpuset_path = CString::new("/dev/cpuset").expect("常量字符串无 NUL");
    if unsafe { libc::access(cpuset_path.as_ptr(), libc::F_OK) } != 0 {
        return topo;
    }

    let mems = fs::read_to_string("/dev/cpuset/mems")
        .ok()
        .and_then(|s| {
            let t = s.trim().to_string();
            if t.is_empty() { None } else { Some(t) }
        })
        .unwrap_or_else(|| "0".to_string());
    topo.mems_str = mems;

    if create_cpuset_dir(base_cpuset(), &topo.present_str, &topo.mems_str) {
        topo.cpuset_enabled = true;
    }

    topo
}

#[cfg(test)]
mod tests {
    use super::*;

    fn present_0_7() -> CpuSet {
        let mut s = CpuSet::new();
        for c in 0..8 {
            s.set(c);
        }
        s
    }

    fn topo_0_7() -> CpuTopology {
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
        CpuTopology {
            present_cpus: present_0_7(),
            present_str: "0-7".to_string(),
            mems_str: "0".to_string(),
            cpuset_enabled: false,
            e_core: e,
            p_core: p,
            hp_core: hp,
        }
    }

    #[test]
    fn bitset_basic() {
        let mut s = CpuSet::new();
        assert_eq!(s.count(), 0);
        s.set(0);
        s.set(3);
        s.set(3); // 幂等
        assert!(s.is_set(0));
        assert!(s.is_set(3));
        assert!(!s.is_set(1));
        assert_eq!(s.count(), 2);
        // 越界写入被忽略
        s.set(CPU_SETSIZE);
        assert!(!s.is_set(CPU_SETSIZE));
    }

    #[test]
    fn bitset_or_merge() {
        let mut a = CpuSet::new();
        a.set(0);
        a.set(4);
        let mut b = CpuSet::new();
        b.set(4);
        b.set(7);
        a.or(&b);
        assert!(a.is_set(0) && a.is_set(4) && a.is_set(7));
        assert_eq!(a.count(), 3);
    }

    #[test]
    fn range_string_single() {
        let mut s = CpuSet::new();
        s.set(0);
        assert_eq!(s.to_range_string(), "0");
        s.set(1);
        s.set(2);
        s.set(3);
        assert_eq!(s.to_range_string(), "0-3");
    }

    #[test]
    fn range_string_mixed() {
        let mut s = CpuSet::new();
        for c in [0usize, 1, 2, 3, 5, 7, 8] {
            s.set(c);
        }
        assert_eq!(s.to_range_string(), "0-3,5,7-8");
    }

    #[test]
    fn range_string_spans_words() {
        // 跨 64 位字边界（63/64/65）
        let mut s = CpuSet::new();
        s.set(63);
        s.set(64);
        s.set(65);
        assert_eq!(s.to_range_string(), "63-65");
        assert_eq!(s.count(), 3);
    }

    #[test]
    fn range_string_empty() {
        assert_eq!(CpuSet::new().to_range_string(), "");
    }

    #[test]
    fn parse_single_and_range() {
        let s = parse_cpu_ranges("0-3,5,7-8", None);
        assert_eq!(s.to_range_string(), "0-3,5,7-8");
    }

    #[test]
    fn parse_reversed_range() {
        // 8-5 视为 5-8
        let s = parse_cpu_ranges("8-5", None);
        assert_eq!(s.to_range_string(), "5-8");
    }

    #[test]
    fn parse_invalid_and_whitespace() {
        let s = parse_cpu_ranges(" 0-3 , 5 , bad , ", None);
        assert_eq!(s.to_range_string(), "0-3,5");
        let s = parse_cpu_ranges("", None);
        assert_eq!(s.count(), 0);
        let s = parse_cpu_ranges("abc", None);
        assert_eq!(s.count(), 0);
    }

    #[test]
    fn parse_out_of_range_clamped() {
        let s = parse_cpu_ranges("0-2000", None);
        // 超过 CPU_SETSIZE 的部分被截断
        assert!(s.is_set(CPU_SETSIZE - 1));
        assert_eq!(s.count(), CPU_SETSIZE);
        // 全越界 → 空
        let s = parse_cpu_ranges("2000-3000", None);
        assert_eq!(s.count(), 0);
    }

    #[test]
    fn parse_present_filter() {
        let present = present_0_7();
        // 请求 5-10，但 present 只有 0-7 → 结果 5-7
        let s = parse_cpu_ranges("5-10", Some(&present));
        assert_eq!(s.to_range_string(), "5-7");
    }

    #[test]
    fn parse_spec_semantic_cores() {
        let topo = topo_0_7();
        assert_eq!(parse_cpu_spec("e-core", &topo).to_range_string(), "0-1");
        assert_eq!(parse_cpu_spec("p-core", &topo).to_range_string(), "2-3");
        assert_eq!(parse_cpu_spec("hp-core", &topo).to_range_string(), "4-7");
        assert_eq!(parse_cpu_spec("all-core", &topo).to_range_string(), "0-7");
    }

    #[test]
    fn parse_spec_mixed() {
        let topo = topo_0_7();
        assert_eq!(
            parse_cpu_spec("e-core,p-core", &topo).to_range_string(),
            "0-3"
        );
        assert_eq!(
            parse_cpu_spec("hp-core,1", &topo).to_range_string(),
            "1,4-7"
        );
        assert_eq!(
            parse_cpu_spec("e-core,7-8", &topo).to_range_string(),
            "0-1,7"
        );
    }

    #[test]
    fn parse_spec_empty_and_unknown() {
        let topo = topo_0_7();
        assert_eq!(parse_cpu_spec("", &topo).count(), 0);
        assert_eq!(parse_cpu_spec("unknown-core", &topo).count(), 0);
    }
}
