#!/usr/bin/env python3
"""ThreadOpt Magisk/KernelSU 模块打包脚本

用法:
  python pack_module.py [版本号]            # 仅打包（需自行构建好二进制）
  python pack_module.py [版本号] --build    # 构建（主程序+eBPF）+ 拷贝 + 打包
产物: modules_update/ThreadOpt-v<版本号>-module.zip

注意: 必须用本脚本打包（zipfile 正斜杠 + Unix 权限位），
PowerShell Compress-Archive 打出的 zip 路径分隔符是反斜杠，KernelSU 无法正确解压。

--build 依赖:
  1. NDK r27d 链接器配置在 ThreadOpt/.cargo/config.toml（aarch64-linux-android）
  2. rustup nightly 工具链 + rust-src 组件（eBPF 用 -Z build-std=core）
  3. bpf-linker 预编译版（默认 D:\\Desktop\\tools\\bpf-linker\\bpf-linker.exe，
     可用环境变量 BPF_LINKER 覆盖；下载: github.com/aya-rs/bpf-linker/releases）
"""
import argparse
import os
import shutil
import subprocess
import sys
import zipfile

HERE = os.path.dirname(os.path.abspath(__file__))
SRC = os.path.join(HERE, "threadopt-module")
THREADOPT_DIR = os.path.normpath(os.path.join(HERE, "..", "ThreadOpt"))
EBPF_DIR = os.path.normpath(os.path.join(HERE, "..", "ThreadOpt-ebpf"))
BIN_DIR = os.path.join(SRC, "bin", "arm64-v8a")
DEFAULT_BPF_LINKER = r"D:\Desktop\tools\bpf-linker\bpf-linker.exe"

MAIN_BIN_SRC = os.path.join(
    THREADOPT_DIR, "target", "aarch64-linux-android", "release", "ThreadOpt"
)
EBPF_BIN_SRC = os.path.join(
    EBPF_DIR, "target", "bpfel-unknown-none", "release", "ThreadOpt-ebpf"
)

# 需要 0755 权限的文件（其余 0644）
EXECUTABLE = {"ThreadOpt", "ThreadOpt-ebpf", "service.sh", "customize.sh", "update-binary"}


def build_main() -> None:
    """交叉编译 Android 主程序（aarch64），链接器来自 ThreadOpt/.cargo/config.toml"""
    print(">>> 交叉编译主程序 (aarch64-linux-android) ...")
    subprocess.run(
        ["cargo", "build", "--release", "--target", "aarch64-linux-android"],
        cwd=THREADOPT_DIR,
        check=True,
    )
    if not os.path.exists(MAIN_BIN_SRC):
        sys.exit(f"错误: 主程序产物不存在: {MAIN_BIN_SRC}")


def build_ebpf() -> None:
    """编译 eBPF 内核态程序（bpfel-unknown-none，-Z build-std=core + bpf-linker）"""
    bpf_linker = os.environ.get("BPF_LINKER", DEFAULT_BPF_LINKER)
    if not os.path.exists(bpf_linker):
        sys.exit(
            f"错误: 找不到 bpf-linker: {bpf_linker}\n"
            "请下载预编译版或设置环境变量 BPF_LINKER 指向它"
        )
    print(f">>> 编译 eBPF 内核态 (bpfel-unknown-none, linker={bpf_linker}) ...")
    # TOML 字符串中反斜杠需转义，统一转正斜杠（Windows 程序同样接受）
    linker_arg = bpf_linker.replace("\\", "/")
    cfg = f'target.bpfel-unknown-none.linker="{linker_arg}"'
    subprocess.run(
        [
            "cargo", "+nightly", "build", "-Z", "build-std=core",
            "--target", "bpfel-unknown-none", "--release",
            "--config", cfg,
        ],
        cwd=EBPF_DIR,
        check=True,
    )
    if not os.path.exists(EBPF_BIN_SRC):
        sys.exit(f"错误: eBPF 产物不存在: {EBPF_BIN_SRC}")


def copy_bins() -> None:
    """拷贝两二进制进模块目录（部署要求: 主程序与 ThreadOpt-ebpf 同目录配套）"""
    os.makedirs(BIN_DIR, exist_ok=True)
    shutil.copy2(MAIN_BIN_SRC, os.path.join(BIN_DIR, "ThreadOpt"))
    shutil.copy2(EBPF_BIN_SRC, os.path.join(BIN_DIR, "ThreadOpt-ebpf"))
    print(f">>> 二进制已就位: {BIN_DIR} (ThreadOpt {os.path.getsize(os.path.join(BIN_DIR, 'ThreadOpt'))}B, "
          f"ThreadOpt-ebpf {os.path.getsize(os.path.join(BIN_DIR, 'ThreadOpt-ebpf'))}B)")


def pack(version: str) -> None:
    out = os.path.join(HERE, f"ThreadOpt-v{version}-module.zip")
    if os.path.exists(out):
        os.remove(out)
    with zipfile.ZipFile(out, "w", zipfile.ZIP_DEFLATED) as z:
        for root, _dirs, files in os.walk(SRC):
            for f in files:
                full = os.path.join(root, f)
                rel = os.path.relpath(full, SRC).replace("\\", "/")
                info = zipfile.ZipInfo(rel)
                mode = 0o755 if f in EXECUTABLE else 0o644
                info.external_attr = (mode & 0xFFFF) << 16
                info.compress_type = zipfile.ZIP_DEFLATED
                with open(full, "rb") as fh:
                    z.writestr(info, fh.read())
    print(f"✅ {out} ({os.path.getsize(out)} bytes)")


def main() -> None:
    parser = argparse.ArgumentParser(description="ThreadOpt 模块打包脚本")
    parser.add_argument("version", nargs="?", default="2.2.0", help="版本号 (默认 2.2.0)")
    parser.add_argument("--build", action="store_true", help="构建主程序+eBPF 并拷贝后打包")
    args = parser.parse_args()

    if args.build:
        build_main()
        build_ebpf()
        copy_bins()
    pack(args.version)


if __name__ == "__main__":
    main()
