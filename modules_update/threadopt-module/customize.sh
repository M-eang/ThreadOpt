SKIPUNZIP=0
check_magisk_version() {
	ui_print "- Module version: $(grep_prop version "${TMPDIR}/module.prop")"
	ui_print "- Module versionCode: $(grep_prop versionCode "${TMPDIR}/module.prop")"
	ui_print "********************************************"
	ui_print "- $(grep_prop description "${TMPDIR}/module.prop")"
	# Magisk 下校验版本；KernelSU 无此变量，跳过（KSU 兼容 Magisk 模块格式）
	if [ -n "$MAGISK_VER_CODE" ] && [ "$MAGISK_VER_CODE" -lt 20400 ]; then
		ui_print "********************************************"
		ui_print "! 请安装 Magisk v20.4+ (20400+)"
		abort    "********************************************"
	fi
}
check_required_files() {
	REQUIRED_FILE_LIST="/sys/devices/system/cpu/present /proc/loadavg"
	for REQUIRED_FILE in $REQUIRED_FILE_LIST; do
		if [ ! -e $REQUIRED_FILE ]; then
			ui_print "********************************************"
			ui_print "! $REQUIRED_FILE 文件不存在"
			ui_print "! 请联系模块作者"
			abort    "********************************************"
		fi
	done
}
extract_bin() {
	ui_print "********************************************"
	if [ "$ARCH" == "arm64" ]; then
		cp $MODPATH/bin/arm64-v8a/ThreadOpt $MODPATH
		# eBPF 内核态程序需与主程序同目录配套（ebpf_mode.rs 按 current_exe 同目录查找）
		cp $MODPATH/bin/arm64-v8a/ThreadOpt-ebpf $MODPATH
	else
		abort "! Unsupported platform: $ARCH"
	fi
	ui_print "- Device platform: $ARCH"
	rm -rf $MODPATH/bin
	[ -f $MODPATH/ThreadOpt ] && chmod a+x $MODPATH/ThreadOpt
	[ -f $MODPATH/ThreadOpt-ebpf ] && chmod a+x $MODPATH/ThreadOpt-ebpf
	# 校验 ELF 魔数，确保包未损坏
	if [ "$(head -c 4 $MODPATH/ThreadOpt 2>/dev/null | od -An -tx1 | tr -d ' \n')" != "7f454c46" ]; then
		abort "! 主程序不是有效的 ELF 文件，请检查模块zip是否损坏"
	fi
	if [ "$(head -c 4 $MODPATH/ThreadOpt-ebpf 2>/dev/null | od -An -tx1 | tr -d ' \n')" != "7f454c46" ]; then
		abort "! eBPF 程序不是有效的 ELF 文件，请检查模块zip是否损坏"
	fi
	# 自检：KernelSU metainstall 阶段 exec 可能受限，失败仅警告，重启后由 service.sh 真正执行
	if ! $MODPATH/ThreadOpt -v; then
		ui_print "! 警告: ThreadOpt -v 自检失败 (exit=$?)"
		ui_print "! 文件: $(ls -la $MODPATH/ThreadOpt 2>&1)"
		ui_print "! 上下文: $(ls -Z $MODPATH/ThreadOpt 2>&1)"
	fi
}
module_instructions() {
	ui_print "********************************************"
	ui_print "线程规则配置文件路径为："
	ui_print "/data/adb/modules/ThreadOpt/applist.conf"
	ui_print "------------------------------------------"
	ui_print "游戏档配置（前台检测到游戏自动切换）："
	ui_print "/data/adb/modules/ThreadOpt/game.conf"
	ui_print "------------------------------------------"
	ui_print "手动锁定档位："
	ui_print "/data/adb/modules/ThreadOpt/mode  内容 auto/power/game"
	ui_print "------------------------------------------"
	ui_print "修改与添加规则无需重启，即时生效"
	ui_print "********************************************"
	all_core="$(cat /sys/devices/system/cpu/present)"
	ui_print "当前$(getprop ro.soc.model)设备为$(nproc)核CPU"
	ui_print "可用CPU范围：$all_core"
	ui_print "------------------------------------------"
	ui_print "规则写法示例："
	ui_print "surfaceflinger=$all_core"
	ui_print "com.tencent.mm=e-core"
	ui_print "com.android.systemui{RenderThread}=hp-core"
	ui_print "------------------------------------------"
	ui_print "更多规则使用说明请参考："
	ui_print "http://AppOpt.suto.top"
	ui_print "********************************************"
}
add_default_rules() {
if [ -f /data/adb/modules/ThreadOpt/applist.conf ]; then
	mv $MODPATH/applist.conf $MODPATH/applist.conf.bak
	cp -r /data/adb/modules/ThreadOpt/applist.conf $MODPATH
fi
# 游戏档配置同样保留用户已有文件，避免升级覆盖
if [ -f /data/adb/modules/ThreadOpt/game.conf ]; then
	mv $MODPATH/game.conf $MODPATH/game.conf.bak
	cp -r /data/adb/modules/ThreadOpt/game.conf $MODPATH
fi
}
check_magisk_version
check_required_files
extract_bin
module_instructions
add_default_rules
set_perm_recursive "$MODPATH" 0 0 0755 0644
set_perm_recursive "$MODPATH/*.sh $MODPATH/ThreadOpt" 0 2000 0755 0755 u:object_r:magisk_file:s0
