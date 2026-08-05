#![cfg(any(target_os = "linux", target_os = "android"))]
#[cfg(target_pointer_width = "32")]
compile_error!("ThreadOpt requires 64-bit target due to cpu_set_t binary layout assumptions");

mod apply_affinity;
mod cache;
mod ebpf_mode;
mod foreground;
mod proc_mode;

use std::collections::HashSet;
use std::env;
use std::ffi::CString;
use std::fs;
use std::path::{Path, PathBuf};
use std::process;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::thread;
use std::time::{Duration, Instant};

#[cfg(feature = "ebpf")]
use crate::ebpf_mode::{
    EbpfState, affinity_check, comm_map_init, ebpf_init, event_dispatch, full_scan,
};
use crate::proc_mode::{ProcScanState, cache_sync};
use threadopt::CONFIG_UPDATED;
use threadopt::config::{CURRENT_CONFIG, config_loader, init_inotify, load_config};
use threadopt::cpuset::{DEFAULT_CPUSET_NAME, init_cpu_topo, set_base_cpuset};
use threadopt::lock_ignore_poison;
use threadopt::mode::{Mode, decide_mode, parse_override};

/// eBPF 失效后每隔该秒数重试恢复，避免永久退化为 /proc 轮询
const EBPF_RETRY_SECS: u64 = 60;

/// 档位检测线程写：目标档位
static CURRENT_MODE: Mutex<Mode> = Mutex::new(Mode::Power);
/// 主循环写：当前生效档位
static EFFECTIVE_MODE: Mutex<Mode> = Mutex::new(Mode::Power);
/// 目标档位变化时检测线程置位，主循环消费
static MODE_CHANGED: AtomicBool = AtomicBool::new(false);
/// 游戏名单（前台检测用），主循环在游戏配置加载/热加载/切档时更新
static GAME_PKGS: Mutex<Option<Arc<HashSet<String>>>> = Mutex::new(None);

fn print_help(prog_name: &str) {
    println!("Usage: {} [OPTIONS]", prog_name);
    println!("Options:");
    println!("  -c <config_file>   指定省电档配置文件 (默认: ./applist.conf)");
    println!("  -g <game_config>   指定游戏档配置文件 (默认: ./game.conf，不存在则档位切换禁用)");
    println!("  -s <interval>      设置检查间隔(秒) (必须>=1, 默认: 2)");
    println!("  -b <cpuset_name>   指定 BASE_CPUSET 目录名 (默认: ThreadOpt)");
    println!("  -v                 显示程序版本");
    println!("  -h                 显示帮助信息");
    println!();
    println!("示例:");
    println!("  {} -c /data/applist.conf -s 3", prog_name);
    println!("  {} -b MyThreadOpt", prog_name);
    println!();
    println!("档位切换（自动为主 + 手动兜底）:");
    println!("  前台检测到 game.conf 中的游戏 → 自动切游戏档；回桌面自动回省电档");
    println!("  手动锁定：在配置文件同目录建 mode 文件，内容 auto/power/game");
    println!();
    println!("规则格式:");
    println!("  # 注释以 # 或 // 开头");
    println!("  com.example=0-3           包级规则，绑定到 CPU 0-3");
    println!("  com.example=e-core        语义核心，绑定到全部小核");
    println!("  com.example=p-core        语义核心，绑定到全部中核");
    println!("  com.example=hp-core       语义核心，绑定到全部大核");
    println!();
    println!("  块语法，包级规则 + 线程规则");
    println!("  com.example {{");
    println!("    RenderThread=6-7");
    println!("    Thread-1=0-5");
    println!("  }}");
    println!("  线程 RenderThread 绑定到 CPU 6-7");
    println!("  线程 Thread-1 绑定到 CPU 0-5");
}

