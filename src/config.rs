use serde::{Deserialize, Serialize};
use std::path::PathBuf;

// ============================================================
// 内置 Agent 工具注册表
// 路径基于实际验证的真实日志位置
// ============================================================

/// 内置工具定义
#[derive(Debug, Clone)]
pub struct BuiltinTool {
    pub id: &'static str,
    pub display_name: &'static str,
    pub description: &'static str,
    pub log_format: LogFormat,
    pub windows_path: &'static str,
    pub macos_path: &'static str,
    pub linux_path: &'static str,
    /// 是否有 token 使用量数据
    pub has_token_data: bool,
}

/// 日志格式类型
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum LogFormat {
    /// Claude Code 项目会话 JSONL（message.usage 字段）
    ClaudeCodeJsonl,
    /// DeepSeek GUI events.jsonl（usage.promptTokens 等字段）
    DeepSeekGuiJsonl,
    /// Cursor AI Tracking SQLite 数据库
    CursorSqlite,
    /// Codex sessions rollout JSONL（按会话累计 token_usage，取每会话最后累计值按天聚合）
    CodexJsonl,
    /// Copilot JetBrains partition JSONL（无 token 数据，仅对话记录）
    CopilotJbJsonl,
    /// Trae CN 日志（解析 ai-agent stdout 日志中的 token 估算数据）
    TraeCnLog,
    /// Trae CN 加密数据库（旧版，不可解析）
    TraeCnEncrypted,
    /// Lingma 日志（普通文本日志）
    LingmaLog,
    /// 通用 JSONL
    GenericJsonl,
    /// OpenCode SQLite 数据库（session 表含精确 token 数据）
    OpenCodeSqlite,
    /// ZCode SQLite 数据库（model_usage 表含精确 token 与缓存数据）
    ZCodeSqlite,
}

/// 判断该日志格式是否为"快照式"解析器。
/// 快照式解析器每次运行重读整个数据源、按天聚合并产出日期精度时间戳
/// （`YYYY-MM-DDT00:00:00`），其累计值会随解析增长——必须按 (tool, date) 替换写入，
/// 否则每次采集都会重复累加。增量式解析器则用真实调用时间戳 + 唯一约束去重插入。
pub fn is_snapshot_format(fmt: &LogFormat) -> bool {
    matches!(
        fmt,
        LogFormat::ZCodeSqlite
            | LogFormat::OpenCodeSqlite
            | LogFormat::TraeCnLog
            | LogFormat::CursorSqlite
            | LogFormat::CodexJsonl
    )
}

