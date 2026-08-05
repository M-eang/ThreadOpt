# ThreadOpt

#### 介绍
安卓应用 CPU 亲和性优化程序 - 基于 [AppOpt](https://gitee.com/sutoliu/AppOpt) 的 Rust 重构版（独立维护）

 **详细使用文档：[docs/使用文档.md](docs/使用文档.md)**（安装、规则语法、通配符、语义核心、验证排障）

 **使用说明请参考** 

http://appopt.suto.top

### 模块说明

| 模块 | 功能 |
|------|------|
| `main.rs` | 程序入口，CLI 解析与 eBPF/proc 双模式主循环编排 |
| `config.rs` | 配置文件解析（块语法/语义核心名）、inotify 热加载与降级轮询 |
| `cpuset.rs` | CpuSet 位图、CPU 拓扑检测与 cpuset 目录管理 |
| `rule_match.rs` | 包名/线程名规则匹配与 comm 到包名映射 |
| `apply_affinity.rs` | 亲和性应用与 `/proc` 文件读取 |
| `cache.rs` | 统一进程缓存 |
| `ebpf_mode.rs` | eBPF 事件驱动 |
| `proc_mode.rs` | proc 轮询模式 |
| `ThreadOpt-ebpf` | eBPF 内核态：4 事件 tracepoint + 白名单前置过滤 |





### 基本语法

```ini
# 注释行（# 或 // 开头）

# 包名 = CPU范围                   → 匹配该包的所有线程
com.example.app=4-5

# 包名 { 线程名 = CPU范围 }          → 匹配该包的特定线程
com.example.game {
    main_thread=0-3
    render_*=4-5
    worker_?=6
}

# 紧凑单行：包名 { 线程名 } = CPU范围
com.example.app{heavy_thread}=6-7

# 块内同时放包级规则 + 线程规则
com.example.app=0-3 {
    bg_thread=4-5
}
```

### 线程名通配符

| 符号 | 含义 | 示例 |
|------|------|------|
| `*` | 匹配任意字符序列 | `render_*` 匹配 `render_thread`、`render_worker` |
| `?` | 匹配单个字符 | `worker_?` 匹配 `worker_1`、`worker_A` |
| `[范围]` | 匹配集合中任一字符 | `thread_[0-9]` 匹配 `thread_0` ~ `thread_9` |

### CPU 范围

```ini
0          # 单个 CPU
0-3        # 连续范围
0-3,5,7-8  # 逗号分隔
```

### 语义核心名（核心自适应）

规则中的 CPU 范围可使用语义名，自动展开为实际 CPU 编号：

| 语义名 | 含义 |
|--------|------|
| `e-core` | 能效小核（最低频率簇） |
| `p-core` | 性能中核（中间频率簇，多簇合并） |
| `hp-core` | 高性能大核（最高频率簇） |
| `all-core` | 所有核心 |

语义名可与数字范围混用，逗号分隔取并集后压缩为范围：

```ini
# 6+2 拓扑(高通8 Elite：e=0-5, hp=6-7)：e-core,p-core 展开为 0-5，hp-core 展开为 6-7
com.tencent.tmgp.sgame=e-core,p-core {
    UnityMain=hp-core
    UnityGfxDeviceW=p-core,hp-core
}
# 等价于
com.tencent.tmgp.sgame=0-5 {
    UnityMain=6-7
    UnityGfxDeviceW=6-7
}
```

分层规则：按 `cpuinfo_max_freq` 升序分组，首组为 `e-core`，末组为 `hp-core`，中间所有组合并为 `p-core`；仅两簇时 `p-core` 为空。

## 开发与测试

Windows 上可直接运行核心逻辑单元测试（无需 Linux 环境）：

```bash
cargo test --lib --no-default-features   # 32 个用例：规则匹配 / 配置解析 / CPU 位图
cargo fmt --check                        # 格式检查
```

`ebpf` feature（默认启用，依赖 aya）仅在 Linux/Android 上编译，CI 每次推送会自动跑全量测试与发布构建。

### 本地交叉编译 Android 版（Windows + NDK）

已配置 NDK（`D:\android-ndk-r27d`）时，无需 Linux 环境即可直接编译出 ARM64 版：

```bash
rustup target add aarch64-linux-android
cd ThreadOpt && cargo build --release --target aarch64-linux-android
# 产物：target/aarch64-linux-android/release/ThreadOpt（ELF aarch64, Android 21+）
```

链接器路径配置在 `ThreadOpt/.cargo/config.toml`（含本机绝对路径，已加入 .gitignore 不提交）。

### 构建 Magisk/KernelSU 模块包

```bash
cd modules_update
python pack_module.py 2.1.3   # 产出 ThreadOpt-v2.1.3-module.zip
```

⚠️ 必须用 `pack_module.py` 打包：PowerShell `Compress-Archive` 打出的 zip 路径分隔符是 `\`，KernelSU/Magisk 无法正确解压（真机踩过坑）。模块已实测通过 KernelSU 安装、开机自启与规则热加载。

## 命令行参数

| 参数 | 说明 | 默认值 |
|------|------|--------|
| `-c <file>` | 指定配置文件路径 | `./applist.conf` |
| `-s <seconds>` | 检查间隔（秒，≥1） | `2` |
| `-b <name>` | 指定 BASE_CPUSET 目录名（不可含 `/`） | `ThreadOpt` |
| `-v` | 显示版本信息 | — |
| `-h` | 显示帮助 | — |