/// 档位检测线程：前台游戏检测 + 手动 override（mode 文件），档位变化时置 MODE_CHANGED
fn spawn_mode_detector(mode_file: PathBuf, det_interval: Duration) -> thread::JoinHandle<()> {
    thread::spawn(move || {
        let name = CString::new("ModeDetect").unwrap();
        unsafe {
            libc::pthread_setname_np(libc::pthread_self(), name.as_ptr());
        }
        let mut last_mode = Mode::Power;
        loop {
            thread::sleep(det_interval);
            // 手动 override 优先（mode 文件内容 auto/power/game，缺失或无法识别视为 auto）
            let override_mode = fs::read_to_string(&mode_file)
                .ok()
                .and_then(|c| parse_override(&c));
            let game_fg = lock_ignore_poison(&GAME_PKGS)
                .as_ref()
                .map(|pkgs| foreground::game_foreground(pkgs))
                .unwrap_or(false);
            let target = decide_mode(override_mode, game_fg);
            if target != last_mode {
                last_mode = target;
                *lock_ignore_poison(&CURRENT_MODE) = target;
                MODE_CHANGED.store(true, Ordering::Release);
                println!(
                    "档位检测: 切换到 {:?}（前台游戏={}，override={:?}）",
                    target, game_fg, override_mode
                );
            }
        }
    })
}