/// 全局内置工具注册表
pub fn builtin_tools() -> &'static [BuiltinTool] {
    &[
        // === 有 Token 数据的工具 ===
        BuiltinTool {
            id: "claude_code",
            display_name: "Claude Code",
            description: "Anthropic 官方命令行 AI 编程助手（含完整 Token 数据）",
            log_format: LogFormat::ClaudeCodeJsonl,
            windows_path: ".claude/projects",
            macos_path: ".claude/projects",
            linux_path: ".claude/projects",
            has_token_data: true,
        },
        BuiltinTool {
            id: "deepseek_gui",
            display_name: "DeepSeek GUI",
            description: "DeepSeek 桌面客户端（含 Token 数据和费用信息）",
            log_format: LogFormat::DeepSeekGuiJsonl,
            windows_path: ".deepseekgui/kun/threads",
            macos_path: ".deepseekgui/kun/threads",
            linux_path: ".deepseekgui/kun/threads",
            has_token_data: true,
        },
        BuiltinTool {
            id: "cursor",
            display_name: "Cursor",
            description: "AI-first code editor（AI 代码追踪数据库）",
            log_format: LogFormat::CursorSqlite,
            windows_path: ".cursor/ai-tracking",
            macos_path: ".cursor/ai-tracking",
            linux_path: ".cursor/ai-tracking",
            has_token_data: false,
        },

        // === 仅会话记录（无 Token 数据）===
        BuiltinTool {
            id: "codex",
            display_name: "Codex (OpenAI)",
            description: "OpenAI Codex 编程 Agent（仅会话记录）",
            log_format: LogFormat::CodexJsonl,
            windows_path: ".codex/sessions",
            macos_path: ".codex/sessions",
            linux_path: ".codex/sessions",
            has_token_data: false,
        },
        BuiltinTool {
            id: "copilot_jb",
            display_name: "GitHub Copilot (JetBrains)",
            description: "GitHub Copilot JetBrains 对话记录",
            log_format: LogFormat::CopilotJbJsonl,
            windows_path: ".copilot/jb",
            macos_path: ".copilot/jb",
            linux_path: ".copilot/jb",
            has_token_data: false,
        },
        BuiltinTool {
            id: "trae_cn",
            display_name: "Trae CN",
            description: "字节跳动 AI IDE（从日志解析 token 估算数据）",
            log_format: LogFormat::TraeCnLog,
            windows_path: "AppData/Roaming/Trae CN/logs",
            macos_path: "Library/Application Support/Trae CN/logs",
            linux_path: ".config/Trae CN/logs",
            has_token_data: true,
        },
        BuiltinTool {
            id: "lingma",
            display_name: "通义灵码",
            description: "阿里云 AI 编码助手",
            log_format: LogFormat::LingmaLog,
            windows_path: ".lingma/logs",
            macos_path: ".lingma/logs",
            linux_path: ".lingma/logs",
            has_token_data: false,
        },

        // === 其他常见工具（默认路径，可能需要自定义）===
        BuiltinTool {
            id: "windsurf",
            display_name: "Windsurf",
            description: "Codeium AI IDE",
            log_format: LogFormat::GenericJsonl,
            windows_path: "AppData/Roaming/Windsurf/User/logs",
            macos_path: "Library/Application Support/Windsurf/User/logs",
            linux_path: ".config/Windsurf/User/logs",
            has_token_data: false,
        },
        BuiltinTool {
            id: "aider",
            display_name: "Aider",
            description: "AI pair programming in your terminal",
            log_format: LogFormat::GenericJsonl,
            windows_path: ".aider/logs",
            macos_path: ".aider/logs",
            linux_path: ".aider/logs",
            has_token_data: false,
        },
        BuiltinTool {
            id: "cline",
            display_name: "Cline",
            description: "VS Code 自主 AI 编程扩展",
            log_format: LogFormat::GenericJsonl,
            windows_path: "AppData/Roaming/Code/User/globalStorage/saoudrizwan.claude-dev",
            macos_path: "Library/Application Support/Code/User/globalStorage/saoudrizwan.claude-dev",
            linux_path: ".config/Code/User/globalStorage/saoudrizwan.claude-dev",
            has_token_data: false,
        },
        BuiltinTool {
            id: "roo_code",
            display_name: "Roo Code",
            description: "VS Code AI 编程扩展",
            log_format: LogFormat::GenericJsonl,
            windows_path: "AppData/Roaming/Code/User/globalStorage/rooveterinaryinc.roo-cline",
            macos_path: "Library/Application Support/Code/User/globalStorage/rooveterinaryinc.roo-cline",
            linux_path: ".config/Code/User/globalStorage/rooveterinaryinc.roo-cline",
            has_token_data: false,
        },
        BuiltinTool {
            id: "continue_dev",
            display_name: "Continue",
            description: "VS Code / JetBrains 开源 AI 助手",
            log_format: LogFormat::GenericJsonl,
            windows_path: "AppData/Roaming/Code/User/globalStorage/continue.continue",
            macos_path: "Library/Application Support/Code/User/globalStorage/continue.continue",
            linux_path: ".config/Code/User/globalStorage/continue.continue",
            has_token_data: false,
        },
        BuiltinTool {
            id: "github_copilot",
            display_name: "GitHub Copilot (VS Code)",
            description: "GitHub Copilot VS Code 扩展",
            log_format: LogFormat::GenericJsonl,
            windows_path: "AppData/Roaming/Code/User/globalStorage/github.copilot-chat",
            macos_path: "Library/Application Support/Code/User/globalStorage/github.copilot-chat",
            linux_path: ".config/Code/User/globalStorage/github.copilot-chat",
            has_token_data: false,
        },
        BuiltinTool {
            id: "amazon_q",
            display_name: "Amazon Q Developer",
            description: "AWS AI 编程助手",
            log_format: LogFormat::GenericJsonl,
            windows_path: ".amazon-q/logs",
            macos_path: ".amazon-q/logs",
            linux_path: ".amazon-q/logs",
            has_token_data: false,
        },
        BuiltinTool {
            id: "opencode",
            display_name: "OpenCode",
            description: "开源终端 AI 编程助手（含完整 Token 和缓存数据）",
            log_format: LogFormat::OpenCodeSqlite,
            windows_path: ".local/share/opencode",
            macos_path: ".local/share/opencode",
            linux_path: ".local/share/opencode",
            has_token_data: true,
        },
        BuiltinTool {
            id: "zcode",
            display_name: "ZCode",
            description: "ZCode AI 编程助手（本地 SQLite 含精确 Token 与缓存数据）",
            log_format: LogFormat::ZCodeSqlite,
            windows_path: ".zcode/cli/db",
            macos_path: ".zcode/cli/db",
            linux_path: ".zcode/cli/db",
            has_token_data: true,
        },
    ]
}

