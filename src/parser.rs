use std::collections::HashMap;
use std::fs::OpenOptions;
use std::io::{BufRead, BufReader, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use log::{info, warn};
use regex::Regex;
use serde_json::Value;

use crate::config::{LogFormat, ToolConfig};
use crate::models::{FilePointer, RawEvent};

/// Cursor 每日 AI 代码行数统计（来自 state.vscdb）
struct CursorDailyStats {
    composer_suggested_lines: i64,
    composer_accepted_lines: i64,
    tab_suggested_lines: i64,
    tab_accepted_lines: i64,
}

/// Trae CN 会话 token 数据（来自 RalphLoop 日志）
#[derive(Default)]
struct TraeSessionData {
    context_total: i64,
    llm_output_tokens: i64,
    user_input_tokens: i64,
    read_tokens: i64,
    write_tokens: i64,
    edit_tokens: i64,
    run_command_tokens: i64,
    web_search_tokens: i64,
    other_tokens: i64,
    /// RalphLoop 条目数（用于估算对话轮次）
    loop_count: i64,
}

/// JSONL 流式读取器，支持增量读取
pub struct LogParser {
    pointers: HashMap<String, FilePointer>,
    pointer_store_path: PathBuf,
}

impl LogParser {
    pub fn new(data_dir: &Path) -> Self {
        let pointer_store_path = data_dir.join("file_pointers.json");
        let pointers = Self::load_pointers(&pointer_store_path);
        Self { pointers, pointer_store_path }
    }

    /// 解析指定工具的日志
    pub fn parse_tool_logs(&mut self, tool_config: &ToolConfig) -> Vec<RawEvent> {
        let log_dir = match tool_config.log_dir() {
            Some(dir) => dir,
            None => {
                warn!("无法解析工具 {} 的日志路径", tool_config.display_name);
                return vec![];
            }
        };

        if !log_dir.exists() {
            info!("日志目录不存在: {} ({})", log_dir.display(), tool_config.display_name);
            return vec![];
        }

        match &tool_config.log_format {
            LogFormat::CursorSqlite => self.parse_cursor_sqlite(&log_dir, &tool_config.name),
            LogFormat::OpenCodeSqlite => self.parse_opencode_sqlite(&log_dir, &tool_config.name),
            LogFormat::ZCodeSqlite => self.parse_zcode_sqlite(&log_dir, &tool_config.name),
            LogFormat::TraeCnLog => self.parse_trae_cn_logs(&log_dir, &tool_config.name),
            LogFormat::CodexJsonl => self.parse_codex_sessions(&log_dir, &tool_config.name),
            LogFormat::TraeCnEncrypted => {
                info!("Trae CN 数据库已加密，暂不支持解析");
                vec![]
            }
            LogFormat::LingmaLog => {
                info!("通义灵码日志格式暂不支持 Token 统计");
                vec![]
            }
            _ => {
                // JSONL 类格式
                let jsonl_files = self.find_jsonl_files(&log_dir);
                let mut all_events = Vec::new();
                for file_path in jsonl_files {
                    let events = self.parse_jsonl_file(&file_path, &tool_config.name, &tool_config.log_format);
                    all_events.extend(events);
                }
                info!("从 {} 解析到 {} 条事件", tool_config.display_name, all_events.len());
                all_events
            }
        }
    }

    /// 解析所有工具的日志
    pub fn parse_all_logs(&mut self, tools: &[ToolConfig]) -> Vec<RawEvent> {
        let mut all_events = Vec::new();
        for tool in tools {
            let events = self.parse_tool_logs(tool);
            all_events.extend(events);
        }
        all_events
    }

    /// 保存文件指针状态
    pub fn save_pointers(&self) {
        if let Ok(content) = serde_json::to_string_pretty(&self.pointers) {
            if let Some(parent) = self.pointer_store_path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            let _ = std::fs::write(&self.pointer_store_path, content);
        }
    }

    // ============================================================
    // Cursor SQLite 解析器
    // 数据源1: state.vscdb 的 aiCodeTracking.dailyStats（每日 AI 代码行数）
    // 数据源2: ai-code-tracking.db 的 ai_code_hashes（模型分布、请求数）
    // ============================================================

    fn parse_cursor_sqlite(&self, dir: &Path, tool_name: &str) -> Vec<RawEvent> {
        let db_path = dir.join("ai-code-tracking.db");
        if !db_path.exists() {
            info!("Cursor AI 追踪数据库不存在: {}", db_path.display());
            return vec![];
        }

        // === 数据源1: 从 state.vscdb 读取每日 AI 代码行数统计 ===
        let daily_stats = self.read_cursor_daily_stats();

        // === 数据源2: 从 ai-code-tracking.db 读取模型分布 ===
        let conn = match rusqlite::Connection::open_with_flags(
            &db_path,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
        ) {
            Ok(c) => c,
            Err(e) => {
                warn!("无法打开 Cursor 数据库: {}", e);
                return vec![];
            }
        };

        // 按日期和模型统计请求数
        let mut model_daily: HashMap<(String, String), i64> = HashMap::new();
        // (date, model) -> count

        let mut stmt = match conn.prepare(
            "SELECT model, timestamp, source FROM ai_code_hashes WHERE source != 'human'"
        ) {
            Ok(s) => s,
            Err(e) => {
                warn!("Cursor 数据库查询失败: {}", e);
                return vec![];
            }
        };

        let rows = stmt.query_map([], |row| {
            let model: String = row.get(0).unwrap_or_default();
            let timestamp: i64 = row.get(1).unwrap_or_default();
            let source: String = row.get(2).unwrap_or_default();
            Ok((model, timestamp, source))
        });

        let mut tab_daily: HashMap<String, i64> = HashMap::new();
        // date -> tab request count

        if let Ok(rows) = rows {
            for row in rows.flatten() {
                let (model, timestamp, source) = row;
                let ts_secs = timestamp / 1000;
                let date = chrono::DateTime::from_timestamp(ts_secs, 0)
                    .map(|dt| dt.format("%Y-%m-%d").to_string())
                    .unwrap_or_default();

                if date.is_empty() {
                    continue;
                }

                if source == "tab" {
                    *tab_daily.entry(date.clone()).or_insert(0) += 1;
                } else {
                    // composer 等来源，按模型统计
                    let mapped_model = Self::map_cursor_model(&model);
                    *model_daily.entry((date, mapped_model)).or_insert(0) += 1;
                }
            }
        }

        // === 合并两个数据源，生成 RawEvent ===
        let mut events = Vec::new();

        // 1. 使用 dailyStats 的行数来估算 composer token
        //    估算规则: 1 行代码 ≈ 15 token（含输入上下文）
        //    composer 请求: 输入/输出比约 3:1（上下文多，输出少）
        for (date, stats) in &daily_stats {
            let composer_lines = stats.composer_accepted_lines;
            let tab_lines = stats.tab_accepted_lines;

            if composer_lines == 0 && tab_lines == 0 {
                continue;
            }

            // 获取该日期的模型分布
            let mut model_counts: HashMap<String, i64> = HashMap::new();
            for ((d, model), count) in &model_daily {
                if d == date {
                    model_counts.insert(model.clone(), *count);
                }
            }

            let total_composer_requests: i64 = model_counts.values().sum();

            if total_composer_requests == 0 && composer_lines == 0 {
                // 只有 tab 数据
                let tab_input = (tab_lines as f64 * 5.0) as i64; // tab: 每行约 5 token 输入
                let tab_output = (tab_lines as f64 * 15.0) as i64; // 每行约 15 token 输出
                events.push(RawEvent {
                    id: None,
                    tool_name: tool_name.to_string(),
                    timestamp: format!("{}T00:00:00", date),
                    input_tokens: tab_input,
                    output_tokens: tab_output,
                    cache_read_tokens: 0,
                    model_name: Some("claude-sonnet".to_string()),
                    actual_cost: None,
                    raw_line: Some(format!("tab: {} accepted lines, estimated from dailyStats", tab_lines)),
                });
                continue;
            }

            // 按模型比例分配 composer token
            let total_tokens = (composer_lines as f64 * 15.0) as i64; // 总 token
            let input_ratio = 0.7; // 70% 输入（含上下文），30% 输出
            let total_input = (total_tokens as f64 * input_ratio) as i64;
            let total_output = total_tokens - total_input;

            for (model, count) in &model_counts {
                let ratio = *count as f64 / total_composer_requests as f64;
                let input = (total_input as f64 * ratio) as i64;
                let output = (total_output as f64 * ratio) as i64;

                if input == 0 && output == 0 {
                    continue;
                }

                events.push(RawEvent {
                    id: None,
                    tool_name: tool_name.to_string(),
                    timestamp: format!("{}T00:00:00", date),
                    input_tokens: input,
                    output_tokens: output,
                    cache_read_tokens: 0,
                    model_name: Some(model.clone()),
                    actual_cost: None,
                    raw_line: Some(format!(
                        "composer: {} lines, {} requests ({:.0}%), estimated from dailyStats",
                        composer_lines, count, ratio * 100.0
                    )),
                });
            }

            // Tab 补全单独计算
            if tab_lines > 0 {
                let tab_input = (tab_lines as f64 * 5.0) as i64;
                let tab_output = (tab_lines as f64 * 15.0) as i64;
                events.push(RawEvent {
                    id: None,
                    tool_name: tool_name.to_string(),
                    timestamp: format!("{}T00:00:00", date),
                    input_tokens: tab_input,
                    output_tokens: tab_output,
                    cache_read_tokens: 0,
                    model_name: Some("claude-sonnet".to_string()),
                    actual_cost: None,
                    raw_line: Some(format!("tab: {} accepted lines, estimated from dailyStats", tab_lines)),
                });
            }
        }

        // 2. 处理 dailyStats 中没有但 ai_code_hashes 中有的日期（回退方案）
        let stats_dates: std::collections::HashSet<String> = daily_stats.keys().cloned().collect();
        let mut remaining_dates: std::collections::HashSet<String> = std::collections::HashSet::new();
        for ((date, _), _) in &model_daily {
            if !stats_dates.contains(date) {
                remaining_dates.insert(date.clone());
            }
        }
        for date in &remaining_dates {
            // 回退：ai_code_hashes 表每个条目是一个被接受的代码块（3-15行），而非一次完整 LLM 请求
            // 每个代码块约 10 行 × 15 token/行 = 150 token，70% 输入 / 30% 输出
            // 注意：Cursor 的 agent/chat 模式实际 token 消耗远高于代码块大小（含上下文文件、系统提示等），
            // 但 hash 表仅记录产出代码块，故用保守值避免虚高
            let mut model_counts: HashMap<String, i64> = HashMap::new();
            for ((d, model), count) in &model_daily {
                if d == date {
                    model_counts.insert(model.clone(), *count);
                }
            }
            let tab_count = tab_daily.get(date).copied().unwrap_or(0);

            for (model, count) in &model_counts {
                // 保守估算：每个 hash 条目 = 约 100 input + 50 output token
                let input = *count * 100;
                let output = *count * 50;
                if input == 0 && output == 0 {
                    continue;
                }
                events.push(RawEvent {
                    id: None,
                    tool_name: tool_name.to_string(),
                    timestamp: format!("{}T00:00:00", date),
                    input_tokens: input,
                    output_tokens: output,
                    cache_read_tokens: 0,
                    model_name: Some(model.clone()),
                    actual_cost: None,
                    raw_line: Some(format!("fallback-composer: {} requests, estimated from requestId", count)),
                });
            }
            // Tab 补全单独计算
            if tab_count > 0 {
                events.push(RawEvent {
                    id: None,
                    tool_name: tool_name.to_string(),
                    timestamp: format!("{}T00:00:00", date),
                    input_tokens: tab_count * 30,
                    output_tokens: tab_count * 120,
                    cache_read_tokens: 0,
                    model_name: Some("claude-sonnet".to_string()),
                    actual_cost: None,
                    raw_line: Some(format!("fallback-tab: {} completions", tab_count)),
                });
            }
        }

        info!("从 Cursor AI 解析到 {} 条统计记录（dailyStats + 模型分布）", events.len());
        events
    }

    /// 从 Cursor 的 state.vscdb 读取每日 AI 代码行数统计
    fn read_cursor_daily_stats(&self) -> HashMap<String, CursorDailyStats> {
        let mut result = HashMap::new();

        // state.vscdb 路径
        let vscdb_path = dirs::data_dir()
            .map(|d| d.join("Cursor").join("User").join("globalStorage").join("state.vscdb"))
            .unwrap_or_default();

        if !vscdb_path.exists() {
            info!("Cursor state.vscdb 不存在: {}", vscdb_path.display());
            return result;
        }

        let conn = match rusqlite::Connection::open_with_flags(
            &vscdb_path,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
        ) {
            Ok(c) => c,
            Err(e) => {
                warn!("无法打开 Cursor state.vscdb: {}", e);
                return result;
            }
        };

        // 查询 aiCodeTracking.dailyStats
        let mut stmt = match conn.prepare(
            "SELECT value FROM ItemTable WHERE key = 'aiCodeTracking.dailyStats'"
        ) {
            Ok(s) => s,
            Err(e) => {
                warn!("查询 Cursor dailyStats 失败: {}", e);
                return result;
            }
        };

        let json_str: String = match stmt.query_row([], |row| row.get(0)) {
            Ok(s) => s,
            Err(_) => {
                info!("Cursor dailyStats 数据为空");
                return result;
            }
        };

        // 解析 JSON 数组
        let stats_array: Vec<Value> = match serde_json::from_str(&json_str) {
            Ok(v) => v,
            Err(e) => {
                warn!("解析 Cursor dailyStats JSON 失败: {}", e);
                return result;
            }
        };

        for stat in &stats_array {
            let date = stat.get("date").and_then(|v| v.as_str()).unwrap_or("").to_string();
            if date.is_empty() {
                continue;
            }
            let composer_suggested = stat.get("composerSuggestedLines").and_then(|v| v.as_i64()).unwrap_or(0);
            let composer_accepted = stat.get("composerAcceptedLines").and_then(|v| v.as_i64()).unwrap_or(0);
            let tab_suggested = stat.get("tabSuggestedLines").and_then(|v| v.as_i64()).unwrap_or(0);
            let tab_accepted = stat.get("tabAcceptedLines").and_then(|v| v.as_i64()).unwrap_or(0);

            result.insert(date, CursorDailyStats {
                composer_suggested_lines: composer_suggested,
                composer_accepted_lines: composer_accepted,
                tab_suggested_lines: tab_suggested,
                tab_accepted_lines: tab_accepted,
            });
        }

        info!("从 Cursor state.vscdb 读取到 {} 天的 dailyStats", result.len());
        result
    }

    /// 将 Cursor 的模型名映射到实际模型
    fn map_cursor_model(model: &str) -> String {
        if model.is_empty() || model == "default" || model == "composer-2" || model == "composer-2.5" {
            "claude-sonnet".to_string()
        } else if model == "premium" {
            "claude-opus".to_string()
        } else {
            model.to_string()
        }
    }

    // ============================================================
    // Trae CN 日志解析器
    // 数据源1: ai-agent stdout 日志的 [RalphLoop] History token accumulate
    //   - 记录的是上下文中每个工具结果占用的 token 数
    //   - 需要乘以倍率估算实际 LLM 消耗（LLM 会读取整个上下文）
    // 数据源2: renderer.log 的 icube_ai_front_response
    //   - 包含模型名、耗时等信息
    // ============================================================

    fn parse_trae_cn_logs(&self, log_dir: &Path, tool_name: &str) -> Vec<RawEvent> {
        // 递归查找所有日志文件
        let agent_log_files = self.find_trae_cn_log_files(log_dir);
        let renderer_log_files = self.find_trae_cn_renderer_logs(log_dir);

        if agent_log_files.is_empty() && renderer_log_files.is_empty() {
            info!("未找到 Trae CN 日志文件，搜索目录: {}", log_dir.display());
            return vec![];
        }

        info!("找到 {} 个 Trae CN ai-agent 日志, {} 个 renderer 日志",
            agent_log_files.len(), renderer_log_files.len());

        // === 数据源1: 从 ai-agent stdout 日志提取 RalphLoop token 数据 ===
        let re = Regex::new(
            r"^(\d{4}-\d{2}-\d{2})T.*\[RalphLoop\]\s+History token accumulate:\s+source=(\w+),\s+item_token_usage=(\d+)"
        ).unwrap();

        // 按 (date, session_id) 分组
        // 每组记录: 上下文 token 总数 + 各来源的 token 数
        let mut grouped: HashMap<(String, String), TraeSessionData> = HashMap::new();

        for log_file in &agent_log_files {
            let file = match OpenOptions::new().read(true).open(log_file) {
                Ok(f) => f,
                Err(e) => {
                    warn!("无法打开 Trae CN 日志文件 {}: {}", log_file.display(), e);
                    continue;
                }
            };

            let reader = BufReader::new(file);
            for line in reader.lines() {
                let line = match line {
                    Ok(l) => l,
                    Err(_) => continue,
                };

                if let Some(caps) = re.captures(&line) {
                    let date = caps.get(1).map(|m| m.as_str()).unwrap_or("").to_string();
                    let source = caps.get(2).map(|m| m.as_str()).unwrap_or("");
                    let token_usage: i64 = caps.get(3)
                        .and_then(|m| m.as_str().parse().ok())
                        .unwrap_or(0);

                    // 从文件路径提取 session_id
                    let session_id = log_file.parent()
                        .and_then(|p| p.file_name())
                        .map(|n| n.to_string_lossy().to_string())
                        .unwrap_or_default();

                    let key = (date.clone(), session_id);
                    let entry = grouped.entry(key).or_default();

                    // 记录各来源的 token
                    match source {
                        "llm_default" => entry.llm_output_tokens += token_usage,
                        "user_input" => entry.user_input_tokens += token_usage,
                        "Read" => entry.read_tokens += token_usage,
                        "Write" => entry.write_tokens += token_usage,
                        "Edit" => entry.edit_tokens += token_usage,
                        "RunCommand" => entry.run_command_tokens += token_usage,
                        "WebSearch" => entry.web_search_tokens += token_usage,
                        _ => entry.other_tokens += token_usage,
                    }
                    entry.context_total += token_usage;
                    entry.loop_count += 1;
                }
            }
        }

        // === 数据源2: 从 renderer.log 提取模型信息 + 输出 token 估算 ===
        let model_re = Regex::new(
            r#""model":\s*"(?:custom_openai_compatible//)?([^"]+)""#
        ).unwrap();
        // 提取 tokenOutputInterval 和 costTime 用于估算输出 token
        let cost_time_re = Regex::new(
            r#""costTime":\s*(\d+)"#
        ).unwrap();
        let token_interval_re = Regex::new(
            r#""tokenOutputInterval":\s*([\d.]+)"#
        ).unwrap();
        let mut date_models: HashMap<String, HashMap<String, i64>> = HashMap::new();
        // date -> { model -> count }
        let mut date_output_estimates: HashMap<String, i64> = HashMap::new();
        // date -> estimated output tokens from renderer.log

        for log_file in &renderer_log_files {
            let file = match OpenOptions::new().read(true).open(log_file) {
                Ok(f) => f,
                Err(_) => continue,
            };

            // 从文件路径提取日期
            let file_date = log_file.parent()
                .and_then(|p| p.parent())
                .and_then(|p| p.file_name())
                .map(|n| n.to_string_lossy().to_string())
                .and_then(|name| {
                    // 目录名格式: 20260610_123456 或 20260610T101635
                    let digits: String = name.chars().take(8).collect();
                    if digits.len() >= 8 && digits.chars().all(|c| c.is_ascii_digit()) {
                        Some(format!("{}-{}-{}", &digits[0..4], &digits[4..6], &digits[6..8]))
                    } else {
                        None
                    }
                })
                .unwrap_or_default();

            let reader = BufReader::new(file);
            for line in reader.lines() {
                let line = match line {
                    Ok(l) => l,
                    Err(_) => continue,
                };

                if line.contains("icube_ai_front_response") {
                    // 提取模型名
                    if let Some(caps) = model_re.captures(&line) {
                        let model = caps.get(1).map(|m| m.as_str()).unwrap_or("").to_string();
                        if !model.is_empty() && !file_date.is_empty() {
                            *date_models
                                .entry(file_date.clone())
                                .or_default()
                                .entry(model)
                                .or_insert(0) += 1;
                        }
                    }

                    // 提取 costTime 和 tokenOutputInterval 估算输出 token
                    let cost_time: i64 = cost_time_re.captures(&line)
                        .and_then(|c| c.get(1))
                        .and_then(|m| m.as_str().parse().ok())
                        .unwrap_or(0);
                    let token_interval: f64 = token_interval_re.captures(&line)
                        .and_then(|c| c.get(1))
                        .and_then(|m| m.as_str().parse().ok())
                        .unwrap_or(0.0);

                    if cost_time > 0 && token_interval > 0.0 && !file_date.is_empty() {
                        let estimated_output = (cost_time as f64 / token_interval) as i64;
                        *date_output_estimates.entry(file_date.clone()).or_insert(0) += estimated_output;
                    }
                }
            }
        }

        // === 合并数据，生成 RawEvent ===
        let mut events = Vec::new();

        // 先按日期聚合所有会话的数据
        let mut date_aggregated: HashMap<String, TraeSessionData> = HashMap::new();
        for ((date, _session_id), session_data) in &grouped {
            let entry = date_aggregated.entry(date.clone()).or_default();
            entry.context_total += session_data.context_total;
            entry.llm_output_tokens += session_data.llm_output_tokens;
            entry.user_input_tokens += session_data.user_input_tokens;
            entry.read_tokens += session_data.read_tokens;
            entry.write_tokens += session_data.write_tokens;
            entry.edit_tokens += session_data.edit_tokens;
            entry.run_command_tokens += session_data.run_command_tokens;
            entry.web_search_tokens += session_data.web_search_tokens;
            entry.other_tokens += session_data.other_tokens;
            entry.loop_count += session_data.loop_count;
        }

        for (date, session_data) in &date_aggregated {
            if session_data.context_total == 0 {
                continue;
            }

            // 估算实际 LLM token 消耗
            // RalphLoop 在每轮 LLM 调用前会重新遍历所有历史条目
            // 所以 context_total 已经包含了重复计数，近似于"总输入 token 消耗"
            // 但 RalphLoop 只统计工具结果 token，不包含 system prompt、消息格式化等开销
            // 乘以 2.5x 倍率补偿这些未计入的 token
            let input_tokens = (session_data.context_total as f64 * 2.5) as i64;

            // 输出 token：优先使用 renderer.log 的估算，否则使用 llm_output_tokens
            let renderer_output = date_output_estimates.get(date).copied().unwrap_or(0);
            let output_tokens = if renderer_output > 0 {
                renderer_output
            } else {
                session_data.llm_output_tokens
            };

            if input_tokens == 0 && output_tokens == 0 {
                continue;
            }

            // 获取该日期使用的模型
            let model_counts = date_models.get(date);
            let total_model_requests: i64 = model_counts
                .map(|m| m.values().sum())
                .unwrap_or(0);

            if total_model_requests == 0 {
                // 没有模型信息，默认 GLM-5.1
                events.push(RawEvent {
                    id: None,
                    tool_name: tool_name.to_string(),
                    timestamp: format!("{}T00:00:00", date),
                    input_tokens,
                    output_tokens,
                    cache_read_tokens: 0,
                    model_name: Some("glm-5.1".to_string()),
                    actual_cost: None,
                    raw_line: Some(format!(
                        "context={}(user={},read={},write={},edit={},run={},web={},other={},llm_out={}), output_est={}",
                        session_data.context_total,
                        session_data.user_input_tokens,
                        session_data.read_tokens,
                        session_data.write_tokens,
                        session_data.edit_tokens,
                        session_data.run_command_tokens,
                        session_data.web_search_tokens,
                        session_data.other_tokens,
                        session_data.llm_output_tokens,
                        renderer_output,
                    )),
                });
            } else {
                // 按模型比例分配 token
                for (model, count) in model_counts.unwrap() {
                    let ratio = *count as f64 / total_model_requests as f64;
                    let input = (input_tokens as f64 * ratio) as i64;
                    let output = (output_tokens as f64 * ratio) as i64;

                    if input == 0 && output == 0 {
                        continue;
                    }

                    events.push(RawEvent {
                        id: None,
                        tool_name: tool_name.to_string(),
                        timestamp: format!("{}T00:00:00", date),
                        input_tokens: input,
                        output_tokens: output,
                        cache_read_tokens: 0,
                        model_name: Some(model.clone()),
                        actual_cost: None,
                        raw_line: Some(format!(
                            "model={} ({:.0}% of {} reqs), input={}, output={}",
                            model, ratio * 100.0, total_model_requests, input, output
                        )),
                    });
                }
            }
        }

        info!("从 Trae CN 日志解析到 {} 条 token 使用记录", events.len());
        events
    }

    /// 递归查找 Trae CN renderer 日志文件
    fn find_trae_cn_renderer_logs(&self, dir: &Path) -> Vec<PathBuf> {
        let mut files = Vec::new();
        if let Ok(entries) = std::fs::read_dir(dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    files.extend(self.find_trae_cn_renderer_logs(&path));
                } else if let Some(name) = path.file_name() {
                    let name = name.to_string_lossy();
                    if name.contains("renderer") && name.ends_with(".log") {
                        files.push(path);
                    }
                }
            }
        }
        files.sort();
        files
    }

    /// 递归查找 Trae CN ai-agent stdout 日志文件
    fn find_trae_cn_log_files(&self, dir: &Path) -> Vec<PathBuf> {
        let mut files = Vec::new();
        if let Ok(entries) = std::fs::read_dir(dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    files.extend(self.find_trae_cn_log_files(&path));
                } else if let Some(name) = path.file_name() {
                    let name = name.to_string_lossy();
                    if name.contains("ai-agent") && name.contains("stdout") {
                        files.push(path);
                    }
                }
            }
        }
        files.sort();
        files
    }

    // ============================================================
    // JSONL 文件解析
    // ============================================================

    fn parse_jsonl_file(
        &mut self,
        file_path: &Path,
        tool_name: &str,
        log_format: &LogFormat,
    ) -> Vec<RawEvent> {
        let key = file_path.to_string_lossy().to_string();
        let file_metadata = std::fs::metadata(file_path);
        let current_modified = file_metadata
            .ok()
            .and_then(|m| m.modified().ok())
            .map(|t| t.duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_secs())
            .unwrap_or(0);

        // Claude Code（按 message.id 整会话去重）与 DeepSeek GUI（累计值差分）
        // 都需要从头读取整个文件，故强制从 offset 0 扫描
        let is_cumulative = *log_format == LogFormat::DeepSeekGuiJsonl
            || *log_format == LogFormat::ClaudeCodeJsonl;

        let start_offset = if is_cumulative {
            0 // 累计值格式始终从头读取
        } else if let Some(pointer) = self.pointers.get(&key) {
            if current_modified >= pointer.last_modified { pointer.offset } else { 0 }
        } else {
            0
        };

        let file = match OpenOptions::new().read(true).open(file_path) {
            Ok(f) => f,
            Err(e) => {
                warn!("无法打开文件 {}: {}", file_path.display(), e);
                return vec![];
            }
        };

        let mut reader = BufReader::new(file);
        if start_offset > 0 {
            if reader.seek(SeekFrom::Start(start_offset)).is_err() {
                let _ = reader.seek(SeekFrom::Start(0));
            }
        }

        let mut events = Vec::new();
        let mut current_pos = start_offset;

        // DeepSeek GUI（累计值差分）与 Claude Code（整会话按 message.id 去重）需先收集整文件再处理
        if *log_format == LogFormat::DeepSeekGuiJsonl || *log_format == LogFormat::ClaudeCodeJsonl {
            let mut all_lines = Vec::new();
            loop {
                let mut line = String::new();
                match reader.read_line(&mut line) {
                    Ok(0) => break,
                    Ok(n) => {
                        current_pos += n as u64;
                        let trimmed = line.trim();
                        if !trimmed.is_empty() {
                            all_lines.push(trimmed.to_string());
                        }
                    }
                    Err(e) => {
                        warn!("读取文件 {} 出错: {}", file_path.display(), e);
                        break;
                    }
                }
            }
            if *log_format == LogFormat::DeepSeekGuiJsonl {
                events = Self::parse_deepseek_gui_with_delta(&all_lines, tool_name);
            } else {
                events = Self::parse_claude_code_session(&all_lines, tool_name);
            }
        } else {
            loop {
                let mut line = String::new();
                match reader.read_line(&mut line) {
                    Ok(0) => break,
                    Ok(n) => {
                        current_pos += n as u64;
                        let trimmed = line.trim();
                        if trimmed.is_empty() { continue; }
                        if let Some(event) = Self::parse_jsonl_line(trimmed, tool_name, log_format) {
                            events.push(event);
                        }
                    }
                    Err(e) => {
                        warn!("读取文件 {} 出错: {}", file_path.display(), e);
                        break;
                    }
                }
            }
        }

        self.pointers.insert(key.clone(), FilePointer {
            file_path: key,
            offset: current_pos,
            last_modified: current_modified,
        });

        events
    }

    fn parse_jsonl_line(line: &str, tool_name: &str, log_format: &LogFormat) -> Option<RawEvent> {
        let value: Value = serde_json::from_str(line).ok()?;
        match log_format {
            LogFormat::ClaudeCodeJsonl => Self::parse_claude_code(&value, tool_name, line),
            LogFormat::DeepSeekGuiJsonl => Self::parse_deepseek_gui(&value, tool_name, line),
            LogFormat::CopilotJbJsonl => Self::parse_copilot_jb(&value, tool_name, line),
            LogFormat::GenericJsonl => Self::parse_generic(&value, tool_name, line),
            _ => None,
        }
    }

    /// Claude Code 会话解析：Anthropic 的 message.usage 是"逐请求"计费（非累计），
    /// 故按 message.id 去重后直接累加每条调用的真实 usage，不再做差分。
    /// 每条事件保留真实调用时间戳，由存储层按唯一约束去重，避免跨运行重复累加。
    fn parse_claude_code_session(lines: &[String], tool_name: &str) -> Vec<RawEvent> {
        let mut seen_ids = std::collections::HashSet::new();
        let mut events = Vec::new();

        for line in lines {
            let value: Value = match serde_json::from_str(line) {
                Ok(v) => v,
                Err(_) => continue,
            };
            if value.get("type").and_then(|v| v.as_str()).unwrap_or("") != "assistant" {
                continue;
            }
            let message = match value.get("message") {
                Some(m) => m,
                None => continue,
            };
            let usage = match message.get("usage") {
                Some(u) => u,
                None => continue,
            };

            let msg_id = message.get("id").and_then(|v| v.as_str()).unwrap_or("").to_string();
            if !msg_id.is_empty() {
                if seen_ids.contains(&msg_id) {
                    continue; // 同一 message.id 可能有多行，仅计一次
                }
                seen_ids.insert(msg_id);
            }

            let input = usage.get("input_tokens").and_then(|v| v.as_i64()).unwrap_or(0);
            let output = usage.get("output_tokens").and_then(|v| v.as_i64()).unwrap_or(0);
            let cache_read = usage.get("cache_read_input_tokens").and_then(|v| v.as_i64()).unwrap_or(0);
            let cache_creation = usage.get("cache_creation_input_tokens").and_then(|v| v.as_i64()).unwrap_or(0);

            // 跳过没有实际 token 消耗的条目
            if input == 0 && output == 0 && cache_read == 0 && cache_creation == 0 {
                continue;
            }

            let timestamp = value.get("timestamp").and_then(|v| v.as_str()).unwrap_or("").to_string();
            let model = message.get("model").and_then(|v| v.as_str()).map(String::from);

            // cache_creation 是写入缓存的 token（首次写入开销），归入 input 侧成本；
            // cache_read 是缓存命中 token，单独存入 cache_read_tokens
            events.push(RawEvent {
                id: None,
                tool_name: tool_name.to_string(),
                timestamp,
                input_tokens: input + cache_creation,
                output_tokens: output,
                cache_read_tokens: cache_read,
                model_name: model,
                actual_cost: None,
                raw_line: Some("claude_code per-call usage".to_string()),
            });
        }

        events
    }

    /// Claude Code 项目会话 JSONL 解析（原始累计值，不再直接使用）
    /// 格式: { "type": "assistant", "message": { "usage": { "input_tokens": ..., "output_tokens": ... }, "model": "..." }, "timestamp": "..." }
    fn parse_claude_code(value: &Value, tool_name: &str, raw: &str) -> Option<RawEvent> {
        // 只处理 assistant 类型的消息（含 usage 数据）
        let msg_type = value.get("type").and_then(|v| v.as_str()).unwrap_or("");
        if msg_type != "assistant" {
            return None;
        }

        let message = value.get("message")?;
        let usage = message.get("usage")?;

        let input_tokens = usage.get("input_tokens").and_then(|v| v.as_i64()).unwrap_or(0);
        let output_tokens = usage.get("output_tokens").and_then(|v| v.as_i64()).unwrap_or(0);
        let cache_read = usage.get("cache_read_input_tokens").and_then(|v| v.as_i64()).unwrap_or(0);
        let cache_creation = usage.get("cache_creation_input_tokens").and_then(|v| v.as_i64()).unwrap_or(0);

        // 跳过没有实际 token 消耗的条目
        if input_tokens == 0 && output_tokens == 0 && cache_read == 0 && cache_creation == 0 {
            return None;
        }

        // cache_creation 是写入缓存的 token（首次写入开销），归入 input 侧成本；
        // cache_read 是缓存命中 token，单独存入 cache_read_tokens
        Some(RawEvent {
            id: None,
            tool_name: tool_name.to_string(),
            timestamp: value.get("timestamp").and_then(|v| v.as_str()).unwrap_or("").to_string(),
            input_tokens: input_tokens + cache_creation,
            output_tokens,
            cache_read_tokens: cache_read,
            model_name: message.get("model").and_then(|v| v.as_str()).map(String::from),
            actual_cost: None,
            raw_line: Some(raw.to_string()),
        })
    }

    /// DeepSeek GUI 增量计算：usage 是累计值，使用自带的 costUsd 计算增量费用
    fn parse_deepseek_gui_with_delta(lines: &[String], tool_name: &str) -> Vec<RawEvent> {
        // 收集所有 usage 条目
        let mut usages: Vec<(String, String, i64, i64, i64, f64, Option<String>)> = Vec::new();
        // (turnId, timestamp, promptTokens, completionTokens, cachedTokens, costUsd, model)

        for line in lines {
            let value: Value = match serde_json::from_str(line) {
                Ok(v) => v,
                Err(_) => continue,
            };
            let kind = value.get("kind").and_then(|v| v.as_str()).unwrap_or("");
            if kind != "usage" {
                continue;
            }
            let usage = match value.get("usage") {
                Some(u) => u,
                None => continue,
            };
            let turn_id = value.get("turnId").and_then(|v| v.as_str()).unwrap_or("").to_string();
            let timestamp = value.get("timestamp").and_then(|v| v.as_str()).unwrap_or("").to_string();
            let prompt = usage.get("promptTokens").and_then(|v| v.as_i64()).unwrap_or(0);
            let completion = usage.get("completionTokens").and_then(|v| v.as_i64()).unwrap_or(0);
            let cached = usage.get("cachedTokens")
                .or_else(|| usage.get("cacheHitTokens"))
                .and_then(|v| v.as_i64())
                .unwrap_or(0);
            let cost_usd = usage.get("costUsd").and_then(|v| v.as_f64()).unwrap_or(0.0);
            let model = value.get("model").and_then(|v| v.as_str()).map(String::from);

            usages.push((turn_id, timestamp, prompt, completion, cached, cost_usd, model));
        }

        // 按顺序计算增量
        let mut events = Vec::new();
        let mut prev_prompt: i64 = 0;
        let mut prev_completion: i64 = 0;
        let mut prev_cached: i64 = 0;
        let mut prev_cost: f64 = 0.0;

        for (i, (turn_id, timestamp, prompt, completion, cached, cost_usd, model)) in usages.iter().enumerate() {
            // 当累计值减少时（计数器重置），将当前值视为绝对值
            let delta_prompt = if i == 0 || *prompt < prev_prompt {
                *prompt
            } else {
                prompt - prev_prompt
            };
            let delta_completion = if i == 0 || *completion < prev_completion {
                *completion
            } else {
                completion - prev_completion
            };
            let delta_cached = if i == 0 || *cached < prev_cached {
                *cached
            } else {
                cached - prev_cached
            };
            let delta_cost = if i == 0 { *cost_usd } else { cost_usd - prev_cost };

            // 跳过增量为 0 的条目
            if delta_prompt == 0 && delta_completion == 0 && delta_cached == 0 && delta_cost <= 0.0 && i > 0 {
                continue;
            }

            // DeepSeek GUI 的 token 是累计值，直接使用增量
            // 但对于第1条（i==0），如果值太大说明是整个会话的累计，用 costUsd 反算等效 token
            let (final_input, final_output) = if i == 0 && *prompt > 1_000_000 {
                // 第1条记录是整个会话的累计，用实际费用反算等效 token
                // 使用 Claude Sonnet 4 定价：input=$3/MTok, output=$15/MTok
                // 假设 90% 是 input token
                let effective_cost = if *cost_usd > 0.0 { *cost_usd } else { delta_cost };
                let input_ratio = 0.9;
                let input_cost = effective_cost * input_ratio;
                let output_cost = effective_cost * (1.0 - input_ratio);
                let equiv_input = (input_cost / 3.0 * 1_000_000.0) as i64;
                let equiv_output = (output_cost / 15.0 * 1_000_000.0) as i64;
                (equiv_input, equiv_output)
            } else {
                (delta_prompt, delta_completion)
            };

            // DeepSeek GUI 自带 costUsd，直接作为实际费用
            let actual_cost = if *cost_usd > 0.0 {
                Some(if i == 0 { *cost_usd } else { delta_cost })
            } else {
                None
            };

            events.push(RawEvent {
                id: None,
                tool_name: tool_name.to_string(),
                timestamp: timestamp.clone(),
                input_tokens: final_input,
                output_tokens: final_output,
                cache_read_tokens: delta_cached,
                model_name: model.clone(),
                actual_cost,
                raw_line: Some(format!("turnId={}, costUsd={:.4}", turn_id, if i == 0 { *cost_usd } else { delta_cost })),
            });

            prev_prompt = *prompt;
            prev_completion = *completion;
            prev_cached = *cached;
            prev_cost = *cost_usd;
        }

        events
    }

    /// DeepSeek GUI events.jsonl 解析（原始累计值，不再直接使用）
    /// 格式: { "kind": "usage", "model": "deepseek-v4-pro", "usage": { "promptTokens": ..., "completionTokens": ..., "cachedTokens": ..., "costUsd": ... }, "timestamp": "..." }
    fn parse_deepseek_gui(value: &Value, tool_name: &str, raw: &str) -> Option<RawEvent> {
        let kind = value.get("kind").and_then(|v| v.as_str()).unwrap_or("");
        if kind != "usage" {
            return None;
        }

        let usage = value.get("usage")?;

        Some(RawEvent {
            id: None,
            tool_name: tool_name.to_string(),
            timestamp: value.get("timestamp").and_then(|v| v.as_str()).unwrap_or("").to_string(),
            input_tokens: usage.get("promptTokens").and_then(|v| v.as_i64()).unwrap_or(0),
            output_tokens: usage.get("completionTokens").and_then(|v| v.as_i64()).unwrap_or(0),
            cache_read_tokens: usage.get("cachedTokens").or_else(|| usage.get("cacheHitTokens"))
                .and_then(|v| v.as_i64()).unwrap_or(0),
            model_name: value.get("model").and_then(|v| v.as_str()).map(String::from),
            actual_cost: None,
            raw_line: Some(raw.to_string()),
        })
    }

    /// Copilot JetBrains partition JSONL 解析（无 token 数据）
    fn parse_copilot_jb(value: &Value, tool_name: &str, raw: &str) -> Option<RawEvent> {
        let msg_type = value.get("type").and_then(|v| v.as_str()).unwrap_or("");
        if msg_type != "assistant.turn_start" {
            return None;
        }

        Some(RawEvent {
            id: None,
            tool_name: tool_name.to_string(),
            timestamp: value.get("timestamp").and_then(|v| v.as_str()).unwrap_or("").to_string(),
            input_tokens: 0,
            output_tokens: 0,
            cache_read_tokens: 0,
            model_name: None,
            actual_cost: None,
            raw_line: Some(raw.to_string()),
        })
    }

    /// 通用 JSONL 解析
    fn parse_generic(value: &Value, tool_name: &str, raw: &str) -> Option<RawEvent> {
        Some(RawEvent {
            id: None,
            tool_name: tool_name.to_string(),
            timestamp: value.get("timestamp").and_then(|v| v.as_str()).unwrap_or("").to_string(),
            input_tokens: value.get("input_tokens").and_then(|v| v.as_i64()).unwrap_or(0),
            output_tokens: value.get("output_tokens").and_then(|v| v.as_i64()).unwrap_or(0),
            cache_read_tokens: value.get("cache_read_tokens").and_then(|v| v.as_i64()).unwrap_or(0),
            model_name: value.get("model").and_then(|v| v.as_str()).map(String::from),
            actual_cost: None,
            raw_line: Some(raw.to_string()),
        })
    }

    /// Codex sessions rollout JSONL 解析（快照式）。
    /// 每个 session 文件中 token_count 事件的 `total_token_usage` 是会话累计值（单调递增，
    /// 即使 compacted 也不重置）；而 `last_token_usage` 逐事件求和会过计（1.1~1.27 倍）。
    /// 故按"每日末次累计值做差"得到当日真实用量：delta(d1)=cum(d1), delta(dk)=cum(dk)-cum(d_{k-1})，
    /// 各日增量之和恰等于会话最终累计值。按 (date, model) 聚合，时间戳取日期精度
    /// `YYYY-MM-DDT00:00:00`，由存储层按 (tool, date) 替换写入，避免跨运行重复累加。
    fn parse_codex_sessions(&self, dir: &Path, tool_name: &str) -> Vec<RawEvent> {
        use std::collections::BTreeMap;
        let files = self.find_jsonl_files(dir);
        // (date, model) -> (input, cached, output, reasoning)
        let mut agg: BTreeMap<(String, String), (i64, i64, i64, i64)> = BTreeMap::new();

        for file_path in &files {
            // date -> (input, cached, output, reasoning, model)，取当日末次（最大）累计值
            let mut per_day: BTreeMap<String, (i64, i64, i64, i64, Option<String>)> = BTreeMap::new();
            let mut last_model: Option<String> = None;

            let file = match OpenOptions::new().read(true).open(file_path) {
                Ok(f) => f,
                Err(e) => {
                    warn!("无法打开 codex 会话文件 {}: {}", file_path.display(), e);
                    continue;
                }
            };
            for line in BufReader::new(file).lines() {
                let line = match line {
                    Ok(l) => l,
                    Err(_) => continue,
                };
                let trimmed = line.trim();
                if trimmed.is_empty() {
                    continue;
                }
                let value: Value = match serde_json::from_str(trimmed) {
                    Ok(v) => v,
                    Err(_) => continue,
                };

                // turn_context 携带 payload.model，记录最近一次模型（按文件顺序，即时间顺序）
                if value.get("type").and_then(|v| v.as_str()) == Some("turn_context") {
                    if let Some(m) = value
                        .get("payload")
                        .and_then(|p| p.get("model"))
                        .and_then(|v| v.as_str())
                    {
                        last_model = Some(m.to_string());
                    }
                    continue;
                }

                if value.get("type").and_then(|v| v.as_str()) != Some("event_msg") {
                    continue;
                }
                let payload = match value.get("payload") {
                    Some(p) => p,
                    None => continue,
                };
                if payload.get("type").and_then(|v| v.as_str()) != Some("token_count") {
                    continue;
                }
                let tt = match payload.get("info").and_then(|i| i.get("total_token_usage")) {
                    Some(t) => t,
                    None => continue,
                };

                let input = tt.get("input_tokens").and_then(|v| v.as_i64()).unwrap_or(0);
                let cached = tt.get("cached_input_tokens").and_then(|v| v.as_i64()).unwrap_or(0);
                let output = tt.get("output_tokens").and_then(|v| v.as_i64()).unwrap_or(0);
                let reason = tt.get("reasoning_output_tokens").and_then(|v| v.as_i64()).unwrap_or(0);

                // 日期取 token_count 事件时间戳前 10 位（UTC 日期）
                let ts = value.get("timestamp").and_then(|v| v.as_str()).unwrap_or("");
                let day = match ts.get(..10) {
                    Some(d) => d.to_string(),
                    None => continue,
                };

                // 累计值单调递增，取当日末次（最大）值；更新时同步记录当日模型
                let entry = per_day.entry(day).or_insert((0, 0, 0, 0, None));
                let mut updated = false;
                if input > entry.0 { entry.0 = input; updated = true; }
                if cached > entry.1 { entry.1 = cached; updated = true; }
                if output > entry.2 { entry.2 = output; updated = true; }
                if reason > entry.3 { entry.3 = reason; updated = true; }
                if updated {
                    entry.4 = last_model.clone();
                }
            }

            // 按日做差得到当日增量，累加到 (date, model) 聚合
            let mut prev: (i64, i64, i64, i64) = (0, 0, 0, 0);
            for (day, cur) in per_day.into_iter() {
                let delta = (
                    (cur.0 - prev.0).max(0),
                    (cur.1 - prev.1).max(0),
                    (cur.2 - prev.2).max(0),
                    (cur.3 - prev.3).max(0),
                );
                let model = cur.4.unwrap_or_default();
                let e = agg.entry((day, model)).or_insert((0, 0, 0, 0));
                e.0 += delta.0;
                e.1 += delta.1;
                e.2 += delta.2;
                e.3 += delta.3;
                prev = (cur.0, cur.1, cur.2, cur.3);
            }
        }

        // 生成 RawEvent：每 (date, model) 一条，时间戳日期精度
        let mut events = Vec::new();
        for ((day, model), (input, cached, output, reason)) in agg.into_iter() {
            if input == 0 && cached == 0 && output == 0 && reason == 0 {
                continue;
            }
            events.push(RawEvent {
                id: None,
                tool_name: tool_name.to_string(),
                timestamp: format!("{}T00:00:00", day),
                input_tokens: input,
                output_tokens: output + reason, // reasoning 属于生成输出
                cache_read_tokens: cached,
                model_name: if model.is_empty() { None } else { Some(model) },
                actual_cost: None,
                raw_line: Some("codex cumulative-delta by day".to_string()),
            });
        }
        info!("从 Codex sessions 解析到 {} 条日聚合事件", events.len());
        events
    }

    /// 递归查找目录下所有 .jsonl 文件
    fn find_jsonl_files(&self, dir: &Path) -> Vec<PathBuf> {
        let mut files = Vec::new();
        if let Ok(entries) = std::fs::read_dir(dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    files.extend(self.find_jsonl_files(&path));
                } else if let Some(ext) = path.extension() {
                    if ext == "jsonl" {
                        files.push(path);
                    }
                }
            }
        }
        files.sort();
        files
    }

    fn load_pointers(path: &Path) -> HashMap<String, FilePointer> {
        if path.exists() {
            if let Ok(content) = std::fs::read_to_string(path) {
                if let Ok(pointers) = serde_json::from_str(&content) {
                    return pointers;
                }
            }
        }
        HashMap::new()
    }

    /// OpenCode SQLite 数据库解析（session 表含精确 token/cache/cost 数据）
    fn parse_opencode_sqlite(&self, log_dir: &Path, tool_name: &str) -> Vec<RawEvent> {
        let db_path = log_dir.join("opencode.db");
        if !db_path.exists() {
            info!("OpenCode 数据库不存在: {}", db_path.display());
            return vec![];
        }

        let conn = match rusqlite::Connection::open(&db_path) {
            Ok(c) => c,
            Err(e) => {
                info!("无法打开 OpenCode 数据库: {}", e);
                return vec![];
            }
        };

        let mut stmt = match conn.prepare(
            "SELECT time_created, model, tokens_input, tokens_output, tokens_cache_read, tokens_cache_write, cost \
             FROM session \
             WHERE tokens_input > 0 OR tokens_output > 0 \
             ORDER BY time_created"
        ) {
            Ok(s) => s,
            Err(e) => {
                info!("OpenCode session 表查询失败: {}", e);
                return vec![];
            }
        };

        let mut events = Vec::new();

        // 按日期聚合
        let mut daily: HashMap<String, (i64, i64, i64, f64, i16)> = HashMap::new();
        // date -> (input, output, cache_read, cost, count)
        // 注意：tokens_cache_write 是写入缓存的 token（首次写入开销），归入 input 侧成本

        let rows_result = stmt.query_map(
            [],
            |row| {
                Ok((
                    row.get(0).unwrap_or(0),
                    row.get(1).ok(),
                    row.get(2).unwrap_or(0),
                    row.get(3).unwrap_or(0),
                    row.get(4).unwrap_or(0),
                    row.get(5).unwrap_or(0),
                    row.get(6).ok(),
                ))
            },
        );

        let rows: Vec<(i64, Option<String>, i64, i64, i64, i64, Option<f64>)> = match rows_result {
            Ok(mapped) => mapped.filter_map(|r| r.ok()).collect(),
            Err(_) => vec![],
        };

        for (time_created, _model_raw, inp, out, cache_read, cache_write, cost) in &rows {
            let ts_secs = *time_created / 1000;
            let dt = match chrono::DateTime::from_timestamp(ts_secs, 0) {
                Some(d) => d.format("%Y-%m-%d").to_string(),
                None => continue,
            };

            let entry = daily.entry(dt).or_insert((0, 0, 0, 0.0, 0));
            entry.0 += *inp + *cache_write; // cache_write 归入 input 侧成本
            entry.1 += *out;
            entry.2 += *cache_read; // cache_read 是缓存命中，单独统计
            if let Some(c) = cost {
                entry.3 += c;
            }
            entry.4 += 1;
        }

        // 提取各日期的模型分布
        let mut model_daily: HashMap<(String, String), i16> = HashMap::new();
        let mut model_stmt = conn.prepare(
            "SELECT time_created, model FROM session WHERE tokens_input > 0 OR tokens_output > 0"
        ).ok();

        if let Some(ref mut ms) = model_stmt {
            if let Ok(rm) = ms.query_map([], |row| {
                Ok((row.get::<_, i64>(0).unwrap_or(0), row.get::<_, Option<String>>(1).ok()))
            }) {
                for r in rm.flatten() {
                    let (tc, model_raw) = r;
                    let ts = tc / 1000;
                    let d = match chrono::DateTime::from_timestamp(ts, 0) {
                        Some(dt) => dt.format("%Y-%m-%d").to_string(),
                        None => continue,
                    };
                    let model_name = model_raw.flatten()
                        .as_ref()
                        .map(|m| Self::parse_opencode_model(m).unwrap_or_else(|| "unknown".to_string()))
                        .unwrap_or_else(|| "unknown".to_string());
                    *model_daily.entry((d, model_name)).or_insert(0) += 1;
                }
            }
        }

        for (date, (input, output, cache_read, cost, _count)) in &daily {
            // 按该日期的模型比例分配 input/output
            let mut model_counts: HashMap<String, i16> = HashMap::new();
            for ((d, model), count) in &model_daily {
                if d == date {
                    model_counts.insert(model.clone(), *count);
                }
            }
            let total_sessions: i16 = model_counts.values().sum();

            if total_sessions == 0 {
                // 所有 session 的 model 可能无法解析，使用 "unknown"
                if *input > 0 || *output > 0 {
                    events.push(RawEvent {
                        id: None,
                        tool_name: tool_name.to_string(),
                        timestamp: format!("{}T00:00:00", date),
                        input_tokens: *input,
                        output_tokens: *output,
                        cache_read_tokens: *cache_read,
                        model_name: Some("unknown".to_string()),
                        actual_cost: if *cost > 0.0 { Some(*cost) } else { None },
                        raw_line: Some("OpenCode session aggregate".to_string()),
                    });
                }
                continue;
            }

            for (model, count) in &model_counts {
                let ratio = *count as f64 / total_sessions as f64;
                let inp = (*input as f64 * ratio) as i64;
                let out = (*output as f64 * ratio) as i64;
                let cr = (*cache_read as f64 * ratio) as i64;
                let c = *cost * ratio;

                if inp == 0 && out == 0 {
                    continue;
                }

                events.push(RawEvent {
                    id: None,
                    tool_name: tool_name.to_string(),
                    timestamp: format!("{}T00:00:00", date),
                    input_tokens: inp,
                    output_tokens: out,
                    cache_read_tokens: cr,
                    model_name: Some(model.clone()),
                    actual_cost: if c > 0.0 { Some(c) } else { None },
                    raw_line: Some(format!("OpenCode: {} sessions ({} total)", count, total_sessions)),
                });
            }
        }

        info!("OpenCode: {} 天, {} 条事件", daily.len(), events.len());
        events
    }

    /// 解析 OpenCode model JSON → 模型名
    /// 格式: {"id":"deepseek-v4-pro","providerID":"deepseek","variant":"default"}
    fn parse_opencode_model(model_raw: &str) -> Option<String> {
        // model 字段可能是 JSON 字符串或 None
        let v: serde_json::Value = match serde_json::from_str(model_raw) {
            Ok(v) => v,
            Err(_) => {
                // 不是 JSON，直接返回字符串
                return Some(model_raw.to_string());
            }
        };
        v.get("id")
            .and_then(|id| id.as_str())
            .map(|s| s.to_string())
    }

    /// ZCode SQLite 数据库解析（model_usage 表含精确 token/cache 数据）
    /// 数据源：~/.zcode/cli/db/db.sqlite 的 model_usage 表
    /// 每行一条模型调用记录，已带 model_id，按 (日期, 模型) 聚合
    fn parse_zcode_sqlite(&self, log_dir: &Path, tool_name: &str) -> Vec<RawEvent> {
        let db_path = log_dir.join("db.sqlite");
        if !db_path.exists() {
            info!("ZCode 数据库不存在: {}", db_path.display());
            return vec![];
        }

        // 只读打开，避免与运行中的 ZCode 抢锁
        let conn = match rusqlite::Connection::open_with_flags(
            &db_path,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
        ) {
            Ok(c) => c,
            Err(e) => {
                info!("无法打开 ZCode 数据库: {}", e);
                return vec![];
            }
        };

        let mut stmt = match conn.prepare(
            "SELECT completed_at, model_id, input_tokens, output_tokens, reasoning_tokens, \
                cache_creation_input_tokens, cache_read_input_tokens \
             FROM model_usage \
             WHERE (input_tokens > 0 OR output_tokens > 0) AND model_id IS NOT NULL \
             ORDER BY completed_at"
        ) {
            Ok(s) => s,
            Err(e) => {
                info!("ZCode model_usage 表查询失败: {}", e);
                return vec![];
            }
        };

        // 按日期+模型聚合：(input, output, cache_read, count)
        // input_tokens 含 cache_creation（写入缓存开销归 input 侧，与 OpenCode 约定一致）
        // output_tokens 含 reasoning（推理 token 按 output 价计费，无独立字段）
        let mut daily: HashMap<(String, String), (i64, i64, i64, i64)> = HashMap::new();

        let rows_result = stmt.query_map(
            [],
            |row| {
                Ok((
                    row.get::<_, i64>(0).unwrap_or(0),              // completed_at (ms)
                    row.get::<_, Option<String>>(1).ok().flatten(), // model_id
                    row.get::<_, i64>(2).unwrap_or(0),              // input_tokens
                    row.get::<_, i64>(3).unwrap_or(0),              // output_tokens
                    row.get::<_, i64>(4).unwrap_or(0),              // reasoning_tokens
                    row.get::<_, i64>(5).unwrap_or(0),              // cache_creation_input_tokens
                    row.get::<_, i64>(6).unwrap_or(0),              // cache_read_input_tokens
                ))
            },
        );

        let rows: Vec<(i64, Option<String>, i64, i64, i64, i64, i64)> = match rows_result {
            Ok(mapped) => mapped.filter_map(|r| r.ok()).collect(),
            Err(_) => vec![],
        };

        for (completed_at, model_raw, inp, out, reason, cache_write, cache_read) in &rows {
            let ts_secs = *completed_at / 1000;
            let dt = match chrono::DateTime::from_timestamp(ts_secs, 0) {
                Some(d) => d.format("%Y-%m-%d").to_string(),
                None => continue,
            };
            // 归一化模型名（大小写不统一：glm-5.2 / GLM-5.2 / GLM-5-Turbo）
            let model = model_raw
                .as_ref()
                .map(|m| m.to_lowercase())
                .unwrap_or_else(|| "unknown".to_string());

            let entry = daily.entry((dt, model)).or_insert((0, 0, 0, 0));
            entry.0 += *inp + *cache_write; // cache_creation 归入 input 侧
            entry.1 += *out + *reason;      // reasoning 归入 output 侧
            entry.2 += *cache_read;         // 缓存命中单独统计
            entry.3 += 1;
        }

        let mut events = Vec::new();
        for ((date, model), (input, output, cache_read, count)) in &daily {
            if *input == 0 && *output == 0 {
                continue;
            }
            events.push(RawEvent {
                id: None,
                tool_name: tool_name.to_string(),
                timestamp: format!("{}T00:00:00", date),
                input_tokens: *input,
                output_tokens: *output,
                cache_read_tokens: *cache_read,
                model_name: Some(model.clone()),
                actual_cost: None, // ZCode 无费用字段，由 estimate_cost 按 GLM-5 定价估算
                raw_line: Some(format!("ZCode: {} records (model={})", count, model)),
            });
        }

        info!("ZCode: {} 天×模型, {} 条事件", daily.len(), events.len());
        events
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::LogFormat;
    use std::io::Write;

    // ============================================================
    // 辅助函数：创建临时 JSONL 文件
    // ============================================================

    fn create_temp_jsonl(dir: &Path, filename: &str, lines: &[&str]) -> PathBuf {
        let file_path = dir.join(filename);
        let mut file = std::fs::File::create(&file_path).unwrap();
        for line in lines {
            writeln!(file, "{}", line).unwrap();
        }
        file_path
    }

    fn create_temp_data_dir() -> tempfile::TempDir {
        tempfile::tempdir().unwrap()
    }

    // ============================================================
    // Claude Code 缓存命中解析测试
    // ============================================================

    #[test]
    fn test_claude_code_cache_creation_goes_to_input_not_cache_read() {
        // 核心测试：cache_creation_input_tokens 应归入 input_tokens，不应归入 cache_read_tokens
        let _dir = create_temp_data_dir();
        let jsonl_content = r#"{"type":"assistant","message":{"id":"msg_001","usage":{"input_tokens":100,"output_tokens":50,"cache_read_input_tokens":200,"cache_creation_input_tokens":300},"model":"claude-sonnet-4-20250514"},"timestamp":"2026-06-09T10:00:00"}"#;

        let lines: Vec<String> = vec![jsonl_content.to_string()];
        let events = LogParser::parse_claude_code_session(&lines, "claude_code");

        assert_eq!(events.len(), 1, "应解析出 1 条事件");
        let event = &events[0];

        // input_tokens 应包含原始 input + cache_creation
        assert_eq!(event.input_tokens, 100 + 300,
            "input_tokens 应为原始 input(100) + cache_creation(300) = 400, 实际 {}",
            event.input_tokens);

        // cache_read_tokens 应只包含 cache_read
        assert_eq!(event.cache_read_tokens, 200,
            "cache_read_tokens 应为 200, 实际 {}", event.cache_read_tokens);

        // output_tokens 不变
        assert_eq!(event.output_tokens, 50,
            "output_tokens 应为 50, 实际 {}", event.output_tokens);
    }

    #[test]
    fn test_claude_code_cache_creation_not_in_cache_read() {
        // 反向验证：cache_creation 不应出现在 cache_read_tokens 中
        let lines: Vec<String> = vec![
            r#"{"type":"assistant","message":{"id":"msg_001","usage":{"input_tokens":100,"output_tokens":50,"cache_read_input_tokens":0,"cache_creation_input_tokens":500},"model":"claude-sonnet"},"timestamp":"2026-06-09T10:00:00"}"#.to_string(),
        ];
        let events = LogParser::parse_claude_code_session(&lines, "claude_code");

        assert_eq!(events.len(), 1);
        let event = &events[0];

        // cache_read 为 0，cache_creation 为 500
        assert_eq!(event.cache_read_tokens, 0,
            "cache_read_tokens 应为 0（没有缓存命中），实际 {}", event.cache_read_tokens);
        assert_eq!(event.input_tokens, 100 + 500,
            "input_tokens 应包含 cache_creation(500)，实际 {}", event.input_tokens);
    }

    #[test]
    fn test_claude_code_per_call_usage() {
        // Anthropic usage 是逐请求计费（非累计），每条 assistant 消息直接记录其真实 usage
        let lines: Vec<String> = vec![
            r#"{"type":"assistant","message":{"id":"msg_001","usage":{"input_tokens":100,"output_tokens":50,"cache_read_input_tokens":0,"cache_creation_input_tokens":200},"model":"claude-sonnet"},"timestamp":"2026-06-09T10:00:00"}"#.to_string(),
            r#"{"type":"assistant","message":{"id":"msg_002","usage":{"input_tokens":200,"output_tokens":100,"cache_read_input_tokens":100,"cache_creation_input_tokens":300},"model":"claude-sonnet"},"timestamp":"2026-06-09T10:01:00"}"#.to_string(),
        ];
        let events = LogParser::parse_claude_code_session(&lines, "claude_code");

        assert_eq!(events.len(), 2, "应解析出 2 条逐调用事件");

        // 第 1 条：input = 100 + 200(cache_creation) = 300, output = 50, cache_read = 0
        assert_eq!(events[0].input_tokens, 300, "第 1 条 input 应为 300");
        assert_eq!(events[0].output_tokens, 50, "第 1 条 output 应为 50");
        assert_eq!(events[0].cache_read_tokens, 0, "第 1 条 cache_read 应为 0");

        // 第 2 条：input = 200 + 300(cache_creation) = 500, output = 100, cache_read = 100
        assert_eq!(events[1].input_tokens, 500, "第 2 条 input 应为 500");
        assert_eq!(events[1].output_tokens, 100, "第 2 条 output 应为 100");
        assert_eq!(events[1].cache_read_tokens, 100, "第 2 条 cache_read 应为 100");
    }

    #[test]
    fn test_claude_code_per_call_independent() {
        // 每条调用的 usage 独立，不做差分；即使后一条数值更小也直接记录
        let lines: Vec<String> = vec![
            r#"{"type":"assistant","message":{"id":"msg_001","usage":{"input_tokens":500,"output_tokens":200,"cache_read_input_tokens":100,"cache_creation_input_tokens":0},"model":"claude-sonnet"},"timestamp":"2026-06-09T10:00:00"}"#.to_string(),
            r#"{"type":"assistant","message":{"id":"msg_002","usage":{"input_tokens":50,"output_tokens":20,"cache_read_input_tokens":10,"cache_creation_input_tokens":0},"model":"claude-sonnet"},"timestamp":"2026-06-09T11:00:00"}"#.to_string(),
        ];
        let events = LogParser::parse_claude_code_session(&lines, "claude_code");

        assert_eq!(events.len(), 2);

        // 第 1 条：直接记录 500/200/100
        assert_eq!(events[0].input_tokens, 500, "第 1 条 input 应为 500");
        assert_eq!(events[0].output_tokens, 200, "第 1 条 output 应为 200");
        assert_eq!(events[0].cache_read_tokens, 100, "第 1 条 cache_read 应为 100");

        // 第 2 条：直接记录 50/20/10（不与前一条做差）
        assert_eq!(events[1].input_tokens, 50, "第 2 条 input 应为 50");
        assert_eq!(events[1].output_tokens, 20, "第 2 条 output 应为 20");
        assert_eq!(events[1].cache_read_tokens, 10, "第 2 条 cache_read 应为 10");
    }

    #[test]
    fn test_claude_code_deduplication_by_message_id() {
        // 测试按 message.id 去重
        let lines: Vec<String> = vec![
            r#"{"type":"assistant","message":{"id":"msg_001","usage":{"input_tokens":100,"output_tokens":50,"cache_read_input_tokens":0,"cache_creation_input_tokens":0},"model":"claude-sonnet"},"timestamp":"2026-06-09T10:00:00"}"#.to_string(),
            r#"{"type":"assistant","message":{"id":"msg_001","usage":{"input_tokens":100,"output_tokens":50,"cache_read_input_tokens":0,"cache_creation_input_tokens":0},"model":"claude-sonnet"},"timestamp":"2026-06-09T10:00:00"}"#.to_string(),
        ];
        let events = LogParser::parse_claude_code_session(&lines, "claude_code");

        assert_eq!(events.len(), 1, "相同 message.id 应去重，只保留 1 条");
    }

    #[test]
    fn test_claude_code_skip_non_assistant() {
        // 非 assistant 类型的消息应被跳过
        let lines: Vec<String> = vec![
            r#"{"type":"human","message":{"content":"hello"},"timestamp":"2026-06-09T10:00:00"}"#.to_string(),
            r#"{"type":"assistant","message":{"id":"msg_001","usage":{"input_tokens":100,"output_tokens":50,"cache_read_input_tokens":0,"cache_creation_input_tokens":0},"model":"claude-sonnet"},"timestamp":"2026-06-09T10:01:00"}"#.to_string(),
        ];
        let events = LogParser::parse_claude_code_session(&lines, "claude_code");

        assert_eq!(events.len(), 1, "只有 assistant 类型应被解析");
    }

    #[test]
    fn test_claude_code_skip_zero_usage() {
        // 全零的 usage 应被跳过
        let lines: Vec<String> = vec![
            r#"{"type":"assistant","message":{"id":"msg_000","usage":{"input_tokens":0,"output_tokens":0,"cache_read_input_tokens":0,"cache_creation_input_tokens":0},"model":"claude-sonnet"},"timestamp":"2026-06-09T10:00:00"}"#.to_string(),
        ];
        let events = LogParser::parse_claude_code_session(&lines, "claude_code");

        assert_eq!(events.len(), 0, "全零 usage 应被跳过");
    }

    // ============================================================
    // OpenCode SQLite 解析测试
    // ============================================================

    #[test]
    fn test_opencode_cache_write_goes_to_input() {
        // 核心测试：tokens_cache_write 应归入 input_tokens
        let dir = create_temp_data_dir();
        let db_path = dir.path().join("opencode.db");

        let conn = rusqlite::Connection::open(&db_path).unwrap();
        conn.execute_batch(
            "CREATE TABLE session (
                id INTEGER PRIMARY KEY,
                time_created INTEGER,
                model TEXT,
                tokens_input INTEGER,
                tokens_output INTEGER,
                tokens_cache_read INTEGER,
                tokens_cache_write INTEGER,
                cost REAL
            );
            INSERT INTO session (time_created, model, tokens_input, tokens_output, tokens_cache_read, tokens_cache_write, cost)
            VALUES (1749427200000, '{\"id\":\"deepseek-v4-pro\",\"providerID\":\"deepseek\"}', 1000, 500, 200, 300, 0.05);
            "
        ).unwrap();
        drop(conn);

        let parser = LogParser::new(dir.path());
        let events = parser.parse_opencode_sqlite(dir.path(), "opencode");

        assert!(!events.is_empty(), "应解析出事件");

        // 验证 cache_write(300) 被归入 input 侧
        // input_tokens 应为 tokens_input(1000) + tokens_cache_write(300) = 1300
        let total_input: i64 = events.iter().map(|e| e.input_tokens).sum();
        let total_output: i64 = events.iter().map(|e| e.output_tokens).sum();
        let total_cache_read: i64 = events.iter().map(|e| e.cache_read_tokens).sum();

        assert_eq!(total_input, 1300,
            "input_tokens 应为 tokens_input(1000) + cache_write(300) = 1300, 实际 {}", total_input);
        assert_eq!(total_output, 500,
            "output_tokens 应为 500, 实际 {}", total_output);
        assert_eq!(total_cache_read, 200,
            "cache_read_tokens 应为 200, 实际 {}", total_cache_read);
    }

    #[test]
    fn test_opencode_cache_write_not_in_cache_read() {
        // 反向验证：tokens_cache_write 不应出现在 cache_read_tokens 中
        let dir = create_temp_data_dir();
        let db_path = dir.path().join("opencode.db");

        let conn = rusqlite::Connection::open(&db_path).unwrap();
        conn.execute_batch(
            "CREATE TABLE session (
                id INTEGER PRIMARY KEY,
                time_created INTEGER,
                model TEXT,
                tokens_input INTEGER,
                tokens_output INTEGER,
                tokens_cache_read INTEGER,
                tokens_cache_write INTEGER,
                cost REAL
            );
            INSERT INTO session (time_created, model, tokens_input, tokens_output, tokens_cache_read, tokens_cache_write, cost)
            VALUES (1749427200000, '{\"id\":\"deepseek-v4-pro\"}', 500, 200, 0, 800, 0.03);
            "
        ).unwrap();
        drop(conn);

        let parser = LogParser::new(dir.path());
        let events = parser.parse_opencode_sqlite(dir.path(), "opencode");

        assert!(!events.is_empty());

        let total_cache_read: i64 = events.iter().map(|e| e.cache_read_tokens).sum();
        assert_eq!(total_cache_read, 0,
            "cache_read_tokens 应为 0（原始 cache_read=0），实际 {}", total_cache_read);

        let total_input: i64 = events.iter().map(|e| e.input_tokens).sum();
        assert_eq!(total_input, 500 + 800,
            "input_tokens 应包含 cache_write(800)，实际 {}", total_input);
    }

    #[test]
    fn test_opencode_model_parsing() {
        // 测试 OpenCode model JSON 解析
        let result = LogParser::parse_opencode_model(r#"{"id":"deepseek-v4-pro","providerID":"deepseek","variant":"default"}"#);
        assert_eq!(result, Some("deepseek-v4-pro".to_string()));

        let result2 = LogParser::parse_opencode_model("plain-model-name");
        assert_eq!(result2, Some("plain-model-name".to_string()));

        let result3 = LogParser::parse_opencode_model(r#"{"id":"claude-sonnet-4","providerID":"anthropic"}"#);
        assert_eq!(result3, Some("claude-sonnet-4".to_string()));
    }

    #[test]
    fn test_opencode_multiple_sessions_aggregation() {
        // 测试多会话按日期聚合
        let dir = create_temp_data_dir();
        let db_path = dir.path().join("opencode.db");

        let conn = rusqlite::Connection::open(&db_path).unwrap();
        conn.execute_batch(
            "CREATE TABLE session (
                id INTEGER PRIMARY KEY,
                time_created INTEGER,
                model TEXT,
                tokens_input INTEGER,
                tokens_output INTEGER,
                tokens_cache_read INTEGER,
                tokens_cache_write INTEGER,
                cost REAL
            );
            INSERT INTO session (time_created, model, tokens_input, tokens_output, tokens_cache_read, tokens_cache_write, cost)
            VALUES (1749427200000, '{\"id\":\"deepseek-v4-pro\"}', 100, 50, 20, 10, 0.01);
            INSERT INTO session (time_created, model, tokens_input, tokens_output, tokens_cache_read, tokens_cache_write, cost)
            VALUES (1749430800000, '{\"id\":\"deepseek-v4-pro\"}', 200, 100, 40, 20, 0.02);
            "
        ).unwrap();
        drop(conn);

        let parser = LogParser::new(dir.path());
        let events = parser.parse_opencode_sqlite(dir.path(), "opencode");

        // 两条记录同一天，应聚合
        assert!(!events.is_empty());

        let total_input: i64 = events.iter().map(|e| e.input_tokens).sum();
        let total_output: i64 = events.iter().map(|e| e.output_tokens).sum();
        let total_cache_read: i64 = events.iter().map(|e| e.cache_read_tokens).sum();

        // input = (100+10) + (200+20) = 330
        assert_eq!(total_input, 330,
            "聚合 input 应为 330, 实际 {}", total_input);
        assert_eq!(total_output, 150,
            "聚合 output 应为 150, 实际 {}", total_output);
        assert_eq!(total_cache_read, 60,
            "聚合 cache_read 应为 60, 实际 {}", total_cache_read);
    }

    #[test]
    fn test_opencode_empty_db() {
        // 空数据库应返回空结果
        let dir = create_temp_data_dir();
        let db_path = dir.path().join("opencode.db");

        let conn = rusqlite::Connection::open(&db_path).unwrap();
        conn.execute_batch(
            "CREATE TABLE session (
                id INTEGER PRIMARY KEY,
                time_created INTEGER,
                model TEXT,
                tokens_input INTEGER,
                tokens_output INTEGER,
                tokens_cache_read INTEGER,
                tokens_cache_write INTEGER,
                cost REAL
            );"
        ).unwrap();
        drop(conn);

        let parser = LogParser::new(dir.path());
        let events = parser.parse_opencode_sqlite(dir.path(), "opencode");

        assert!(events.is_empty(), "空数据库应返回空结果");
    }

    // ============================================================
    // ZCode SQLite 解析测试
    // ============================================================

    #[test]
    fn test_zcode_cache_write_goes_to_input() {
        // 核心测试：cache_creation_input_tokens 应归入 input_tokens
        let dir = create_temp_data_dir();
        let db_path = dir.path().join("db.sqlite");

        let conn = rusqlite::Connection::open(&db_path).unwrap();
        conn.execute_batch(
            "CREATE TABLE model_usage (
                id INTEGER PRIMARY KEY,
                completed_at INTEGER,
                model_id TEXT,
                input_tokens INTEGER,
                output_tokens INTEGER,
                reasoning_tokens INTEGER,
                cache_creation_input_tokens INTEGER,
                cache_read_input_tokens INTEGER
            );
            INSERT INTO model_usage (completed_at, model_id, input_tokens, output_tokens, reasoning_tokens, cache_creation_input_tokens, cache_read_input_tokens)
            VALUES (1782416586082, 'glm-5.2', 1000, 500, 0, 300, 200);
            "
        ).unwrap();
        drop(conn);

        let parser = LogParser::new(dir.path());
        let events = parser.parse_zcode_sqlite(dir.path(), "zcode");

        assert!(!events.is_empty(), "应解析出事件");

        // input_tokens 应为 input(1000) + cache_creation(300) = 1300
        let total_input: i64 = events.iter().map(|e| e.input_tokens).sum();
        let total_output: i64 = events.iter().map(|e| e.output_tokens).sum();
        let total_cache_read: i64 = events.iter().map(|e| e.cache_read_tokens).sum();

        assert_eq!(total_input, 1300,
            "input_tokens 应为 input(1000) + cache_creation(300) = 1300, 实际 {}", total_input);
        assert_eq!(total_output, 500,
            "output_tokens 应为 500, 实际 {}", total_output);
        assert_eq!(total_cache_read, 200,
            "cache_read_tokens 应为 200, 实际 {}", total_cache_read);
    }

    #[test]
    fn test_zcode_reasoning_goes_to_output() {
        // reasoning_tokens 应归入 output_tokens（按 output 价计费）
        let dir = create_temp_data_dir();
        let db_path = dir.path().join("db.sqlite");

        let conn = rusqlite::Connection::open(&db_path).unwrap();
        conn.execute_batch(
            "CREATE TABLE model_usage (
                id INTEGER PRIMARY KEY,
                completed_at INTEGER,
                model_id TEXT,
                input_tokens INTEGER,
                output_tokens INTEGER,
                reasoning_tokens INTEGER,
                cache_creation_input_tokens INTEGER,
                cache_read_input_tokens INTEGER
            );
            INSERT INTO model_usage (completed_at, model_id, input_tokens, output_tokens, reasoning_tokens, cache_creation_input_tokens, cache_read_input_tokens)
            VALUES (1782416586082, 'glm-5.2', 500, 200, 800, 0, 0);
            "
        ).unwrap();
        drop(conn);

        let parser = LogParser::new(dir.path());
        let events = parser.parse_zcode_sqlite(dir.path(), "zcode");

        assert!(!events.is_empty());

        // output_tokens 应为 output(200) + reasoning(800) = 1000
        let total_output: i64 = events.iter().map(|e| e.output_tokens).sum();
        assert_eq!(total_output, 1000,
            "output_tokens 应为 output(200) + reasoning(800) = 1000, 实际 {}", total_output);

        let total_input: i64 = events.iter().map(|e| e.input_tokens).sum();
        assert_eq!(total_input, 500, "input_tokens 应为 500, 实际 {}", total_input);
    }

    #[test]
    fn test_zcode_model_normalization() {
        // 同日大小写不同的模型名应归一化并聚合
        let dir = create_temp_data_dir();
        let db_path = dir.path().join("db.sqlite");

        let conn = rusqlite::Connection::open(&db_path).unwrap();
        conn.execute_batch(
            "CREATE TABLE model_usage (
                id INTEGER PRIMARY KEY,
                completed_at INTEGER,
                model_id TEXT,
                input_tokens INTEGER,
                output_tokens INTEGER,
                reasoning_tokens INTEGER,
                cache_creation_input_tokens INTEGER,
                cache_read_input_tokens INTEGER
            );
            INSERT INTO model_usage (completed_at, model_id, input_tokens, output_tokens, reasoning_tokens, cache_creation_input_tokens, cache_read_input_tokens)
            VALUES (1782416586082, 'GLM-5.2', 100, 50, 0, 0, 0);
            INSERT INTO model_usage (completed_at, model_id, input_tokens, output_tokens, reasoning_tokens, cache_creation_input_tokens, cache_read_input_tokens)
            VALUES (1782416686082, 'glm-5.2', 200, 100, 0, 0, 0);
            "
        ).unwrap();
        drop(conn);

        let parser = LogParser::new(dir.path());
        let events = parser.parse_zcode_sqlite(dir.path(), "zcode");

        assert!(!events.is_empty());
        // GLM-5.2 与 glm-5.2 同日应归一化聚合为一条
        assert_eq!(events.len(), 1, "同日同模型(大小写不同)应聚合为一条事件");
        let total_input: i64 = events.iter().map(|e| e.input_tokens).sum();
        assert_eq!(total_input, 300, "聚合 input 应为 300, 实际 {}", total_input);
        assert_eq!(events[0].model_name.as_deref(), Some("glm-5.2"),
            "模型名应归一化为小写 glm-5.2");
    }

    #[test]
    fn test_zcode_multiple_records_aggregation() {
        // 同日同模型多条记录应聚合
        let dir = create_temp_data_dir();
        let db_path = dir.path().join("db.sqlite");

        let conn = rusqlite::Connection::open(&db_path).unwrap();
        conn.execute_batch(
            "CREATE TABLE model_usage (
                id INTEGER PRIMARY KEY,
                completed_at INTEGER,
                model_id TEXT,
                input_tokens INTEGER,
                output_tokens INTEGER,
                reasoning_tokens INTEGER,
                cache_creation_input_tokens INTEGER,
                cache_read_input_tokens INTEGER
            );
            INSERT INTO model_usage (completed_at, model_id, input_tokens, output_tokens, reasoning_tokens, cache_creation_input_tokens, cache_read_input_tokens)
            VALUES (1782416586082, 'glm-5.2', 100, 50, 10, 10, 20);
            INSERT INTO model_usage (completed_at, model_id, input_tokens, output_tokens, reasoning_tokens, cache_creation_input_tokens, cache_read_input_tokens)
            VALUES (1782416686082, 'glm-5.2', 200, 100, 20, 20, 40);
            "
        ).unwrap();
        drop(conn);

        let parser = LogParser::new(dir.path());
        let events = parser.parse_zcode_sqlite(dir.path(), "zcode");

        assert!(!events.is_empty());
        assert_eq!(events.len(), 1, "同日同模型应聚合为一条");

        let total_input: i64 = events.iter().map(|e| e.input_tokens).sum();
        let total_output: i64 = events.iter().map(|e| e.output_tokens).sum();
        let total_cache_read: i64 = events.iter().map(|e| e.cache_read_tokens).sum();

        // input = (100+10) + (200+20) = 330
        assert_eq!(total_input, 330, "聚合 input 应为 330, 实际 {}", total_input);
        // output = (50+10) + (100+20) = 180
        assert_eq!(total_output, 180, "聚合 output 应为 180, 实际 {}", total_output);
        assert_eq!(total_cache_read, 60, "聚合 cache_read 应为 60, 实际 {}", total_cache_read);
    }

    #[test]
    fn test_zcode_empty_db() {
        // 空数据库应返回空结果
        let dir = create_temp_data_dir();
        let db_path = dir.path().join("db.sqlite");

        let conn = rusqlite::Connection::open(&db_path).unwrap();
        conn.execute_batch(
            "CREATE TABLE model_usage (
                id INTEGER PRIMARY KEY,
                completed_at INTEGER,
                model_id TEXT,
                input_tokens INTEGER,
                output_tokens INTEGER,
                reasoning_tokens INTEGER,
                cache_creation_input_tokens INTEGER,
                cache_read_input_tokens INTEGER
            );"
        ).unwrap();
        drop(conn);

        let parser = LogParser::new(dir.path());
        let events = parser.parse_zcode_sqlite(dir.path(), "zcode");

        assert!(events.is_empty(), "空数据库应返回空结果");
    }

    // ============================================================
    // DeepSeek GUI 解析测试
    // ============================================================

    #[test]
    fn test_deepseek_gui_delta_calculation() {
        let lines: Vec<String> = vec![
            r#"{"kind":"usage","turnId":"t1","timestamp":"2026-06-09T10:00:00","model":"deepseek-v4-pro","usage":{"promptTokens":1000,"completionTokens":500,"cachedTokens":200,"costUsd":0.05}}"#.to_string(),
            r#"{"kind":"usage","turnId":"t2","timestamp":"2026-06-09T10:01:00","model":"deepseek-v4-pro","usage":{"promptTokens":3000,"completionTokens":1200,"cachedTokens":600,"costUsd":0.15}}"#.to_string(),
        ];
        let events = LogParser::parse_deepseek_gui_with_delta(&lines, "deepseek_gui");

        assert_eq!(events.len(), 2, "应解析出 2 条增量事件");

        // 第 1 条：绝对值
        assert_eq!(events[0].input_tokens, 1000);
        assert_eq!(events[0].output_tokens, 500);
        assert_eq!(events[0].cache_read_tokens, 200);

        // 第 2 条：增量
        assert_eq!(events[1].input_tokens, 2000, "增量 prompt = 3000-1000");
        assert_eq!(events[1].output_tokens, 700, "增量 completion = 1200-500");
        assert_eq!(events[1].cache_read_tokens, 400, "增量 cached = 600-200");
    }

    #[test]
    fn test_deepseek_gui_actual_cost() {
        let lines: Vec<String> = vec![
            r#"{"kind":"usage","turnId":"t1","timestamp":"2026-06-09T10:00:00","model":"deepseek-v4-pro","usage":{"promptTokens":1000,"completionTokens":500,"cachedTokens":200,"costUsd":0.05}}"#.to_string(),
        ];
        let events = LogParser::parse_deepseek_gui_with_delta(&lines, "deepseek_gui");

        assert_eq!(events.len(), 1);
        assert_eq!(events[0].actual_cost, Some(0.05), "应保留实际费用 costUsd");
    }

    #[test]
    fn test_deepseek_gui_skip_non_usage() {
        let lines: Vec<String> = vec![
            r#"{"kind":"response","turnId":"t1","timestamp":"2026-06-09T10:00:00"}"#.to_string(),
        ];
        let events = LogParser::parse_deepseek_gui_with_delta(&lines, "deepseek_gui");

        assert!(events.is_empty(), "非 usage 类型应被跳过");
    }

    // ============================================================
    // Cursor 模型映射测试
    // ============================================================

    #[test]
    fn test_cursor_model_mapping() {
        assert_eq!(LogParser::map_cursor_model("default"), "claude-sonnet");
        assert_eq!(LogParser::map_cursor_model("composer-2"), "claude-sonnet");
        assert_eq!(LogParser::map_cursor_model("composer-2.5"), "claude-sonnet");
        assert_eq!(LogParser::map_cursor_model("premium"), "claude-opus");
        assert_eq!(LogParser::map_cursor_model(""), "claude-sonnet");
        assert_eq!(LogParser::map_cursor_model("gpt-4o"), "gpt-4o");
    }

    // ============================================================
    // JSONL 文件解析集成测试
    // ============================================================

    #[test]
    fn test_jsonl_file_parsing_claude_code() {
        let dir = create_temp_data_dir();
        let log_dir = dir.path().join("claude_logs");
        std::fs::create_dir_all(&log_dir).unwrap();

        create_temp_jsonl(&log_dir, "session1.jsonl", &[
            r#"{"type":"assistant","message":{"id":"msg_001","usage":{"input_tokens":100,"output_tokens":50,"cache_read_input_tokens":30,"cache_creation_input_tokens":70},"model":"claude-sonnet"},"timestamp":"2026-06-09T10:00:00"}"#,
        ]);

        let mut parser = LogParser::new(dir.path());
        let tool_config = crate::config::ToolConfig {
            name: "claude_code".to_string(),
            display_name: "Claude Code".to_string(),
            description: "test".to_string(),
            log_format: LogFormat::ClaudeCodeJsonl,
            custom_log_path: Some(log_dir.to_string_lossy().to_string()),
            has_token_data: true,
        };

        let events = parser.parse_tool_logs(&tool_config);
        assert_eq!(events.len(), 1, "应解析出 1 条事件");
        assert_eq!(events[0].input_tokens, 100 + 70, "input 应包含 cache_creation");
        assert_eq!(events[0].cache_read_tokens, 30, "cache_read 应为 30");
    }

    // ============================================================
    // 通用 JSONL 解析测试
    // ============================================================

    #[test]
    fn test_generic_jsonl_parsing() {
        let dir = create_temp_data_dir();
        let log_dir = dir.path().join("generic_logs");
        std::fs::create_dir_all(&log_dir).unwrap();

        create_temp_jsonl(&log_dir, "data.jsonl", &[
            r#"{"timestamp":"2026-06-09T10:00:00","input_tokens":100,"output_tokens":50,"cache_read_tokens":20,"model":"test-model"}"#,
            r#"{"timestamp":"2026-06-09T11:00:00","input_tokens":200,"output_tokens":100,"cache_read_tokens":40}"#,
        ]);

        let mut parser = LogParser::new(dir.path());
        let tool_config = crate::config::ToolConfig {
            name: "test_tool".to_string(),
            display_name: "Test Tool".to_string(),
            description: "test".to_string(),
            log_format: LogFormat::GenericJsonl,
            custom_log_path: Some(log_dir.to_string_lossy().to_string()),
            has_token_data: true,
        };

        let events = parser.parse_tool_logs(&tool_config);
        assert_eq!(events.len(), 2, "应解析出 2 条事件");
        assert_eq!(events[0].input_tokens, 100);
        assert_eq!(events[1].output_tokens, 100);
    }

    // ============================================================
    // Codex sessions 解析测试（累计值按日做差）
    // ============================================================

    #[test]
    fn test_codex_sessions_cumulative_delta_by_day() {
        // total_token_usage 是会话累计值（单调递增）；按"每日末次累计值做差"得到当日用量
        let dir = create_temp_data_dir();
        let lines = [
            // turn_context 提供 model
            r#"{"timestamp":"2026-06-09T10:00:00.000Z","type":"turn_context","payload":{"model":"glm-5.2","turn_id":"t1"}}"#,
            // Day1: 累计值增长 100→200
            r#"{"timestamp":"2026-06-09T10:00:00.000Z","type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":100,"cached_input_tokens":0,"output_tokens":10,"reasoning_output_tokens":0,"total_tokens":110},"last_token_usage":{}}}}"#,
            r#"{"timestamp":"2026-06-09T11:00:00.000Z","type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":200,"cached_input_tokens":50,"output_tokens":20,"reasoning_output_tokens":0,"total_tokens":270},"last_token_usage":{}}}}"#,
            // Day2: 累计值增长到 500
            r#"{"timestamp":"2026-06-10T10:00:00.000Z","type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":500,"cached_input_tokens":100,"output_tokens":40,"reasoning_output_tokens":0,"total_tokens":640},"last_token_usage":{}}}}"#,
        ];
        create_temp_jsonl(dir.path(), "rollout-test.jsonl", &lines);

        let mut parser = LogParser::new(dir.path());
        let events = parser.parse_codex_sessions(dir.path(), "codex");

        // 2 天 → 2 条日聚合事件
        assert_eq!(events.len(), 2, "应解析出 2 条日聚合事件，实际 {}", events.len());

        // 按日期升序（BTreeMap）
        assert_eq!(events[0].timestamp, "2026-06-09T00:00:00");
        assert_eq!(events[0].input_tokens, 200, "Day1 input = 末次累计 200 - 0");
        assert_eq!(events[0].cache_read_tokens, 50, "Day1 cache = 50 - 0");
        assert_eq!(events[0].output_tokens, 20, "Day1 output = 20 - 0");
        assert_eq!(events[0].model_name.as_deref(), Some("glm-5.2"));

        assert_eq!(events[1].timestamp, "2026-06-10T00:00:00");
        assert_eq!(events[1].input_tokens, 300, "Day2 input = 500 - 200");
        assert_eq!(events[1].cache_read_tokens, 50, "Day2 cache = 100 - 50");
        assert_eq!(events[1].output_tokens, 20, "Day2 output = 40 - 20");
    }

    #[test]
    fn test_codex_sessions_skips_files_without_token_data() {
        // 无 token_count 事件的会话文件应被跳过
        let dir = create_temp_data_dir();
        let lines = [
            r#"{"timestamp":"2026-06-09T10:00:00.000Z","type":"session_meta","payload":{"id":"s1"}}"#,
            r#"{"timestamp":"2026-06-09T10:00:00.000Z","type":"event_msg","payload":{"type":"task_complete"}}"#,
        ];
        create_temp_jsonl(dir.path(), "rollout-empty.jsonl", &lines);

        let mut parser = LogParser::new(dir.path());
        let events = parser.parse_codex_sessions(dir.path(), "codex");
        assert!(events.is_empty(), "无 token 数据的文件不应产生事件");
    }

    // ============================================================
    // 文件指针持久化测试
    // ============================================================

    #[test]
    fn test_file_pointer_save_and_load() {
        let dir = create_temp_data_dir();
        let mut parser = LogParser::new(dir.path());

        // 模拟插入指针
        parser.pointers.insert("test.jsonl".to_string(), FilePointer {
            file_path: "test.jsonl".to_string(),
            offset: 1024,
            last_modified: 1234567890,
        });

        parser.save_pointers();

        // 重新加载
        let loaded = LogParser::load_pointers(&dir.path().join("file_pointers.json"));
        assert!(loaded.contains_key("test.jsonl"), "应能加载保存的指针");
        assert_eq!(loaded["test.jsonl"].offset, 1024, "偏移量应为 1024");
    }
}
