### v2.2.1（ThreadOpt 独立维护版）
- **绑核日志**：每次绑核写入 logs/apply.log（1MB 滚动），验证规则命中与排障用
- **移除防云控**：删除 Joyose teg 冻结功能（真机实测冻结会被 Joyose 自愈重写，不可靠且与游戏闪退无关）
- 版本 2.2.1 / versionCode 162

### v2.2.0（ThreadOpt 独立维护版）
- **eBPF 模式正式启用**：发布包配套 ThreadOpt-ebpf 内核态程序（事件驱动，进程/线程创建即绑核，无 proc 轮询延迟）
- 真机验证通过（天玑 9200+ / KernelSU / SELinux Enforcing）：BTF 识别、tracepoint 挂载（fork/exec/exit/rename）、规则应用全部生效
- 构建链打通：eBPF 内核态用 nightly + build-std + bpf-linker 编译（bpfel-unknown-none）
- CI 新增 eBPF 内核态编译 job（ubuntu，产物可下载）
- pack_module.py 支持 `--build` 一键构建（主程序 + eBPF）+ 打包
- 版本 2.2.0 / versionCode 161

### v2.1.3（ThreadOpt 独立维护版）
- 重构为 lib + bin 结构：核心逻辑（规则匹配/配置解析/CPU 位图）可跨平台单元测试（32 用例）
- 新增 GitHub Actions CI：格式检查 + 全量测试 + Android 交叉编译 + release 构建
- 线程名匹配改为跨平台自研实现（支持 `*`/`?`/`[0-9]`），行为与 glibc fnmatch 对齐
- 修复：inotify_rm_watch 在 glibc/bionic 的签名差异（Android 编译失败问题）
- 模块包支持 KernelSU 安装（兼容 Magisk 模块格式）
- 真机验证：通配符匹配、语义核心展开、配置热加载、开机自启全部通过

### v2.1.3（原作者）
- 主程序：修复已知问题
- 主程序：性能优化

### v2.1.2更新日志
- 主程序：支持在applist.conf中使用"语义核心"，可与范围核心随意混用
```
- e-core 表示能效小核
- p-core 表示性能中核
- hp-core 为高性能大核
- all-core 表示所有核心
 也可以组合一起用（英文逗号）：
- e-core,p-core  为小核与中核
- p-core,hp-core 为中核与大核
```
- 主程序：支持使用-b参数自定义/dev/cpuset目录，防止被某些程序检测
```
- 使用方法：例如AppOpt -b MyAppOpt则目录会变成/dev/cpuset/MyAppOpt
```
- 主程序：修复了已知问题
- 主程序：完善 eBPF 支持
- 主程序：性能优化，内存与CPU占用更低


### v2.0.1更新日志
- 主程序：使用Rust重构，性能大幅增强
- 主程序：增加 eBPF 支持，优先使用 eBPF 事件驱动，不支持则自动回退到proc模式
- 主程序：支持了新的规则格式，同时兼容旧版格式，可随意混用
- 主程序：支持使用//或者#进行规则注释
- 主程序：修复了已知问题