/// 根据 ID 查找内置工具
pub fn find_builtin_tool(id: &str) -> Option<&'static BuiltinTool> {
    builtin_tools().iter().find(|t| t.id == id)
}

/// 获取内置工具的默认日志路径（跨平台）
pub fn get_default_log_path(tool_id: &str) -> Option<PathBuf> {
    let tool = find_builtin_tool(tool_id)?;
    let home = dirs::home_dir()?;

    let relative = if cfg!(target_os = "windows") {
        tool.windows_path
    } else if cfg!(target_os = "macos") {
        tool.macos_path
    } else {
        tool.linux_path
    };

    // 如果路径以 AppData 开头，需要特殊处理
    if relative.starts_with("AppData/") {
        let local_app_data = dirs::data_dir()?;
        let sub_path = relative.strip_prefix("AppData/Roaming/")
            .or_else(|| relative.strip_prefix("AppData/Local/"))
            .unwrap_or(relative);
        Some(local_app_data.join(sub_path))
    } else {
        Some(home.join(relative))
    }
}

// ============================================================
// 用户配置结构
// ============================================================

fn default_exchange_rate() -> f64 {
    7.2
}

/// VibeStats 全局配置
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    /// 启用的工具 ID 列表
    pub enabled_tools: Vec<String>,
    /// 工具自定义路径覆盖
    pub custom_paths: std::collections::HashMap<String, String>,
    /// 定时任务执行时间
    pub schedule_time: String,
    /// 数据库存储路径
    pub db_path: String,
    /// HTTP 服务端口
    pub serve_port: u16,
    /// 模型定价覆盖（key=模型名小写, value=每百万 Token 美元价），覆盖内置硬编码定价
    #[serde(default)]
    pub pricing_overrides: std::collections::HashMap<String, crate::models::ModelPricing>,
    /// 美元→人民币汇率（1 USD = ? CNY），可在设置页修改并触发历史费用重算
    #[serde(default = "default_exchange_rate")]
    pub exchange_rate: f64,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            enabled_tools: vec![
                "claude_code".into(),
                "deepseek_gui".into(),
                "cursor".into(),
                "trae_cn".into(),
                "codex".into(),
                "copilot_jb".into(),
                "opencode".into(),
                "zcode".into(),
            ],
            custom_paths: std::collections::HashMap::new(),
            schedule_time: "00:30".into(),
            db_path: "vibestats.db".into(),
            serve_port: 7890,
            pricing_overrides: std::collections::HashMap::new(),
            exchange_rate: 7.2,
        }
    }
}

impl Config {
    pub fn load_from_file(path: &std::path::Path) -> anyhow::Result<Self> {
        let content = std::fs::read_to_string(path)?;
        let config: Config = toml::from_str(&content)?;
        Ok(config)
    }

    pub fn save_to_file(&self, path: &std::path::Path) -> anyhow::Result<()> {
        let content = toml::to_string_pretty(self)?;
        // 原子写：先写同目录临时文件再 rename 替换，避免写一半崩溃导致 config.toml 损坏
        // （损坏会被 load_config 静默重置为 Default，丢失全部设置）
        let tmp_path = path.with_file_name(format!(
            "{}.tmp",
            path.file_name().and_then(|s| s.to_str()).unwrap_or("config")
        ));
        if let Err(e) = std::fs::write(&tmp_path, &content) {
            let _ = std::fs::remove_file(&tmp_path);
            return Err(anyhow::anyhow!("写临时配置失败: {}", e));
        }
        if let Err(e) = std::fs::rename(&tmp_path, path) {
            let _ = std::fs::remove_file(&tmp_path);
            return Err(anyhow::anyhow!("替换配置文件失败: {}", e));
        }
        Ok(())
    }

