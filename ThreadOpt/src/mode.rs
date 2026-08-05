//! 档位（模式）切换纯逻辑：省电档（默认日常）与游戏档
//!
//! 与平台无关的部分（档位枚举、手动 override 解析、切换决策）放在 lib，
//! 可在 Windows 上编译与测试；/proc 前台检测等平台相关代码在 bin 侧。

/// 运行档位
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// 省电档（默认日常）：前台宽松 + 后台收紧，低功耗且保持流畅
    Power,
    /// 游戏档：帧率优先，游戏线程全上性能核，功耗不考虑
    Game,
}

impl Mode {
    pub fn as_str(self) -> &'static str {
        match self {
            Mode::Power => "power",
            Mode::Game => "game",
        }
    }
}

/// 手动 override 内容：auto = 跟随前台自动检测；power/game = 强制锁定档位
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Override {
    Auto,
    Mode(Mode),
}

/// 解析 override 文件内容（忽略大小写与首尾空白）。
/// 空内容或 "auto" 视为 Auto；无法识别返回 None（调用方按 Auto 处理）。
pub fn parse_override(content: &str) -> Option<Override> {
    match content.trim().to_ascii_lowercase().as_str() {
        "" | "auto" => Some(Override::Auto),
        "power" | "battery" | "eco" => Some(Override::Mode(Mode::Power)),
        "game" | "performance" | "gaming" => Some(Override::Mode(Mode::Game)),
        _ => None,
    }
}

/// 档位决策：手动 override 优先；auto（或未识别）时按前台是否命中游戏名单
pub fn decide_mode(override_mode: Option<Override>, game_foreground: bool) -> Mode {
    match override_mode {
        Some(Override::Mode(m)) => m,
        _ => {
            if game_foreground {
                Mode::Game
            } else {
                Mode::Power
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_override_auto_and_empty() {
        assert_eq!(parse_override(""), Some(Override::Auto));
        assert_eq!(parse_override("auto"), Some(Override::Auto));
        assert_eq!(parse_override("  AUTO \n"), Some(Override::Auto));
    }

    #[test]
    fn parse_override_modes() {
        assert_eq!(parse_override("power"), Some(Override::Mode(Mode::Power)));
        assert_eq!(parse_override("battery"), Some(Override::Mode(Mode::Power)));
        assert_eq!(parse_override("game"), Some(Override::Mode(Mode::Game)));
        assert_eq!(
            parse_override("performance"),
            Some(Override::Mode(Mode::Game))
        );
    }

    #[test]
    fn parse_override_unknown_is_none() {
        assert_eq!(parse_override("unknown"), None);
        assert_eq!(parse_override("42"), None);
    }

    #[test]
    fn decide_override_wins() {
        assert_eq!(
            decide_mode(Some(Override::Mode(Mode::Power)), true),
            Mode::Power
        );
        assert_eq!(
            decide_mode(Some(Override::Mode(Mode::Game)), false),
            Mode::Game
        );
    }

    #[test]
    fn decide_auto_follows_foreground() {
        assert_eq!(decide_mode(Some(Override::Auto), true), Mode::Game);
        assert_eq!(decide_mode(Some(Override::Auto), false), Mode::Power);
        assert_eq!(decide_mode(None, true), Mode::Game);
        assert_eq!(decide_mode(None, false), Mode::Power);
    }
}
