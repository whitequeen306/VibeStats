use chrono::NaiveDate;
use serde::{Deserialize, Serialize};

/// 从日志中解析出的原始事件
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RawEvent {
    pub id: Option<i64>,
    pub tool_name: String,
    pub timestamp: String,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cache_read_tokens: i64,
    pub model_name: Option<String>,
    /// 实际费用（如果日志中有，如 DeepSeek GUI 的 costUsd）
    pub actual_cost: Option<f64>,
    pub raw_line: Option<String>,
}

/// 按天聚合的统计数据
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DailyStats {
    pub id: Option<i64>,
    pub date: String,
    pub tool_name: String,
    pub total_input_tokens: i64,
    pub total_output_tokens: i64,
    pub total_cache_read_tokens: i64,
    pub estimated_cost: f64,
    pub code_lines_equivalent: i64,
    pub opus4_equivalent: f64,
    pub event_count: i64,
}

/// 聚合视图（用于前端展示）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AggregatedStats {
    pub dates: Vec<String>,
    pub by_tool: Vec<ToolDailyStats>,
    pub totals: DailyTotals,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolDailyStats {
    pub tool_name: String,
    pub daily_data: Vec<SingleDayStats>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SingleDayStats {
    pub date: String,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cache_read_tokens: i64,
    pub estimated_cost: f64,
    pub code_lines_equivalent: i64,
    pub opus4_equivalent: f64,
    pub event_count: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DailyTotals {
    pub total_input_tokens: i64,
    pub total_output_tokens: i64,
    pub total_cache_read_tokens: i64,
    pub total_estimated_cost: f64,
    pub total_code_lines: i64,
    pub total_opus4_equivalent: f64,
    pub total_events: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrendPoint {
    pub bucket: String,
    pub tool_name: String,
    pub estimated_cost: f64,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cache_read_tokens: i64,
    pub event_count: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrendResponse {
    pub granularity: String,
    pub buckets: Vec<String>,
    pub series: Vec<ToolTrendSeries>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolTrendSeries {
    pub tool_name: String,
    pub points: Vec<TrendPointValue>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrendPointValue {
    pub bucket: String,
    pub estimated_cost: f64,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cache_read_tokens: i64,
    pub event_count: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NotificationMessage {
    pub title: String,
    pub body: String,
}

/// 缓存统计数据（按工具+日期聚合）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CacheStats {
    pub tool_name: String,
    pub date: String,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cache_read_tokens: i64,
}

/// 缓存统计 API 响应
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CacheStatsResponse {
    pub by_tool: Vec<CacheToolStats>,
    pub totals: CacheTotals,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CacheToolStats {
    pub tool_name: String,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cache_read_tokens: i64,
    pub cache_hit_rate: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CacheTotals {
    pub total_input_tokens: i64,
    pub total_output_tokens: i64,
    pub total_cache_read_tokens: i64,
    pub overall_cache_hit_rate: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FilePointer {
    pub file_path: String,
    pub offset: u64,
    pub last_modified: u64,
}

// ============================================================
// 模型定价表（每百万 Token 的美元价格；计费统一按人民币显示，
// aggregate_daily 出口按 USD_TO_RMB 折算，改汇率只需动此常量 + 重算）
// ============================================================

/// 美元→人民币汇率默认值（1 USD = 7.2 CNY）。运行时实际值由 get_usd_to_rmb() 返回，
/// 可在设置页修改并触发历史重算；启动时从 config.exchange_rate 注入。
pub const DEFAULT_USD_TO_RMB: f64 = 7.2;

// 运行时汇率（OnceLock<RwLock<f64>>，仿 PRICING_OVERRIDES 模式）
static USD_TO_RMB_RUNTIME: std::sync::OnceLock<std::sync::RwLock<f64>> = std::sync::OnceLock::new();

fn usd_to_rmb_lock() -> &'static std::sync::RwLock<f64> {
    USD_TO_RMB_RUNTIME.get_or_init(|| std::sync::RwLock::new(DEFAULT_USD_TO_RMB))
}

/// 取当前美元→人民币汇率（运行时值，未注入时为 DEFAULT_USD_TO_RMB）
pub fn get_usd_to_rmb() -> f64 {
    *usd_to_rmb_lock().read().unwrap()
}

/// 设置运行时汇率（设置页保存时调用，随后 recompute_all 重算历史费用）
pub fn set_usd_to_rmb(v: f64) {
    *usd_to_rmb_lock().write().unwrap() = v;
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ModelPricing {
    #[serde(rename = "input")]
    pub input_per_mtok: f64,
    #[serde(rename = "output")]
    pub output_per_mtok: f64,
    #[serde(rename = "cache_read")]
    pub cache_read_per_mtok: f64,
}

// ============================================================
// 模型定价覆盖（运行时从 config.toml 的 [pricing_overrides] 加载）
// 启动时由 set_pricing_overrides 设入；get_model_pricing 优先查此表，
// 未命中再走下方硬编码分支。key 为模型名小写全名，精确匹配。
// ============================================================
static PRICING_OVERRIDES: std::sync::OnceLock<
    std::sync::RwLock<Option<std::collections::HashMap<String, ModelPricing>>>,
> = std::sync::OnceLock::new();

/// 取覆盖表锁（首次访问时初始化为 None）
fn pricing_overrides_lock(
) -> &'static std::sync::RwLock<Option<std::collections::HashMap<String, ModelPricing>>> {
    PRICING_OVERRIDES.get_or_init(|| std::sync::RwLock::new(None))
}

/// 纯函数：在给定覆盖表中按小写全名精确查找（便于无全局态单测）
fn lookup_override(
    map: &std::collections::HashMap<String, ModelPricing>,
    model: &str,
) -> Option<ModelPricing> {
    map.get(&model.to_lowercase()).cloned()
}

/// 启动时注入覆盖表（来自 Config.pricing_overrides）
pub fn set_pricing_overrides(map: std::collections::HashMap<String, ModelPricing>) {
    *pricing_overrides_lock().write().unwrap() = Some(map);
}

/// 清除覆盖表（测试用）
#[cfg(test)]
pub fn clear_pricing_overrides() {
    *pricing_overrides_lock().write().unwrap() = None;
}

/// 测试用：串行化所有调用 set_pricing_overrides 的测试，避免并行互相覆盖全局覆盖表
#[cfg(test)]
pub(crate) static PRICING_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// 获取模型定价
pub fn get_model_pricing(model: &str) -> ModelPricing {
    // 优先使用配置覆盖（精确匹配小写全名），未命中再走硬编码
    {
        let guard = pricing_overrides_lock().read().unwrap();
        if let Some(map) = guard.as_ref() {
            if let Some(p) = lookup_override(map, model) {
                return p;
            }
        }
    }
    let model_lower = model.to_lowercase();

    // DeepSeek 模型（官方定价，USD/MTok；cache_read 为缓存命中价）
    if model_lower.contains("deepseek") {
        // DeepSeek-V4-Pro: input $0.435 / output $0.87 / cache_read $0.003625
        if model_lower.contains("v4-pro") {
            return ModelPricing { input_per_mtok: 0.435, output_per_mtok: 0.87, cache_read_per_mtok: 0.003625 };
        }
        // DeepSeek-V3 (deepseek-chat): input $0.27 / output $1.10 / cache_read $0.07
        if model_lower.contains("v3") {
            return ModelPricing { input_per_mtok: 0.27, output_per_mtok: 1.10, cache_read_per_mtok: 0.07 };
        }
        // DeepSeek-V4-Flash: input $0.14 / output $0.28 / cache_read $0.0028
        // （官方说明 legacy deepseek-chat 已并入 V4-Flash 非思考模式）
        if model_lower.contains("v4-flash") || model_lower.contains("flash") {
            return ModelPricing { input_per_mtok: 0.14, output_per_mtok: 0.28, cache_read_per_mtok: 0.0028 };
        }
        // DeepSeek 默认：按官方说明等同 V4-Flash 非思考模式
        return ModelPricing { input_per_mtok: 0.14, output_per_mtok: 0.28, cache_read_per_mtok: 0.0028 };
    }

    // Claude 模型
    if model_lower.contains("opus") {
        return ModelPricing { input_per_mtok: 15.0, output_per_mtok: 75.0, cache_read_per_mtok: 1.50 };
    }
    if model_lower.contains("sonnet") {
        return ModelPricing { input_per_mtok: 3.0, output_per_mtok: 15.0, cache_read_per_mtok: 0.30 };
    }
    if model_lower.contains("haiku") {
        return ModelPricing { input_per_mtok: 0.80, output_per_mtok: 4.0, cache_read_per_mtok: 0.08 };
    }

    // GPT 模型（OpenAI 官方定价，USD/MTok；来源：GPT-5 发布定价 2025-08）
    // gpt-5.x 系列（含 5.5/5.4/5.2 等）统一按 GPT-5 价估算，子型号差异忽略
    if model_lower.contains("gpt-5-mini") || model_lower.contains("gpt5-mini") {
        // input $0.25, output $2, cached $0.125（输入缓存 5 折）
        return ModelPricing { input_per_mtok: 0.25, output_per_mtok: 2.0, cache_read_per_mtok: 0.125 };
    }
    if model_lower.contains("gpt-5") || model_lower.contains("gpt5") {
        // input $1.25, output $10, cached $0.625（输入缓存 5 折）
        return ModelPricing { input_per_mtok: 1.25, output_per_mtok: 10.0, cache_read_per_mtok: 0.625 };
    }
    if model_lower.contains("gpt-4") || model_lower.contains("gpt4") {
        return ModelPricing { input_per_mtok: 2.50, output_per_mtok: 10.0, cache_read_per_mtok: 1.25 };
    }
    if model_lower.contains("o3") {
        return ModelPricing { input_per_mtok: 10.0, output_per_mtok: 40.0, cache_read_per_mtok: 2.50 };
    }
    if model_lower.contains("o4") {
        return ModelPricing { input_per_mtok: 15.0, output_per_mtok: 60.0, cache_read_per_mtok: 3.75 };
    }

    // Gemini 模型
    if model_lower.contains("gemini") {
        if model_lower.contains("pro") {
            return ModelPricing { input_per_mtok: 1.25, output_per_mtok: 10.0, cache_read_per_mtok: 0.30 };
        }
        return ModelPricing { input_per_mtok: 0.50, output_per_mtok: 2.0, cache_read_per_mtok: 0.10 };
    }

    // Qwen 模型
    if model_lower.contains("qwen") {
        return ModelPricing { input_per_mtok: 0.80, output_per_mtok: 4.0, cache_read_per_mtok: 0.10 };
    }

    // GLM 模型（智谱 AI / Z.AI 官方定价，USD/MTok）
    // 来源：docs.z.ai 定价页，GLM-4.5 / GLM-4.6 / GLM-5 系列同价
    if model_lower.contains("glm") {
        // input $0.6, cached $0.11, output $2.2 per MTok
        return ModelPricing { input_per_mtok: 0.60, output_per_mtok: 2.20, cache_read_per_mtok: 0.11 };
    }

    // Doubao 模型（字节跳动/豆包）
    if model_lower.contains("doubao") || model_lower.contains("seed") {
        // Doubao-Seed 系列：约 ¥0.03/千tokens ≈ $0.42/MTok
        return ModelPricing { input_per_mtok: 0.42, output_per_mtok: 0.42, cache_read_per_mtok: 0.06 };
    }

    // Kimi 模型（月之暗面）
    if model_lower.contains("kimi") || model_lower.contains("moonshot") {
        return ModelPricing { input_per_mtok: 0.80, output_per_mtok: 4.0, cache_read_per_mtok: 0.10 };
    }

    // MiniMax 模型
    if model_lower.contains("minimax") {
        return ModelPricing { input_per_mtok: 0.50, output_per_mtok: 2.0, cache_read_per_mtok: 0.08 };
    }

    // 默认定价（保守估计，接近 GPT-4o 级别）
    ModelPricing { input_per_mtok: 2.50, output_per_mtok: 10.0, cache_read_per_mtok: 1.25 }
}

/// 趣味指标换算参数
pub struct FunMetrics;

impl FunMetrics {
    pub fn tokens_to_code_lines(total_tokens: i64) -> i64 {
        (total_tokens as f64 / 15.0) as i64
    }

    pub fn cost_to_opus4_equivalent(cost: f64) -> f64 {
        // cost 为人民币；折算为"等价 Opus4 输出 token 数（MTok）"：
        // 该花费按 Opus4 输出价 $75/MTok 能买到的输出量 = cost / (75 × 汇率)
        cost / (75.0 * get_usd_to_rmb())
    }

    /// 按模型定价估算费用
    pub fn estimate_cost(input_tokens: i64, output_tokens: i64, cache_read_tokens: i64, model: Option<&str>) -> f64 {
        let pricing = get_model_pricing(model.unwrap_or("default"));
        let input_cost = (input_tokens as f64 / 1_000_000.0) * pricing.input_per_mtok;
        let output_cost = (output_tokens as f64 / 1_000_000.0) * pricing.output_per_mtok;
        let cache_cost = (cache_read_tokens as f64 / 1_000_000.0) * pricing.cache_read_per_mtok;
        input_cost + output_cost + cache_cost
    }

    /// 生成趣味通知文案
    pub fn format_morning_notification(
        date: &NaiveDate,
        tool_stats: &[(String, i64, f64)],
        total_cost: f64,
        total_code_lines: i64,
        total_opus4: f64,
    ) -> NotificationMessage {
        let date_str = date.format("%Y-%m-%d").to_string();
        let tool_details: Vec<String> = tool_stats
            .iter()
            .filter(|(_, _, cost)| *cost > 0.0)
            .map(|(name, lines, cost)| format!("{}: {} 行代码 (≈ ¥{:.2})", name, lines, cost))
            .collect();
        let tool_detail_str = if tool_details.is_empty() { "无消耗".to_string() } else { tool_details.join("，") };

        let title = format!("VibeStats 晨间报告 - {}", date_str);
        let body = format!(
            "早上好！昨日你用 VibeCoding 敲出了约 {} 行代码（≈ ¥{:.2} / {:.1} 个 Opus4），总计燃烧了 ¥{:.2}！\n{}",
            total_code_lines, total_cost, total_opus4, total_cost, tool_detail_str
        );

        NotificationMessage { title, body }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ============================================================
    // 模型定价测试
    // ============================================================

    #[test]
    fn test_claude_sonnet_pricing() {
        let p = get_model_pricing("claude-sonnet-4-20250514");
        assert_eq!(p.input_per_mtok, 3.0, "Claude Sonnet input 应为 $3/MTok");
        assert_eq!(p.output_per_mtok, 15.0, "Claude Sonnet output 应为 $15/MTok");
        assert_eq!(p.cache_read_per_mtok, 0.30, "Claude Sonnet cache_read 应为 $0.30/MTok");
    }

    #[test]
    fn test_claude_opus_pricing() {
        let p = get_model_pricing("claude-opus-4");
        assert_eq!(p.input_per_mtok, 15.0, "Claude Opus input 应为 $15/MTok");
        assert_eq!(p.output_per_mtok, 75.0, "Claude Opus output 应为 $75/MTok");
        assert_eq!(p.cache_read_per_mtok, 1.50, "Claude Opus cache_read 应为 $1.50/MTok");
    }

    #[test]
    fn test_claude_haiku_pricing() {
        let p = get_model_pricing("claude-haiku-3.5");
        assert_eq!(p.input_per_mtok, 0.80, "Claude Haiku input 应为 $0.80/MTok");
        assert_eq!(p.output_per_mtok, 4.0, "Claude Haiku output 应为 $4/MTok");
        assert_eq!(p.cache_read_per_mtok, 0.08, "Claude Haiku cache_read 应为 $0.08/MTok");
    }

    #[test]
    fn test_deepseek_v4_pro_pricing() {
        let p = get_model_pricing("deepseek-v4-pro");
        assert_eq!(p.input_per_mtok, 0.435, "DeepSeek V4 Pro input 应为 $0.435/MTok");
        assert_eq!(p.output_per_mtok, 0.87, "DeepSeek V4 Pro output 应为 $0.87/MTok");
        assert_eq!(p.cache_read_per_mtok, 0.003625, "DeepSeek V4 Pro cache_read 应为 $0.003625/MTok");
    }

    #[test]
    fn test_deepseek_v3_pricing() {
        let p = get_model_pricing("deepseek-v3");
        assert_eq!(p.input_per_mtok, 0.27, "DeepSeek V3 input 应为 $0.27/MTok");
        assert_eq!(p.output_per_mtok, 1.10, "DeepSeek V3 output 应为 $1.10/MTok");
        assert_eq!(p.cache_read_per_mtok, 0.07, "DeepSeek V3 cache_read 应为 $0.07/MTok");
    }

    #[test]
    fn test_deepseek_flash_pricing() {
        let p = get_model_pricing("deepseek-v4-flash");
        assert_eq!(p.input_per_mtok, 0.14, "DeepSeek Flash input 应为 $0.14/MTok");
        assert_eq!(p.output_per_mtok, 0.28, "DeepSeek Flash output 应为 $0.28/MTok");
        assert_eq!(p.cache_read_per_mtok, 0.0028, "DeepSeek Flash cache_read 应为 $0.0028/MTok");
    }

    #[test]
    fn test_deepseek_default_pricing() {
        let p = get_model_pricing("deepseek-chat");
        assert_eq!(p.input_per_mtok, 0.14, "DeepSeek 默认 input 应为 $0.14/MTok");
        assert_eq!(p.output_per_mtok, 0.28, "DeepSeek 默认 output 应为 $0.28/MTok");
    }

    #[test]
    fn test_gpt5_pricing() {
        // OpenAI GPT-5 发布定价（2025-08）：input $1.25 / output $10 / cached $0.625
        let p = get_model_pricing("gpt-5");
        assert_eq!(p.input_per_mtok, 1.25, "GPT-5 input 应为 $1.25/MTok");
        assert_eq!(p.output_per_mtok, 10.0, "GPT-5 output 应为 $10/MTok");
        assert_eq!(p.cache_read_per_mtok, 0.625, "GPT-5 cached 应为 $0.625/MTok");
    }

    #[test]
    fn test_gpt5_mini_pricing() {
        // OpenAI GPT-5 mini：input $0.25 / output $2 / cached $0.125
        let p = get_model_pricing("gpt-5-mini");
        assert_eq!(p.input_per_mtok, 0.25, "GPT-5 Mini input 应为 $0.25/MTok");
        assert_eq!(p.output_per_mtok, 2.0, "GPT-5 Mini output 应为 $2/MTok");
        assert_eq!(p.cache_read_per_mtok, 0.125, "GPT-5 Mini cached 应为 $0.125/MTok");
    }

    #[test]
    fn test_gpt4_pricing() {
        let p = get_model_pricing("gpt-4o");
        assert_eq!(p.input_per_mtok, 2.50, "GPT-4 input 应为 $2.50/MTok");
        assert_eq!(p.output_per_mtok, 10.0, "GPT-4 output 应为 $10/MTok");
    }

    #[test]
    fn test_o3_pricing() {
        let p = get_model_pricing("o3");
        assert_eq!(p.input_per_mtok, 10.0, "O3 input 应为 $10/MTok");
        assert_eq!(p.output_per_mtok, 40.0, "O3 output 应为 $40/MTok");
    }

    #[test]
    fn test_o4_pricing() {
        let p = get_model_pricing("o4-mini");
        assert_eq!(p.input_per_mtok, 15.0, "O4 input 应为 $15/MTok");
        assert_eq!(p.output_per_mtok, 60.0, "O4 output 应为 $60/MTok");
    }

    #[test]
    fn test_gemini_pro_pricing() {
        let p = get_model_pricing("gemini-2.5-pro");
        assert_eq!(p.input_per_mtok, 1.25, "Gemini Pro input 应为 $1.25/MTok");
        assert_eq!(p.output_per_mtok, 10.0, "Gemini Pro output 应为 $10/MTok");
    }

    #[test]
    fn test_gemini_default_pricing() {
        let p = get_model_pricing("gemini-2.0-flash");
        assert_eq!(p.input_per_mtok, 0.50, "Gemini 默认 input 应为 $0.50/MTok");
        assert_eq!(p.output_per_mtok, 2.0, "Gemini 默认 output 应为 $2/MTok");
    }

    #[test]
    fn test_qwen_pricing() {
        let p = get_model_pricing("qwen-max");
        assert_eq!(p.input_per_mtok, 0.80, "Qwen input 应为 $0.80/MTok");
        assert_eq!(p.output_per_mtok, 4.0, "Qwen output 应为 $4/MTok");
    }

    #[test]
    fn test_glm5_pricing() {
        // 智谱 Z.AI 官方定价：GLM-5 系列 input $0.6 / output $2.2 / cached $0.11
        let p = get_model_pricing("glm-5.1");
        assert_eq!(p.input_per_mtok, 0.60, "GLM-5 input 应为 $0.6/MTok");
        assert_eq!(p.output_per_mtok, 2.20, "GLM-5 output 应为 $2.2/MTok");
        assert_eq!(p.cache_read_per_mtok, 0.11, "GLM-5 cached 应为 $0.11/MTok");
    }

    #[test]
    fn test_glm4_pricing() {
        // 智谱 Z.AI 官方定价：GLM-4.5/4.6 系列 input $0.6 / output $2.2 / cached $0.11
        let p = get_model_pricing("glm-4-plus");
        assert_eq!(p.input_per_mtok, 0.60, "GLM-4 input 应为 $0.6/MTok");
        assert_eq!(p.output_per_mtok, 2.20, "GLM-4 output 应为 $2.2/MTok");
        assert_eq!(p.cache_read_per_mtok, 0.11, "GLM-4 cached 应为 $0.11/MTok");
    }

    #[test]
    fn test_doubao_pricing() {
        let p = get_model_pricing("doubao-seed-1.6");
        assert_eq!(p.input_per_mtok, 0.42, "Doubao input 应为 $0.42/MTok");
        assert_eq!(p.output_per_mtok, 0.42, "Doubao output 应为 $0.42/MTok");
    }

    #[test]
    fn test_kimi_pricing() {
        let p = get_model_pricing("moonshot-v1");
        assert_eq!(p.input_per_mtok, 0.80, "Kimi/Moonshot input 应为 $0.80/MTok");
        assert_eq!(p.output_per_mtok, 4.0, "Kimi/Moonshot output 应为 $4/MTok");
    }

    #[test]
    fn test_minimax_pricing() {
        let p = get_model_pricing("minimax-text-01");
        assert_eq!(p.input_per_mtok, 0.50, "MiniMax input 应为 $0.50/MTok");
        assert_eq!(p.output_per_mtok, 2.0, "MiniMax output 应为 $2/MTok");
    }

    #[test]
    fn test_default_pricing() {
        let p = get_model_pricing("unknown-model-xyz");
        assert_eq!(p.input_per_mtok, 2.50, "默认 input 应为 $2.50/MTok");
        assert_eq!(p.output_per_mtok, 10.0, "默认 output 应为 $10/MTok");
        assert_eq!(p.cache_read_per_mtok, 1.25, "默认 cache_read 应为 $1.25/MTok");
    }

    #[test]
    fn test_pricing_case_insensitive() {
        let p1 = get_model_pricing("Claude-Sonnet");
        let p2 = get_model_pricing("claude-sonnet");
        assert_eq!(p1, p2, "模型名匹配应不区分大小写");
    }

    // ============================================================
    // 定价覆盖（[pricing_overrides]）测试
    // ============================================================

    #[test]
    fn test_lookup_override_exact_match() {
        let mut map = std::collections::HashMap::new();
        map.insert(
            "glm-5.2".to_string(),
            ModelPricing { input_per_mtok: 2.0, output_per_mtok: 8.0, cache_read_per_mtok: 0.20 },
        );
        let p = lookup_override(&map, "glm-5.2").expect("精确匹配应命中");
        assert_eq!(p.input_per_mtok, 2.0);
        assert_eq!(p.output_per_mtok, 8.0);
        assert_eq!(p.cache_read_per_mtok, 0.20);
    }

    #[test]
    fn test_lookup_override_case_insensitive() {
        let mut map = std::collections::HashMap::new();
        map.insert(
            "glm-5.2".to_string(),
            ModelPricing { input_per_mtok: 2.0, output_per_mtok: 8.0, cache_read_per_mtok: 0.20 },
        );
        // 传入大写模型名也应命中（内部 to_lowercase）
        let p = lookup_override(&map, "GLM-5.2").expect("大小写不敏感应命中");
        assert_eq!(p.input_per_mtok, 2.0);
    }

    #[test]
    fn test_lookup_override_miss() {
        let mut map = std::collections::HashMap::new();
        map.insert(
            "glm-5.2".to_string(),
            ModelPricing { input_per_mtok: 2.0, output_per_mtok: 8.0, cache_read_per_mtok: 0.20 },
        );
        // 未列出的模型不应命中
        assert!(lookup_override(&map, "claude-sonnet").is_none());
        // 精确匹配：不应模糊命中 glm-5.1
        assert!(lookup_override(&map, "glm-5.1").is_none());
    }

    #[test]
    fn test_get_model_pricing_uses_global_override() {
        // 串行化所有触碰全局覆盖表的测试，避免 set_pricing_overrides 互相覆盖
        let _guard = PRICING_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        // 用唯一假模型名，避免与并行硬编码测试竞态
        set_pricing_overrides(std::collections::HashMap::from([(
            "test-override-model".to_string(),
            ModelPricing { input_per_mtok: 1.25, output_per_mtok: 5.0, cache_read_per_mtok: 0.50 },
        )]));
        // 覆盖的模型命中覆盖值
        let p = get_model_pricing("test-override-model");
        assert_eq!(p.input_per_mtok, 1.25, "覆盖模型应命中覆盖值");
        assert_eq!(p.output_per_mtok, 5.0);
        assert_eq!(p.cache_read_per_mtok, 0.50);
        // 未覆盖的模型仍走硬编码（claude-sonnet = $3/$15/$0.30）
        let p2 = get_model_pricing("claude-sonnet-4");
        assert_eq!(p2.input_per_mtok, 3.0, "未覆盖模型应走硬编码");
        // 清除后，假模型回到默认定价
        clear_pricing_overrides();
        let p3 = get_model_pricing("test-override-model");
        assert_eq!(p3.input_per_mtok, 2.50, "清除覆盖后应回退默认定价");
        assert_eq!(p3.output_per_mtok, 10.0);
    }

    // ============================================================
    // 费用估算测试
    // ============================================================

    #[test]
    fn test_estimate_cost_claude_sonnet() {
        // 1M input + 1M output + 1M cache_read = $3 + $15 + $0.30 = $18.30
        let cost = FunMetrics::estimate_cost(1_000_000, 1_000_000, 1_000_000, Some("claude-sonnet"));
        let expected = 3.0 + 15.0 + 0.30;
        assert!(
            (cost - expected).abs() < 0.01,
            "Claude Sonnet 费用计算错误: 期望 ${:.2}, 实际 ${:.2}",
            expected, cost
        );
    }

    #[test]
    fn test_estimate_cost_no_model() {
        // 无模型名应使用默认定价
        let cost = FunMetrics::estimate_cost(1_000_000, 0, 0, None);
        let expected = 2.50; // 默认 input $2.50/MTok
        assert!(
            (cost - expected).abs() < 0.01,
            "无模型费用计算错误: 期望 ${:.2}, 实际 ${:.2}",
            expected, cost
        );
    }

    #[test]
    fn test_estimate_cost_zero_tokens() {
        let cost = FunMetrics::estimate_cost(0, 0, 0, Some("claude-sonnet"));
        assert_eq!(cost, 0.0, "零 token 费用应为 $0");
    }

    // ============================================================
    // 趣味指标测试
    // ============================================================

    #[test]
    fn test_tokens_to_code_lines() {
        assert_eq!(FunMetrics::tokens_to_code_lines(150), 10, "150 token = 10 行");
        assert_eq!(FunMetrics::tokens_to_code_lines(0), 0, "0 token = 0 行");
        assert_eq!(FunMetrics::tokens_to_code_lines(15), 1, "15 token = 1 行");
    }

    #[test]
    fn test_cost_to_opus4_equivalent() {
        // Opus4 输出价 $75/MTok；cost_to_opus4 接收人民币，按汇率折算 = 等价输出 MTok 数
        // ¥(75 × 汇率) 应等于 1 MTok 等价
        let eq = FunMetrics::cost_to_opus4_equivalent(75.0 * DEFAULT_USD_TO_RMB);
        assert!((eq - 1.0).abs() < 0.01, "¥{:.2} 应等于 1 MTok Opus4", 75.0 * DEFAULT_USD_TO_RMB);
        let eq2 = FunMetrics::cost_to_opus4_equivalent(750.0 * DEFAULT_USD_TO_RMB);
        assert!((eq2 - 10.0).abs() < 0.01, "¥{:.2} 应等于 10 MTok Opus4", 750.0 * DEFAULT_USD_TO_RMB);
    }

    // ============================================================
    // 缓存命中率计算测试（模拟 dashboard 中的逻辑）
    // ============================================================

    #[test]
    fn test_cache_hit_rate_only_cache_supporting_tools() {
        // 模拟 dashboard.rs 中 get_cache_stats 的缓存命中率计算逻辑
        // 只有 claude_code, deepseek_gui, opencode 有缓存数据
        let cache_supporting_tools: std::collections::HashSet<&str> =
            ["claude_code", "deepseek_gui", "opencode"].iter().copied().collect();

        // 模拟数据：cursor 有 input 但无 cache_read
        let stats = vec![
            CacheStats { tool_name: "claude_code".to_string(), date: "2026-06-09".to_string(), input_tokens: 1000, output_tokens: 500, cache_read_tokens: 800 },
            CacheStats { tool_name: "cursor".to_string(), date: "2026-06-09".to_string(), input_tokens: 2000, output_tokens: 1000, cache_read_tokens: 0 },
            CacheStats { tool_name: "deepseek_gui".to_string(), date: "2026-06-09".to_string(), input_tokens: 500, output_tokens: 300, cache_read_tokens: 200 },
        ];

        // 计算总体命中率（只统计有缓存数据的工具）
        let (cache_input, cache_only) = stats.iter()
            .filter(|s| cache_supporting_tools.contains(s.tool_name.as_str()))
            .fold((0i64, 0i64), |(input_acc, cache_acc), s| {
                (input_acc + s.input_tokens, cache_acc + s.cache_read_tokens)
            });

        let overall_total_readable = cache_input + cache_only;
        let overall_hit_rate = if overall_total_readable > 0 {
            cache_only as f64 / overall_total_readable as f64 * 100.0
        } else {
            0.0
        };

        // claude_code: input=1000, cache=800; deepseek_gui: input=500, cache=200
        // 总 input=1500, 总 cache=1000
        // 命中率 = 1000 / (1500 + 1000) * 100 = 40%
        assert_eq!(cache_input, 1500, "缓存支持工具的 input 总和应为 1500");
        assert_eq!(cache_only, 1000, "缓存支持工具的 cache_read 总和应为 1000");
        assert!((overall_hit_rate - 40.0).abs() < 0.1,
            "总体缓存命中率应为 40%, 实际 {:.1}%", overall_hit_rate);
    }

    #[test]
    fn test_cache_hit_rate_cursor_not_in_denominator() {
        // 验证 Cursor 的 input_tokens 不参与缓存命中率分母
        let cache_supporting_tools: std::collections::HashSet<&str> =
            ["claude_code", "deepseek_gui", "opencode"].iter().copied().collect();

        let stats = vec![
            CacheStats { tool_name: "cursor".to_string(), date: "2026-06-09".to_string(), input_tokens: 99999, output_tokens: 99999, cache_read_tokens: 0 },
        ];

        let (cache_input, cache_only) = stats.iter()
            .filter(|s| cache_supporting_tools.contains(s.tool_name.as_str()))
            .fold((0i64, 0i64), |(input_acc, cache_acc), s| {
                (input_acc + s.input_tokens, cache_acc + s.cache_read_tokens)
            });

        assert_eq!(cache_input, 0, "Cursor 不应在缓存命中率分母中");
        assert_eq!(cache_only, 0, "Cursor 的 cache_read 应为 0");
    }

    #[test]
    fn test_cache_hit_rate_per_tool() {
        // 单工具命中率 = cache_read / (input + cache_read) * 100
        let input = 1000i64;
        let cache = 500i64;
        let total_readable = input + cache;
        let hit_rate = if total_readable > 0 {
            cache as f64 / total_readable as f64 * 100.0
        } else {
            0.0
        };
        assert!((hit_rate - 33.33).abs() < 0.1,
            "单工具命中率应为 33.33%, 实际 {:.2}%", hit_rate);
    }

    #[test]
    fn test_cache_hit_rate_zero_tokens() {
        let input = 0i64;
        let cache = 0i64;
        let total_readable = input + cache;
        let hit_rate = if total_readable > 0 {
            cache as f64 / total_readable as f64 * 100.0
        } else {
            0.0
        };
        assert_eq!(hit_rate, 0.0, "零 token 命中率应为 0%");
    }
}