    pub fn config_path() -> PathBuf {
        Self::data_dir().join("config.toml")
    }

    pub fn data_dir() -> PathBuf {
        dirs::data_local_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("vibestats")
    }

    pub fn db_full_path(&self) -> PathBuf {
        Self::data_dir().join(&self.db_path)
    }

    /// 获取启用的工具配置列表
    pub fn enabled_tools(&self) -> Vec<ToolConfig> {
        self.enabled_tools
            .iter()
            .filter_map(|id| {
                let builtin = find_builtin_tool(id)?;
                let custom_path = self.custom_paths.get(id).cloned();
                Some(ToolConfig {
                    name: builtin.id.to_string(),
                    display_name: builtin.display_name.to_string(),
                    description: builtin.description.to_string(),
                    log_format: builtin.log_format.clone(),
                    custom_log_path: custom_path,
                    has_token_data: builtin.has_token_data,
                })
            })
            .collect()
    }

    /// 获取所有内置工具的状态
    pub fn all_tools_status(&self) -> Vec<ToolStatus> {
        builtin_tools()
            .iter()
            .map(|t| ToolStatus {
                id: t.id.to_string(),
                display_name: t.display_name.to_string(),
                description: t.description.to_string(),
                enabled: self.enabled_tools.contains(&t.id.to_string()),
                has_token_data: t.has_token_data,
                default_path: get_default_log_path(t.id)
                    .map(|p| p.to_string_lossy().to_string())
                    .unwrap_or_default(),
                custom_path: self.custom_paths.get(t.id).cloned(),
            })
            .collect()
    }
}

/// 解析后的工具配置
#[derive(Debug, Clone)]
pub struct ToolConfig {
    pub name: String,
    pub display_name: String,
    pub description: String,
    pub log_format: LogFormat,
    pub custom_log_path: Option<String>,
    pub has_token_data: bool,
}

/// 工具状态（用于前端展示）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolStatus {
    pub id: String,
    pub display_name: String,
    pub description: String,
    pub enabled: bool,
    pub has_token_data: bool,
    pub default_path: String,
    pub custom_path: Option<String>,
}