fn main() {
    let args: Vec<String> = env::args().collect();
    let prog_name = &args[0];

    let mut config_file = String::from("./applist.conf");
    let mut game_file = String::from("./game.conf");
    let mut sleep_interval: u64 = 2;
    let mut cpuset_name = String::from(DEFAULT_CPUSET_NAME);

    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "-c" => {
                i += 1;
                if i < args.len() {
                    config_file = args[i].clone();
                    println!("配置文件: {}", config_file);
                } else {
                    eprintln!("错误: -c 需要指定配置文件路径");
                    process::exit(1);
                }
            }
            "-g" => {
                i += 1;
                if i < args.len() {
                    game_file = args[i].clone();
                    println!("游戏档配置文件: {}", game_file);
                } else {
                    eprintln!("错误: -g 需要指定配置文件路径");
                    process::exit(1);
                }
            }
            "-s" => {
                i += 1;
                if i < args.len() {
                    let val: u64 = match args[i].parse() {
                        Ok(v) if v >= 1 => v,
                        _ => {
                            eprintln!("无效的时间间隔: {}", args[i]);
                            eprintln!("间隔必须是 >=1 的整数");
                            process::exit(1);
                        }
                    };
                    sleep_interval = val;
                    println!("检查间隔: {} 秒", sleep_interval);
                } else {
                    eprintln!("错误: -s 需要指定时间间隔");
                    process::exit(1);
                }
            }
            "-b" => {
                i += 1;
                if i < args.len() {
                    cpuset_name = args[i].clone();
                    if cpuset_name.is_empty() || cpuset_name.contains('/') {
                        eprintln!("无效的 cpuset 目录名: {}", args[i]);
                        eprintln!("目录名不能为空或包含路径分隔符");
                        process::exit(1);
                    }
                    println!("cpuset 目录名: {}", cpuset_name);
                } else {
                    eprintln!("错误: -b 需要指定 cpuset 目录名");
                    process::exit(1);
                }
            }
            "-v" => {
                #[cfg(feature = "ebpf")]
                let ebpf_ok = crate::ebpf_mode::ebpf_probe();
                #[cfg(not(feature = "ebpf"))]
                let ebpf_ok = false;
                if ebpf_ok {
                    println!("ThreadOpt 版本 {} eBPF", env!("CARGO_PKG_VERSION"));
                } else {
                    println!("ThreadOpt 版本 {}", env!("CARGO_PKG_VERSION"));
                }
                process::exit(0);
            }
            "-h" => {
                print_help(prog_name);
                process::exit(0);
            }
            other => {
                eprintln!("未知选项: {}", other);
                print_help(prog_name);
                process::exit(1);
            }
        }
        i += 1;
    }

    // 先设置 cpuset 路径再初始化拓扑，init_cpu_topo 会创建 BASE_CPUSET 目录
    set_base_cpuset(&cpuset_name);
    let topo = init_cpu_topo();

    if fs::metadata(&config_file).is_err() {
        let initial_content = "# 规则编写与使用说明请参考 http://AppOpt.suto.top\n\n";
        if fs::write(&config_file, initial_content).is_ok() {
            println!("配置文件不存在，重建一个空的配置文件: {}", config_file);
        }
    }

    let mut tmp_mtime: i64 = -1;
    let initial_config = match load_config(&config_file, &topo, &mut tmp_mtime) {
        Some(cfg) => cfg,
        None => {
            eprintln!("初始配置加载失败");
            process::exit(1);
        }
    };

    {
        let mut guard = lock_ignore_poison(&CURRENT_CONFIG);
        *guard = Some(Arc::new(initial_config));
    }
    CONFIG_UPDATED.store(true, Ordering::Release);

    init_inotify(&config_file);

    // 绑核日志：写入 <配置文件目录>/logs/apply.log（排障/验证规则命中用）
    crate::cache::init_apply_log(&config_file);

    // 游戏档配置（可选）：存在则启用档位切换，包名名单供前台检测使用
    let mut game_mtime: i64 = -1;
    if fs::metadata(&game_file).is_ok() {
        if let Some(game_cfg) = load_config(&game_file, &topo, &mut game_mtime) {
            *lock_ignore_poison(&GAME_PKGS) = Some(Arc::new(game_cfg.pkgs.clone()));
            println!(
                "游戏档配置: {}（{} 个游戏包），前台自动检测已启用",
                game_file,
                game_cfg.pkgs.len()
            );
        } else {
            eprintln!("警告: 游戏档配置 {} 解析失败，档位切换禁用", game_file);
        }
    } else {
        println!("未找到游戏档配置 {}，档位切换禁用（仅省电档）", game_file);
    }

    // 档位检测线程：前台游戏检测 + 手动 override（mode 文件），档位变化时置 MODE_CHANGED
    let mode_file = Path::new(&config_file)
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join("mode");
    let det_interval = Duration::from_secs((2 * sleep_interval).max(2));
    let mut mode_handle = spawn_mode_detector(mode_file.clone(), det_interval);

    // 守护进程模式，保存 JoinHandle 用于 panic 恢复检测
    let mut config_handle = thread::spawn(move || {
        config_loader(sleep_interval);
    });

    let mut proc_state: Option<ProcScanState> = None;
    let mut affinity_deadline = Instant::now();

    println!("启动ThreadOpt服务 v{}", env!("CARGO_PKG_VERSION"));

    #[cfg(feature = "ebpf")]
    let mut ebpf_state: Option<EbpfState> = ebpf_init();
    #[cfg(not(feature = "ebpf"))]
    let mut ebpf_state: Option<()> = None;
    let mut last_ebpf_retry = Instant::now();
    // 切档失败日志节流：30 秒内只打印一次，避免损坏文件时每轮刷屏
    let mut last_mode_fail_log = Instant::now();

    loop {
        // 先 swap CONFIG_UPDATED 再获取 cfg 防止漏更新
        let config_changed = CONFIG_UPDATED.swap(false, Ordering::AcqRel);
        let Some(cfg) = lock_ignore_poison(&CURRENT_CONFIG).clone() else {
            thread::sleep(Duration::from_millis(100));
            continue;
        };

        // 档位切换：目标档位变化时现场加载对应配置并替换生效配置
        if MODE_CHANGED.swap(false, Ordering::AcqRel) {
            let target = *lock_ignore_poison(&CURRENT_MODE);
            let effective = *lock_ignore_poison(&EFFECTIVE_MODE);
            if target != effective {
                let target_file = if target == Mode::Game {
                    &game_file
                } else {
                    &config_file
                };
                // 现场强制加载（mtime=-1），成功后下一轮按 CONFIG_UPDATED 重建白名单/全量扫描
                let mut dummy_mtime: i64 = -1;
                if let Some(new_cfg) = load_config(target_file, &cfg.topo, &mut dummy_mtime) {
                    if target == Mode::Game {
                        *lock_ignore_poison(&GAME_PKGS) = Some(Arc::new(new_cfg.pkgs.clone()));
                    }
                    *lock_ignore_poison(&CURRENT_CONFIG) = Some(Arc::new(new_cfg));
                    CONFIG_UPDATED.store(true, Ordering::Release);
                    *lock_ignore_poison(&EFFECTIVE_MODE) = target;
                    println!("档位生效: {:?}（配置 {}）", target, target_file);
                } else {
                    if last_mode_fail_log.elapsed() >= Duration::from_secs(30) {
                        eprintln!(
                            "警告: 加载 {:?} 档配置 {} 失败，保持当前档位（每30秒重试）",
                            target, target_file
                        );
                        last_mode_fail_log = Instant::now();
                    }
                    // 保留目标档位并重新置位，下轮重试，避免"文件损坏+游戏在前台"时永久失配
                    MODE_CHANGED.store(true, Ordering::Release);
                }
            }
        }

        // 一致性自愈：切档与 config_loader 热加载交错时（最后写者赢）可能出现
        // CURRENT_CONFIG 与 EFFECTIVE_MODE 失配，每轮校验并强制恢复
        {
            let expected_file = if *lock_ignore_poison(&EFFECTIVE_MODE) == Mode::Game {
                game_file.as_str()
            } else {
                config_file.as_str()
            };
            let cfg_file = lock_ignore_poison(&CURRENT_CONFIG)
                .as_ref()
                .map(|c| c.config_file.clone());
            if cfg_file.as_deref() != Some(expected_file) {
                let mut dummy_mtime: i64 = -1;
                if let Some(new_cfg) = load_config(expected_file, &cfg.topo, &mut dummy_mtime) {
                    *lock_ignore_poison(&CURRENT_CONFIG) = Some(Arc::new(new_cfg));
                    CONFIG_UPDATED.store(true, Ordering::Release);
                } else {
                    eprintln!(
                        "警告: 档位一致性自愈失败，无法加载 {}（可能已损坏或被删除）",
                        expected_file
                    );
                }
            }
        }

        // game.conf 热加载检查：省电档时 config_loader 只监控主配置，游戏配置
        // 变更由主循环轮询 mtime 感知（切到游戏档后同时替换生效配置并重建白名单）
        if fs::metadata(&game_file).is_ok() {
            if let Some(new_game) = load_config(&game_file, &cfg.topo, &mut game_mtime) {
                let new_pkgs = new_game.pkgs.clone();
                println!("游戏档配置已热加载: {} 个游戏包", new_pkgs.len());
                *lock_ignore_poison(&GAME_PKGS) = Some(Arc::new(new_pkgs));
                if *lock_ignore_poison(&EFFECTIVE_MODE) == Mode::Game {
                    *lock_ignore_poison(&CURRENT_CONFIG) = Some(Arc::new(new_game));
                    CONFIG_UPDATED.store(true, Ordering::Release);
                }
            }
        }

        // 配置加载线程 panic 恢复
        if config_handle.is_finished() {
            eprintln!("警告: 配置加载线程异常退出，尝试重启...");
            config_handle = thread::spawn(move || {
                config_loader(sleep_interval);
            });
        }

        // 档位检测线程 panic 恢复
        if mode_handle.is_finished() {
            eprintln!("警告: 档位检测线程异常退出，尝试重启...");
            mode_handle = spawn_mode_detector(mode_file.clone(), det_interval);
        }

        #[cfg(feature = "ebpf")]
        let mut ebpf_dead = false;

        #[cfg(feature = "ebpf")]
        let need_reload = if let Some(es) = ebpf_state.as_mut() {
            if config_changed {
                let r = comm_map_init(&mut es.bpf, &cfg.pkgs, es.comm_capacity);
                if !r {
                    full_scan(&cfg, es);
                }
                r
            } else {
                false
            }
        } else {
            false
        };
        #[cfg(not(feature = "ebpf"))]
        let need_reload = false;

        #[cfg(feature = "ebpf")]
        if need_reload {
            ebpf_state = None;
            if let Some(mut new_es) = ebpf_init() {
                if comm_map_init(&mut new_es.bpf, &cfg.pkgs, new_es.comm_capacity) {
                    eprintln!("eBPF: 重载后白名单容量仍不足，回退到 /proc 轮询");
                    continue;
                }
                full_scan(&cfg, &mut new_es);
                ebpf_state = Some(new_es);
            }
            continue;
        }

        #[cfg(feature = "ebpf")]
        if let Some(es) = ebpf_state.as_mut() {
            match es.event_rx.recv_timeout(Duration::from_secs(1)) {
                Ok(event) => {
                    event_dispatch(&event, &cfg, es);
                    while let Ok(event) = es.event_rx.try_recv() {
                        event_dispatch(&event, &cfg, es);
                    }
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    ebpf_dead = true;
                }
            }

            if affinity_deadline.elapsed() >= Duration::from_secs(3 * sleep_interval) {
                affinity_check(es, &cfg);
                affinity_deadline = Instant::now();
            }
        } else {
            // eBPF 不可用（初始化失败/通道断开/重载失败），回退 /proc 轮询；
            // 每 EBPF_RETRY_SECS 秒重试恢复，成功后自动切回事件驱动
            if last_ebpf_retry.elapsed() >= Duration::from_secs(EBPF_RETRY_SECS) {
                last_ebpf_retry = Instant::now();
                if let Some(mut new_es) = ebpf_init() {
                    if comm_map_init(&mut new_es.bpf, &cfg.pkgs, new_es.comm_capacity) {
                        eprintln!("eBPF: 自愈后白名单容量仍不足，继续 /proc 轮询");
                    } else {
                        full_scan(&cfg, &mut new_es);
                        ebpf_state = Some(new_es);
                        println!("eBPF: 自愈成功，恢复事件驱动模式");
                    }
                }
            }
            let cache = proc_state.get_or_insert_with(ProcScanState::new);
            if config_changed {
                cache.scan_all_proc = true;
                cache.last_proc_count = 0;
            }
            cache_sync(cache, &cfg);
            if affinity_deadline.elapsed() >= Duration::from_secs(5 * sleep_interval)
                || cache.force_affinity
            {
                cache.cache.affinity_sync(&cfg.topo);
                affinity_deadline = Instant::now();
                cache.force_affinity = false;
            }
            thread::sleep(Duration::from_secs(sleep_interval));
        }

        #[cfg(not(feature = "ebpf"))]
        {
            // 纯 /proc 轮询模式（无 eBPF 支持构建时）
            let cache = proc_state.get_or_insert_with(ProcScanState::new);
            if config_changed {
                cache.scan_all_proc = true;
                cache.last_proc_count = 0;
            }
            cache_sync(cache, &cfg);
            if affinity_deadline.elapsed() >= Duration::from_secs(5 * sleep_interval)
                || cache.force_affinity
            {
                cache.cache.affinity_sync(&cfg.topo);
                affinity_deadline = Instant::now();
                cache.force_affinity = false;
            }
            thread::sleep(Duration::from_secs(sleep_interval));
        }

        #[cfg(feature = "ebpf")]
        if ebpf_dead {
            eprintln!("eBPF: 事件通道断开，回退到 /proc 轮询");
            ebpf_state = None;
            let cache = proc_state.get_or_insert_with(ProcScanState::new);
            cache.scan_all_proc = true;
            cache.last_proc_count = 0;
            cache.force_affinity = true;
            affinity_deadline = Instant::now();
        }
    }
}