impl ToolConfig {
    /// 获取日志目录路径
    pub fn log_dir(&self) -> Option<PathBuf> {
        if let Some(custom) = &self.custom_log_path {
            let path = PathBuf::from(custom);
            if path.is_absolute() {
                Some(path)
            } else if custom.starts_with("AppData/") {
                // AppData 路径需要用 data_dir() 拼接
                let data_dir = dirs::data_dir()?;
                let sub_path = custom.strip_prefix("AppData/Roaming/")
                    .or_else(|| custom.strip_prefix("AppData/Local/"))
                    .unwrap_or(custom);
                Some(data_dir.join(sub_path))
            } else {
                dirs::home_dir().map(|home| home.join(custom))
            }
        } else {
            get_default_log_path(&self.name)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_builtin_tools_not_empty() {
        let tools = builtin_tools();
        assert!(!tools.is_empty(), "内置工具列表不应为空");
    }

    #[test]
    fn test_builtin_tools_has_required_ids() {
        let tools = builtin_tools();
        let ids: Vec<&str> = tools.iter().map(|t| t.id).collect();

        assert!(ids.contains(&"claude_code"), "应包含 claude_code");
        assert!(ids.contains(&"deepseek_gui"), "应包含 deepseek_gui");
        assert!(ids.contains(&"cursor"), "应包含 cursor");
        assert!(ids.contains(&"trae_cn"), "应包含 trae_cn");
        assert!(ids.contains(&"opencode"), "应包含 opencode");
        assert!(ids.contains(&"zcode"), "应包含 zcode");
        assert!(ids.contains(&"codex"), "应包含 codex");
    }

    #[test]
    fn test_find_builtin_tool() {
        let tool = find_builtin_tool("claude_code");
        assert!(tool.is_some());
        assert_eq!(tool.unwrap().id, "claude_code");
        assert_eq!(tool.unwrap().log_format, LogFormat::ClaudeCodeJsonl);
        assert!(tool.unwrap().has_token_data);
    }

    #[test]
    fn test_find_builtin_tool_not_found() {
        let tool = find_builtin_tool("nonexistent");
        assert!(tool.is_none());
    }

    #[test]
    fn test_default_config() {
        let config = Config::default();
        assert!(config.enabled_tools.contains(&"claude_code".to_string()));
        assert!(config.enabled_tools.contains(&"deepseek_gui".to_string()));
        assert!(config.enabled_tools.contains(&"cursor".to_string()));
        assert!(config.enabled_tools.contains(&"opencode".to_string()));
        assert!(config.enabled_tools.contains(&"zcode".to_string()));
        assert_eq!(config.serve_port, 7890);
    }

    #[test]
    fn test_config_save_and_load() {
        let dir = tempfile::tempdir().unwrap();
        let config_path = dir.path().join("config.toml");

        let config = Config::default();
        config.save_to_file(&config_path).unwrap();

        let loaded = Config::load_from_file(&config_path).unwrap();
        assert_eq!(loaded.enabled_tools, config.enabled_tools);
        assert_eq!(loaded.serve_port, config.serve_port);
    }

    #[test]
    fn test_enabled_tools_config() {
        let config = Config::default();
        let tools = config.enabled_tools();

        assert!(!tools.is_empty(), "默认配置应启用工具");

        let claude = tools.iter().find(|t| t.name == "claude_code");
        assert!(claude.is_some(), "应包含 claude_code");
        assert_eq!(claude.unwrap().log_format, LogFormat::ClaudeCodeJsonl);
        assert!(claude.unwrap().has_token_data);
    }

    #[test]
    fn test_all_tools_status() {
        let config = Config::default();
        let status = config.all_tools_status();

        assert!(!status.is_empty());
        let claude = status.iter().find(|t| t.id == "claude_code");
        assert!(claude.is_some());
        assert!(claude.unwrap().enabled);
    }

    #[test]
    fn test_tool_config_custom_path() {
        let tool = ToolConfig {
            name: "test".to_string(),
            display_name: "Test".to_string(),
            description: "test".to_string(),
            log_format: LogFormat::GenericJsonl,
            custom_log_path: Some("C:\\test_logs".to_string()),
            has_token_data: false,
        };

        let log_dir = tool.log_dir();
        assert!(log_dir.is_some());
        // 在 Windows 上绝对路径应直接返回
        let path = log_dir.unwrap();
        assert!(path.is_absolute(), "自定义绝对路径应返回绝对路径");
    }

    #[test]
    fn test_tool_config_home_path() {
        let tool = ToolConfig {
            name: "test".to_string(),
            display_name: "Test".to_string(),
            description: "test".to_string(),
            log_format: LogFormat::GenericJsonl,
            custom_log_path: Some(".test/logs".to_string()),
            has_token_data: false,
        };

        let log_dir = tool.log_dir();
        assert!(log_dir.is_some());
        // 应拼接 home 目录
        let home = dirs::home_dir().unwrap();
        assert_eq!(log_dir.unwrap(), home.join(".test/logs"));
    }

    #[test]
    fn test_log_format_equality() {
        assert_eq!(LogFormat::ClaudeCodeJsonl, LogFormat::ClaudeCodeJsonl);
        assert_ne!(LogFormat::ClaudeCodeJsonl, LogFormat::DeepSeekGuiJsonl);
    }

    #[test]
    fn test_cache_supporting_tools_have_token_data() {
        // 验证有缓存数据的工具标记正确
        let tools = builtin_tools();
        let cache_tools = ["claude_code", "deepseek_gui", "opencode", "zcode"];

        for id in &cache_tools {
            let tool = tools.iter().find(|t| t.id == *id).unwrap();
            assert!(tool.has_token_data, "{} 应标记为有 token 数据", id);
        }
    }

    #[test]
    fn test_cursor_no_token_data() {
        let tools = builtin_tools();
        let cursor = tools.iter().find(|t| t.id == "cursor").unwrap();
        assert!(!cursor.has_token_data, "Cursor 不应有精确 token 数据");
    }
}
