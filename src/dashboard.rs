use std::path::Path;

use actix_files as fs;
use actix_web::{web, App, HttpServer, HttpResponse};
use log::info;

use crate::config::Config;
use crate::models::{AggregatedStats, CacheStatsResponse, CacheToolStats, CacheTotals, ModelPricing, TrendResponse, ToolTrendSeries, TrendPointValue};
use crate::storage::Storage;

const TOOL_COLORS: &[(&str, &str)] = &[
    ("claude_code", "#7C3AED"),
    ("deepseek_gui", "#00B4D8"),
    ("cursor", "#F59E0B"),
    ("codex", "#10B981"),
    ("copilot_jb", "#6366F1"),
    ("trae_cn", "#EF4444"),
    ("lingma", "#EC4899"),
    ("opencoder", "#8B5CF6"),
    ("windsurf", "#14B8A6"),
    ("aider", "#F97316"),
    ("cline", "#06B6D4"),
    ("roo_code", "#84CC16"),
    ("continue_dev", "#A855F7"),
    ("github_copilot", "#3B82F6"),
    ("amazon_q", "#FF6B35"),
];

#[allow(dead_code)]
fn get_tool_color(tool_name: &str) -> &'static str {
    TOOL_COLORS.iter()
        .find(|(name, _)| *name == tool_name)
        .map(|(_, color)| *color)
        .unwrap_or("#94A3B8")
}

pub struct Dashboard;

impl Dashboard {
    pub async fn serve(config: &Config, storage: Storage) -> anyhow::Result<()> {
        let port = config.serve_port;
        let data_dir = Config::data_dir();
        info!("启动 Web Dashboard，地址: http://localhost:{}", port);

        let storage_data = web::Data::new(std::sync::Mutex::new(storage));
        let config_data = web::Data::new(std::sync::RwLock::new(config.clone()));
        let data_dir_clone = data_dir.clone();

        HttpServer::new(move || {
            App::new()
                .app_data(storage_data.clone())
                .app_data(config_data.clone())
                .route("/api/stats", web::get().to(get_stats))
                .route("/api/stats/range", web::get().to(get_stats_range))
                .route("/api/trend", web::get().to(get_trend))
                .route("/api/tools", web::get().to(get_tools))
                .route("/api/builtin-tools", web::get().to(get_builtin_tools))
                .route("/api/dates", web::get().to(get_dates))
                .route("/api/cache-stats", web::get().to(get_cache_stats))
                .route("/api/pricing", web::get().to(get_pricing))
                .route("/api/pricing", web::put().to(update_pricing))
        .route("/pricing", web::get().to(pricing_page))
        .route("/settings", web::get().to(settings_page))
        .route("/api/settings", web::get().to(get_settings))
        .route("/api/settings", web::put().to(update_settings))
        .route("/", web::get().to(index))
                .service(fs::Files::new("/static", data_dir_clone.join("static")).show_files_listing())
        })
        .bind(format!("127.0.0.1:{}", port))?
        .run()
        .await?;
        Ok(())
    }

    pub fn generate_static_report(storage: &Storage, output_path: &Path) -> anyhow::Result<()> {
        let today = chrono::Local::now().date_naive();
        let yesterday = today - chrono::Duration::days(1);
        let aggregated = crate::stats::StatsEngine::get_aggregated_range(storage, &yesterday, &yesterday)?;
        let html = render_dashboard_html(&aggregated)?;
        if let Some(parent) = output_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(output_path, html)?;
        info!("静态报告已生成: {}", output_path.display());
        Ok(())
    }
}

async fn get_stats(storage: web::Data<std::sync::Mutex<Storage>>) -> HttpResponse {
    let storage = storage.lock().unwrap();
    match storage.get_all_daily_stats() {
        Ok(stats) => HttpResponse::Ok().json(stats),
        Err(e) => HttpResponse::InternalServerError().body(format!("Error: {}", e)),
    }
}

async fn get_stats_range(
    storage: web::Data<std::sync::Mutex<Storage>>,
    query: web::Query<RangeQuery>,
) -> HttpResponse {
    let storage = storage.lock().unwrap();
    let start = chrono::NaiveDate::parse_from_str(&query.start, "%Y-%m-%d");
    let end = chrono::NaiveDate::parse_from_str(&query.end, "%Y-%m-%d");
    match (start, end) {
        (Ok(s), Ok(e)) => {
            match crate::stats::StatsEngine::get_aggregated_range(&storage, &s, &e) {
                Ok(stats) => HttpResponse::Ok().json(stats),
                Err(err) => HttpResponse::InternalServerError().body(format!("Error: {}", err)),
            }
        }
        _ => HttpResponse::BadRequest().body("Invalid date format. Use YYYY-MM-DD"),
    }
}

async fn get_trend(
    storage: web::Data<std::sync::Mutex<Storage>>,
    query: web::Query<TrendQuery>,
) -> HttpResponse {
    let storage = storage.lock().unwrap();
    let now = chrono::Local::now();
    let (start, end, granularity) = match query.range.as_str() {
        "1d" => {
            // 近一天 = 今天 00:00:00 到明天 00:00:00（补齐整24小时内的所有时段）
            let today = now.format("%Y-%m-%d").to_string();
            let tomorrow = (now + chrono::Duration::days(1)).format("%Y-%m-%d").to_string();
            (format!("{}T00:00", today), format!("{}T00:00", tomorrow), "hour".to_string())
        }
        "1w" => {
            let s = (now - chrono::Duration::days(7)).format("%Y-%m-%d").to_string();
            let e = now.format("%Y-%m-%d").to_string();
            (s, e, "day".to_string())
        }
        "1m" => {
            let s = (now - chrono::Duration::days(30)).format("%Y-%m-%d").to_string();
            let e = now.format("%Y-%m-%d").to_string();
            (s, e, "day".to_string())
        }
        _ => {
            let s = (now - chrono::Duration::days(7)).format("%Y-%m-%d").to_string();
            let e = now.format("%Y-%m-%d").to_string();
            (s, e, "day".to_string())
        }
    };

    match storage.get_trend_points(&start, &end, &granularity) {
        Ok(points) => {
            // 收集所有 bucket 和 tool_name
            let mut bucket_set = std::collections::BTreeSet::new();
            let mut tool_set = std::collections::BTreeSet::new();
            for p in &points {
                bucket_set.insert(p.bucket.clone());
                tool_set.insert(p.tool_name.clone());
            }
            let buckets: Vec<String> = bucket_set.into_iter().collect();

            // 按 tool 分组
            let mut series_map: std::collections::HashMap<String, Vec<TrendPointValue>> =
                std::collections::HashMap::new();
            for p in points {
                series_map.entry(p.tool_name.clone()).or_default().push(TrendPointValue {
                    bucket: p.bucket,
                    estimated_cost: p.estimated_cost,
                    input_tokens: p.input_tokens,
                    output_tokens: p.output_tokens,
                    cache_read_tokens: p.cache_read_tokens,
                    event_count: p.event_count,
                });
            }

            let series: Vec<ToolTrendSeries> = tool_set.into_iter().map(|tool_name| {
                let mut pts = series_map.remove(&tool_name).unwrap_or_default();
                pts.sort_by(|a, b| a.bucket.cmp(&b.bucket));
                ToolTrendSeries { tool_name, points: pts }
            }).collect();

            let resp = TrendResponse { granularity, buckets, series };
            HttpResponse::Ok().json(resp)
        }
        Err(e) => HttpResponse::InternalServerError().body(format!("Error: {}", e)),
    }
}

async fn get_tools(storage: web::Data<std::sync::Mutex<Storage>>) -> HttpResponse {
    let storage = storage.lock().unwrap();
    match storage.get_tool_names() {
        Ok(tools) => HttpResponse::Ok().json(tools),
        Err(e) => HttpResponse::InternalServerError().body(format!("Error: {}", e)),
    }
}

async fn get_builtin_tools(config: web::Data<std::sync::RwLock<Config>>) -> HttpResponse {
    let config = config.read().unwrap();
    let status = config.all_tools_status();
    HttpResponse::Ok().json(status)
}

// 模型定价：返回当前生效价（覆盖优先，否则内置硬编码），覆盖所有 DB 出现过的模型 + 已有覆盖项
async fn get_pricing(
    config: web::Data<std::sync::RwLock<Config>>,
    storage: web::Data<std::sync::Mutex<Storage>>,
) -> HttpResponse {
    let mut map = config.read().unwrap().pricing_overrides.clone();
    let models = storage.lock().unwrap().get_distinct_models().unwrap_or_default();
    for model in models {
        // 统一小写为 key，避免同一模型大小写不同出现两行
        let key = model.to_lowercase();
        map.entry(key).or_insert_with(|| crate::models::get_model_pricing(&model));
    }
    // 兜底定价也作为可编辑行：覆盖 model_name 为空（未识别）的事件
    map.entry("default".to_string())
        .or_insert_with(|| crate::models::get_model_pricing("default"));
    // 返回 {rate, currency, symbol, models}：定价编辑器按 rate 把 USD 存储价换算为 ¥ 展示
    HttpResponse::Ok().json(serde_json::json!({
        "rate": crate::models::get_usd_to_rmb(),
        "currency": "CNY",
        "symbol": "¥",
        "models": map,
    }))
}

// 模型定价：整体替换 pricing_overrides，更新全局表 + 按新价重算 + 落盘
// 顺序原则：先设全局表→重算→成功才落盘；重算失败则回滚全局表，避免磁盘/历史费用不一致
async fn update_pricing(
    config: web::Data<std::sync::RwLock<Config>>,
    storage: web::Data<std::sync::Mutex<Storage>>,
    body: web::Json<std::collections::HashMap<String, ModelPricing>>,
) -> HttpResponse {
    let new_map = body.into_inner();
    // 旧值用于重算失败时回滚全局表（config_data 与全局表启动时同源、且仅此处同步更新）
    let old_map = config.read().unwrap().pricing_overrides.clone();
    // 1) 先更新进程全局覆盖表，recompute 通过 get_model_pricing 读取此表用新价
    crate::models::set_pricing_overrides(new_map.clone());
    // 2) 用新定价重算所有有原始事件的日期
    let recompute = {
        let storage = storage.lock().unwrap();
        crate::stats::StatsEngine::recompute_all(&storage)
    };
    match recompute {
        Ok(n) => {
            // 3) 重算成功才更新内存配置并落盘（内存与全局表/历史费用保持一致）
            let mut config = config.write().unwrap();
            config.pricing_overrides = new_map;
            match config.save_to_file(&Config::config_path()) {
                Ok(()) => HttpResponse::Ok().json(serde_json::json!({
                    "ok": true,
                    "recomputed_dates": n,
                })),
                Err(e) => HttpResponse::InternalServerError().body(format!(
                    "重算已完成但保存配置失败（本次已生效，重启将回退）: {}", e
                )),
            }
        }
        Err(e) => {
            // 重算失败：回滚全局表到旧值，磁盘与内存配置保持不变
            crate::models::set_pricing_overrides(old_map);
            HttpResponse::InternalServerError()
                .body(format!("重算失败，已回滚未保存: {}", e))
        }
    }
}

// 定价编辑页
async fn pricing_page() -> HttpResponse {
    HttpResponse::Ok().content_type("text/html").body(render_pricing_html())
}

// 设置页保存请求体（主题与轮询为前端 localStorage，不在此）
#[derive(serde::Deserialize)]
struct SettingsUpdate {
    exchange_rate: Option<f64>,
    schedule_time: Option<String>,
    enabled_tools: Option<Vec<String>>,
    custom_paths: Option<std::collections::HashMap<String, String>>,
}

// 设置页：返回当前数据配置（汇率/调度时间/启用工具/路径 + 内置工具状态供前端渲染）
async fn get_settings(
    config: web::Data<std::sync::RwLock<Config>>,
) -> HttpResponse {
    let cfg = config.read().unwrap();
    HttpResponse::Ok().json(serde_json::json!({
        "exchange_rate": crate::models::get_usd_to_rmb(),
        "schedule_time": cfg.schedule_time,
        "enabled_tools": cfg.enabled_tools,
        "custom_paths": cfg.custom_paths,
        "builtin_tools": cfg.all_tools_status(),
    }))
}

// 设置页：保存数据配置。汇率变动需 recompute_all 重算历史
// 顺序原则同 update_pricing：先设运行时汇率→重算→成功才落盘；重算失败回滚运行时汇率
async fn update_settings(
    config: web::Data<std::sync::RwLock<Config>>,
    storage: web::Data<std::sync::Mutex<Storage>>,
    body: web::Json<SettingsUpdate>,
) -> HttpResponse {
    let req = body.into_inner();

    // 校验调度时间 HH:MM
    if let Some(st) = &req.schedule_time {
        let parts: Vec<&str> = st.split(':').collect();
        let valid = parts.len() == 2
            && parts[0].parse::<u32>().map(|h| h < 24).unwrap_or(false)
            && parts[1].parse::<u32>().map(|m| m < 60).unwrap_or(false);
        if !valid {
            return HttpResponse::BadRequest().body("schedule_time 格式应为 HH:MM");
        }
    }
    // 校验汇率
    if let Some(r) = req.exchange_rate {
        if !(r > 0.0 && r.is_finite()) {
            return HttpResponse::BadRequest().body("exchange_rate 必须为正数");
        }
    }

    // 汇率变动：先设运行时值→重算→成功才落盘；失败回滚运行时值
    let mut recomputed: Option<usize> = None;
    if let Some(new_rate) = req.exchange_rate {
        let old_rate = crate::models::get_usd_to_rmb();
        if (new_rate - old_rate).abs() > f64::EPSILON {
            crate::models::set_usd_to_rmb(new_rate);
            let recompute = {
                let storage = storage.lock().unwrap();
                crate::stats::StatsEngine::recompute_all(&storage)
            };
            match recompute {
                Ok(n) => recomputed = Some(n),
                Err(e) => {
                    crate::models::set_usd_to_rmb(old_rate);
                    return HttpResponse::InternalServerError()
                        .body(format!("重算失败，已回滚未保存: {}", e));
                }
            }
        }
    }

    // 落盘全部配置项（schedule_time/enabled_tools/custom_paths 需重启 scheduler 生效，前端已提示）
    let mut cfg = config.write().unwrap();
    if let Some(r) = req.exchange_rate {
        cfg.exchange_rate = r;
    }
    if let Some(st) = req.schedule_time {
        cfg.schedule_time = st;
    }
    if let Some(t) = req.enabled_tools {
        cfg.enabled_tools = t;
    }
    if let Some(p) = req.custom_paths {
        cfg.custom_paths = p;
    }
    match cfg.save_to_file(&Config::config_path()) {
        Ok(()) => HttpResponse::Ok().json(serde_json::json!({
            "ok": true,
            "recomputed_dates": recomputed,
        })),
        Err(e) => HttpResponse::InternalServerError().body(format!(
            "配置已生效但保存失败（重启将回退）: {}", e
        )),
    }
}

// 设置页
async fn settings_page() -> HttpResponse {
    HttpResponse::Ok().content_type("text/html").body(render_settings_html())
}

async fn get_dates(storage: web::Data<std::sync::Mutex<Storage>>) -> HttpResponse {
    let storage = storage.lock().unwrap();
    match storage.get_dates() {
        Ok(dates) => HttpResponse::Ok().json(dates),
        Err(e) => HttpResponse::InternalServerError().body(format!("Error: {}", e)),
    }
}

async fn get_cache_stats(
    storage: web::Data<std::sync::Mutex<Storage>>,
    query: web::Query<CacheStatsQuery>,
) -> HttpResponse {
    let storage = storage.lock().unwrap();
    let start = &query.start;
    let end = &query.end;

    match storage.get_cache_stats(start, end) {
        Ok(stats) => {
            // 按工具聚合
            let mut tool_map: std::collections::HashMap<String, (i64, i64, i64)> = std::collections::HashMap::new();
            for s in &stats {
                let entry = tool_map.entry(s.tool_name.clone()).or_insert((0, 0, 0));
                entry.0 += s.input_tokens;
                entry.1 += s.output_tokens;
                entry.2 += s.cache_read_tokens;
            }

            let mut total_input = 0i64;
            let mut total_output = 0i64;
            let mut total_cache = 0i64;
            // 仅 Claude Code、DeepSeek GUI、OpenCode、ZCode 有缓存数据
            let cache_supporting_tools: std::collections::HashSet<&str> =
                ["claude_code", "deepseek_gui", "opencode", "zcode"].iter().copied().collect();

            let by_tool: Vec<CacheToolStats> = tool_map.into_iter().map(|(tool_name, (input, output, cache))| {
                total_input += input;
                total_output += output;
                total_cache += cache;
                let total_readable = input + cache;
                let hit_rate = if total_readable > 0 {
                    cache as f64 / total_readable as f64 * 100.0
                } else {
                    0.0
                };
                CacheToolStats {
                    tool_name,
                    input_tokens: input,
                    output_tokens: output,
                    cache_read_tokens: cache,
                    cache_hit_rate: hit_rate,
                }
            }).collect();

            // 总体命中率只计算有缓存数据的工具
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

            let resp = CacheStatsResponse {
                by_tool,
                totals: CacheTotals {
                    total_input_tokens: total_input,
                    total_output_tokens: total_output,
                    total_cache_read_tokens: total_cache,
                    overall_cache_hit_rate: overall_hit_rate,
                },
            };
            HttpResponse::Ok().json(resp)
        }
        Err(e) => HttpResponse::InternalServerError().body(format!("Error: {}", e)),
    }
}

async fn index(storage: web::Data<std::sync::Mutex<Storage>>) -> HttpResponse {
    let storage = storage.lock().unwrap();
    let today = chrono::Local::now().date_naive();
    let yesterday = today - chrono::Duration::days(1);

    match crate::stats::StatsEngine::get_aggregated_range(&storage, &yesterday, &yesterday) {
        Ok(aggregated) => {
            match render_dashboard_html(&aggregated) {
                Ok(html) => HttpResponse::Ok().content_type("text/html").body(html),
                Err(e) => HttpResponse::InternalServerError().body(format!("Render error: {}", e)),
            }
        }
        Err(e) => HttpResponse::InternalServerError().body(format!("Error: {}", e)),
    }
}

#[derive(serde::Deserialize)]
struct RangeQuery {
    start: String,
    end: String,
}

#[derive(serde::Deserialize)]
struct TrendQuery {
    range: String,
}

#[derive(serde::Deserialize)]
struct CacheStatsQuery {
    start: String,
    end: String,
}

fn render_dashboard_html(stats: &AggregatedStats) -> anyhow::Result<String> {
    let stats_json = serde_json::to_string(stats)?;
    let color_map_js: Vec<String> = TOOL_COLORS.iter()
        .map(|(name, color)| format!("'{}': '{}'", name, color))
        .collect();
    let color_map_js = color_map_js.join(", ");

    let yesterday = chrono::Local::now().date_naive() - chrono::Duration::days(1);
    let yesterday_str = yesterday.format("%Y-%m-%d").to_string();

    let html = format!(r##"<!DOCTYPE html>
<html lang="zh-CN">
<head>
    <meta charset="UTF-8">
    <meta name="viewport" content="width=device-width, initial-scale=1.0">
    <title>VibeStats - VibeCoding 趣味仪表盘</title>
    <script src="https://cdn.jsdelivr.net/npm/echarts@5/dist/echarts.min.js"></script>
    <script>(function(){{var t=localStorage.getItem('vibestats-theme')||'light';document.documentElement.dataset.theme=t;}})();</script>
    <style>
        :root {{
            --bg-deep: #F5F7FA;
            --bg-surface: #FFFFFF;
            --bg-elevated: #FFFFFF;
            --bg-card: rgba(255,255,255,0.82);
            --border-subtle: rgba(15,23,42,0.08);
            --border-medium: rgba(15,23,42,0.12);
            --text-primary: #0F172A;
            --text-secondary: #475569;
            --text-muted: #64748B;
            --accent-cyan: #0891B2;
            --accent-pink: #DB2777;
            --accent-amber: #D97706;
            --accent-lime: #059669;
            --accent-violet: #7C3AED;
            --gradient-hero: linear-gradient(135deg, #00B4D8 0%, #7B61FF 50%, #DB2777 100%);
            --gradient-card: linear-gradient(180deg, rgba(15,23,42,0.025) 0%, rgba(15,23,42,0) 100%);
            --shadow-glow: 0 0 40px rgba(8,145,178,0.06);
            --radius-lg: 20px;
            --radius-md: 14px;
            --radius-sm: 10px;
            --chart-text: #475569;
            --chart-border: #FFFFFF;
            --gauge-tick: #94A3B8;
            --overlay-faint: rgba(15,23,42,0.03);
            --overlay-soft: rgba(15,23,42,0.05);
        }}
        [data-theme="dark"] {{
            --bg-deep: #0B0F1A;
            --bg-surface: #111827;
            --bg-elevated: #1A2236;
            --bg-card: rgba(26,34,54,0.7);
            --border-subtle: rgba(255,255,255,0.06);
            --border-medium: rgba(255,255,255,0.1);
            --text-primary: #F0F2F7;
            --text-secondary: #8B95A8;
            --text-muted: #5A6478;
            --accent-cyan: #00E5FF;
            --accent-pink: #FF2E7D;
            --accent-amber: #FFB800;
            --accent-lime: #7EE787;
            --accent-violet: #A78BFA;
            --gradient-hero: linear-gradient(135deg, #00E5FF 0%, #7B61FF 50%, #FF2E7D 100%);
            --gradient-card: linear-gradient(180deg, rgba(255,255,255,0.06) 0%, rgba(255,255,255,0.02) 100%);
            --shadow-glow: 0 0 40px rgba(0,229,255,0.08);
            --chart-text: #ccc;
            --chart-border: #1A2236;
            --gauge-tick: #fff;
            --overlay-faint: rgba(255,255,255,0.02);
            --overlay-soft: rgba(255,255,255,0.04);
        }}

        * {{ margin: 0; padding: 0; box-sizing: border-box; }}

        body {{
            font-family: -apple-system, BlinkMacSystemFont, 'Segoe UI', Roboto, 'Helvetica Neue', Arial, 'PingFang SC', 'Microsoft YaHei', sans-serif;
            background: var(--bg-deep);
            color: var(--text-primary);
            min-height: 100vh;
            overflow-x: hidden;
        }}

        /* 背景噪点纹理 */
        body::before {{
            content: '';
            position: fixed; inset: 0;
            background-image: url("data:image/svg+xml,%3Csvg viewBox='0 0 256 256' xmlns='http://www.w3.org/2000/svg'%3E%3Cfilter id='n'%3E%3CfeTurbulence type='fractalNoise' baseFrequency='0.9' numOctaves='4' stitchTiles='stitch'/%3E%3C/filter%3E%3Crect width='100%25' height='100%25' filter='url(%23n)' opacity='0.03'/%3E%3C/svg%3E");
            pointer-events: none; z-index: 0;
        }}

        /* 顶部光晕 */
        .hero-glow {{
            position: fixed; top: -200px; left: 50%; transform: translateX(-50%);
            width: 800px; height: 400px;
            background: radial-gradient(ellipse, rgba(0,229,255,0.12) 0%, transparent 70%);
            pointer-events: none; z-index: 0;
        }}

        .app {{ position: relative; z-index: 1; }}

        /* 顶部光晕 */
        .header {{
            text-align: center; padding: 48px 20px 16px;
            position: relative;
        }}
        .header-badge {{
            display: inline-flex; align-items: center; gap: 8px;
            padding: 6px 16px; border-radius: 100px;
            background: rgba(0,229,255,0.08); border: 1px solid rgba(0,229,255,0.15);
            font-size: 0.78em; color: var(--accent-cyan); font-weight: 500;
            letter-spacing: 0.5px; margin-bottom: 16px;
        }}
        .header-badge::before {{
            content: ''; width: 6px; height: 6px; border-radius: 50%;
            background: var(--accent-cyan); animation: pulse 2s ease-in-out infinite;
        }}
        @keyframes pulse {{
            0%, 100% {{ opacity: 1; transform: scale(1); }}
            50% {{ opacity: 0.4; transform: scale(0.7); }}
        }}
        .header h1 {{
            font-family: -apple-system, BlinkMacSystemFont, 'Segoe UI', Roboto, 'Helvetica Neue', Arial, 'PingFang SC', 'Microsoft YaHei', sans-serif;
            font-size: 3.2em; font-weight: 700; letter-spacing: -1.5px;
            background: var(--gradient-hero);
            -webkit-background-clip: text; -webkit-text-fill-color: transparent; background-clip: text;
            line-height: 1.1;
        }}
        .header .tagline {{
            color: var(--text-secondary); margin-top: 10px;
            font-size: 1em; font-weight: 300; letter-spacing: 0.5px;
        }}
        .header .date-label {{
            color: var(--text-muted); margin-top: 14px; font-size: 0.9em;
            font-weight: 400; letter-spacing: 1px; text-transform: uppercase;
        }}

        .container {{ max-width: 1400px; margin: 0 auto; padding: 0 24px 40px; }}

        /* ===== Controls ===== */
        .controls {{
            display: flex; gap: 10px; margin-bottom: 24px;
            justify-content: center; align-items: center; flex-wrap: wrap;
        }}
        .time-btn {{
            padding: 9px 22px; border: 1px solid var(--border-subtle);
            background: var(--bg-elevated); color: var(--text-secondary);
            border-radius: 12px; cursor: pointer; font-size: 0.88em;
            font-family: inherit; font-weight: 500;
            transition: all 0.25s cubic-bezier(0.4, 0, 0.2, 1);
            letter-spacing: 0.3px;
        }}
        .time-btn:hover {{
            border-color: rgba(0,229,255,0.3); color: var(--text-primary);
            transform: translateY(-1px);
            box-shadow: 0 4px 20px rgba(0,229,255,0.08);
        }}
        .time-btn.active {{
            background: linear-gradient(135deg, rgba(0,229,255,0.15), rgba(123,97,255,0.15));
            border-color: rgba(0,229,255,0.4); color: var(--accent-cyan);
            box-shadow: 0 0 20px rgba(0,229,255,0.1), inset 0 1px 0 rgba(255,255,255,0.05);
        }}
        .trend-btn {{
            padding: 6px 16px; border: 1px solid var(--border-subtle);
            background: var(--bg-elevated); color: var(--text-muted);
            border-radius: 10px; cursor: pointer; font-size: 0.8em;
            font-family: inherit; font-weight: 500;
            transition: all 0.25s cubic-bezier(0.4, 0, 0.2, 1);
        }}
        .trend-btn:hover {{
            border-color: rgba(255,46,125,0.3); color: var(--text-primary);
        }}
        .trend-btn.active {{
            background: linear-gradient(135deg, rgba(255,46,125,0.15), rgba(123,97,255,0.15));
            border-color: rgba(255,46,125,0.4); color: var(--accent-pink);
            box-shadow: 0 0 16px rgba(255,46,125,0.08);
        }}
        .date-picker {{
            padding: 9px 16px; border: 1px solid var(--border-subtle);
            background: var(--bg-elevated); color: var(--text-secondary);
            border-radius: 12px; font-size: 0.88em;
            transition: all 0.25s;
        }}
        .date-picker:hover {{ border-color: rgba(0,229,255,0.25); }}
        .date-picker input[type="date"] {{
            background: transparent; color: var(--text-secondary); border: none;
            font-family: inherit; font-size: 0.88em; cursor: pointer; outline: none;
        }}
        .date-picker input[type="date"]::-webkit-calendar-picker-indicator {{
            filter: invert(0.6); cursor: pointer;
        }}

        /* 每日摘要 */
        .daily-report {{
            position: relative;
            background: var(--gradient-card);
            border-radius: var(--radius-lg); padding: 28px 32px;
            border: 1px solid var(--border-medium);
            margin-bottom: 24px;
            overflow: hidden;
        }}
        .daily-report::before {{
            content: ''; position: absolute; top: 0; left: 0; right: 0; height: 2px;
            background: var(--gradient-hero); opacity: 0.6;
        }}
        .daily-report-text {{
            font-size: 1.12em; line-height: 1.9; color: var(--text-primary);
            font-weight: 400;
        }}
        .daily-report-text .highlight {{
            color: var(--accent-cyan); font-weight: 600;
        }}
        .daily-report-text .cost-highlight {{
            color: var(--accent-pink); font-weight: 700;
        }}
        .daily-report-text .tool-highlight {{
            font-weight: 600; padding: 2px 10px; border-radius: 8px;
            background: rgba(0,229,255,0.1); border: 1px solid rgba(0,229,255,0.2);
        }}

        /* 核心指标卡片 */
        .summary-cards {{
            display: grid; grid-template-columns: 1.4fr 1fr 1fr 1fr;
            gap: 16px; margin-bottom: 24px;
        }}
        @media (max-width: 1100px) {{ .summary-cards {{ grid-template-columns: repeat(2, 1fr); }} }}
        @media (max-width: 600px) {{ .summary-cards {{ grid-template-columns: 1fr; }} }}

        .card {{
            position: relative;
            background: var(--gradient-card);
            border-radius: var(--radius-md); padding: 24px;
            border: 1px solid var(--border-subtle);
            transition: all 0.35s cubic-bezier(0.4, 0, 0.2, 1);
            overflow: hidden;
        }}
        .card.cost {{
            padding: 28px 27px 26px;
            border-radius: 22px;
        }}
        .card::before {{
            content: ''; position: absolute; top: 0; left: 0; right: 0; height: 3px;
            opacity: 0; transition: opacity 0.35s;
        }}
        .card:hover {{
            transform: translateY(-4px);
            border-color: var(--border-medium);
            box-shadow: var(--shadow-glow), 0 20px 40px rgba(0,0,0,0.3);
        }}
        .card:hover::before {{ opacity: 1; }}
        .card .label {{
            font-size: 0.8em; color: var(--text-secondary); margin-bottom: 10px;
            font-weight: 500; letter-spacing: 0.5px; text-transform: uppercase;
        }}
        .card .value {{
            font-size: 2em; font-weight: 700; letter-spacing: -0.5px;
            font-family: -apple-system, BlinkMacSystemFont, 'Segoe UI', Roboto, 'Helvetica Neue', Arial, 'PingFang SC', 'Microsoft YaHei', sans-serif;
        }}
        .card .sub {{
            font-size: 0.78em; color: var(--text-muted); margin-top: 6px;
            font-weight: 400;
        }}
        .card.cost::before {{ background: var(--accent-pink); }}
        .card.cost .value {{ color: var(--accent-pink); }}
        .card.lines::before {{ background: var(--accent-cyan); }}
        .card.lines .value {{ color: var(--accent-cyan); }}
        .card.books::before {{ background: var(--accent-violet); }}
        .card.books .value {{ color: var(--accent-violet); }}
        .card.events::before {{ background: var(--accent-amber); }}
        .card.events .value {{ color: var(--accent-amber); }}

        /* 图表区域 */
        .charts-row {{
            display: grid; grid-template-columns: 1fr 1fr;
            gap: 16px; margin-bottom: 24px;
        }}
        @media (max-width: 900px) {{ .charts-row {{ grid-template-columns: 1fr; }} }}

        .chart-box {{
            background: var(--gradient-card);
            border-radius: var(--radius-md); padding: 20px;
            border: 1px solid var(--border-subtle);
            transition: all 0.3s;
        }}
        .chart-box:hover {{
            border-color: var(--border-medium);
            box-shadow: 0 8px 32px rgba(0,0,0,0.2);
        }}
        .chart-box h3 {{
            margin-bottom: 14px; color: var(--text-secondary);
            font-size: 0.92em; font-weight: 600; letter-spacing: 0.5px;
            text-transform: uppercase;
        }}

        /* 各 Agent 用量明细表 */
        .agent-detail {{
            background: var(--gradient-card);
            border-radius: var(--radius-md); padding: 20px 24px;
            border: 1px solid var(--border-subtle); margin-bottom: 24px;
        }}
        .agent-detail h3 {{
            margin-bottom: 14px; color: var(--text-secondary);
            font-size: 0.92em; font-weight: 600; letter-spacing: 0.5px;
            text-transform: uppercase;
        }}
        .table-wrap {{ overflow-x: auto; -webkit-overflow-scrolling: touch; }}
        .agent-table {{
            width: 100%; border-collapse: collapse; font-size: 0.88em; min-width: 760px;
        }}
        .agent-table th, .agent-table td {{
            padding: 10px 12px; text-align: left;
            border-bottom: 1px solid var(--border-subtle);
        }}
        .agent-table th {{
            color: var(--text-muted); font-weight: 600; font-size: 0.8em;
            text-transform: uppercase; letter-spacing: 0.4px;
            background: var(--overlay-faint); white-space: nowrap;
        }}
        .agent-table td.num, .agent-table th.num {{ text-align: right; font-variant-numeric: tabular-nums; white-space: nowrap; }}
        .agent-table td.cost {{ color: var(--accent-pink); font-weight: 600; }}
        .agent-table tbody tr:hover {{ background: var(--overlay-faint); }}
        .agent-table .agent-name {{ font-weight: 600; color: var(--text-primary); white-space: nowrap; }}
        .agent-table .dot {{
            display: inline-block; width: 9px; height: 9px; border-radius: 50%;
            margin-right: 8px; vertical-align: middle;
        }}
        .agent-table tr.total-row {{
            font-weight: 700; border-top: 2px solid var(--border-medium);
            background: var(--overlay-soft);
        }}
        .agent-table tr.total-row td {{ border-bottom: none; }}
        .agent-table td.empty {{ text-align: center; color: var(--text-muted); padding: 24px; }}
        @media (max-width: 600px) {{ .agent-detail {{ padding: 16px; }} .agent-table {{ font-size: 0.8em; }} }}

        /* 趣味换算 */
        .fun-metrics {{
            background: var(--gradient-card);
            border-radius: var(--radius-md); padding: 24px;
            border: 1px solid var(--border-subtle); margin-bottom: 24px;
        }}
        .fun-metrics h3 {{
            color: var(--text-secondary); margin-bottom: 16px;
            font-size: 0.92em; font-weight: 600; letter-spacing: 0.5px;
            text-transform: uppercase;
        }}
        .fun-grid {{
            display: grid; grid-template-columns: repeat(auto-fit, minmax(280px, 1fr)); gap: 12px;
        }}
        .fun-item {{
            background: var(--overlay-faint); border-radius: var(--radius-sm);
            padding: 18px; border: 1px solid var(--border-subtle);
            transition: all 0.3s;
        }}
        .fun-item:hover {{
            border-color: var(--border-medium);
            background: var(--overlay-soft);
        }}
        .fun-item .fun-label {{
            color: var(--text-muted); font-size: 0.8em; font-weight: 500;
            letter-spacing: 0.5px; text-transform: uppercase;
        }}
        .fun-item .fun-value {{
            font-size: 1.3em; font-weight: 700; color: var(--accent-cyan);
            margin-top: 6px; font-family: -apple-system, BlinkMacSystemFont, 'Segoe UI', Roboto, 'Helvetica Neue', Arial, 'PingFang SC', 'Microsoft YaHei', sans-serif;
        }}
        .fun-item .fun-desc {{
            color: var(--text-muted); font-size: 0.76em; margin-top: 4px;
        }}

        /* Agent 注册表 */
        .tools-section {{
            background: var(--gradient-card);
            border-radius: var(--radius-md); padding: 24px;
            border: 1px solid var(--border-subtle); margin-bottom: 24px;
        }}
        .tools-section h3 {{
            color: var(--text-secondary); margin-bottom: 16px;
            font-size: 0.92em; font-weight: 600; letter-spacing: 0.5px;
            text-transform: uppercase;
        }}
        .tools-section > p {{
            color: var(--text-muted); font-size: 0.82em; margin-bottom: 14px;
        }}
        .tools-grid {{
            display: grid; grid-template-columns: repeat(auto-fit, minmax(260px, 1fr)); gap: 10px;
        }}
        .tool-card {{
            background: var(--overlay-faint); border-radius: var(--radius-sm);
            padding: 14px; display: flex; align-items: center; gap: 12px;
            border: 1px solid var(--border-subtle);
            transition: all 0.25s;
        }}
        .tool-card:hover {{
            border-color: var(--border-medium);
            background: var(--overlay-soft);
            transform: translateX(3px);
        }}
        .tool-card.enabled {{ border-left: 3px solid var(--accent-cyan); }}
        .tool-card.disabled {{ border-left: 3px solid #3A3F4D; opacity: 0.55; }}
        .tool-card .tool-dot {{
            width: 10px; height: 10px; border-radius: 50%; flex-shrink: 0;
            box-shadow: 0 0 8px currentColor;
        }}
        .tool-card .tool-name {{ font-weight: 600; font-size: 0.9em; color: var(--text-primary); }}
        .tool-card .tool-desc {{ font-size: 0.78em; color: var(--text-muted); }}
        .tool-card .tool-status {{
            margin-left: auto; font-size: 0.75em; padding: 3px 10px; border-radius: 100px;
            font-weight: 500;
        }}
        .tool-card .tool-status.on {{
            background: rgba(0,229,255,0.1); color: var(--accent-cyan);
            border: 1px solid rgba(0,229,255,0.2);
        }}
        .tool-card .tool-status.off {{
            background: var(--overlay-faint); color: var(--text-muted);
            border: 1px solid var(--border-subtle);
        }}

        /* 底部 */
        .footer {{
            text-align: center; padding: 32px 24px;
            color: var(--text-muted); font-size: 0.82em;
            border-top: 1px solid var(--border-subtle); margin-top: 8px;
        }}

        /* 入场动画 */
        @keyframes fadeUp {{
            from {{ opacity: 0; transform: translateY(24px); }}
            to {{ opacity: 1; transform: translateY(0); }}
        }}
        .summary-cards .card {{
            animation: fadeUp 0.6s cubic-bezier(0.4, 0, 0.2, 1) both;
        }}
        .summary-cards .card:nth-child(1) {{ animation-delay: 0.05s; }}
        .summary-cards .card:nth-child(2) {{ animation-delay: 0.12s; }}
        .summary-cards .card:nth-child(3) {{ animation-delay: 0.19s; }}
        .summary-cards .card:nth-child(4) {{ animation-delay: 0.26s; }}
    </style>
</head>
<body>
    <button id="themeToggle" onclick="toggleTheme()" title="切换浅色/深色" style="position:fixed;top:16px;right:235px;z-index:100;padding:7px 12px;border:1px solid var(--border-medium);border-radius:8px;background:var(--bg-elevated);color:var(--text-secondary);font-size:15px;cursor:pointer;backdrop-filter:blur(6px);">☀</button>
    <a href="/settings" style="position:fixed;top:16px;right:135px;color:var(--text-secondary);text-decoration:none;font-size:13px;z-index:100;padding:7px 14px;border:1px solid var(--border-medium);border-radius:8px;background:var(--bg-elevated);backdrop-filter:blur(6px);">⚙ 设置</a>
    <a href="/pricing" style="position:fixed;top:16px;right:20px;color:var(--text-secondary);text-decoration:none;font-size:13px;z-index:100;padding:7px 14px;border:1px solid var(--border-medium);border-radius:8px;background:var(--bg-elevated);backdrop-filter:blur(6px);">⚙ 模型定价</a>
    <div class="hero-glow"></div>
    <div class="app">
        <div class="header">
            <div class="header-badge">LIVE DASHBOARD</div>
            <h1>VibeStats</h1>
            <p class="tagline">让每一次 Token 燃烧都有迹可循</p>
            <p class="date-label" id="dateLabel">昨日数据</p>
        </div>

        <div class="container">
            <div class="controls">
                <button class="time-btn active" onclick="setRange('yesterday')">昨日</button>
                <button class="time-btn" onclick="setRange('last_week')">上周</button>
                <button class="time-btn" onclick="setRange('last_month')">上月</button>
                <button class="time-btn" onclick="setRange('all_time')">迄今</button>
                <div class="date-picker">
                    <label style="color:var(--text-muted)">跳转到: </label>
                    <input type="date" id="dateInput" value="{yesterday_str}" onchange="jumpToDate(this.value)">
                </div>
            </div>

            <div class="daily-report" id="dailyReport">
                <div class="daily-report-text" id="dailyReportText">加载中...</div>
            </div>

            <div class="summary-cards">
                <div class="card cost">
                    <div class="label">花了多少钱</div>
                    <div class="value" id="totalCost">¥0.00</div>
                    <div class="sub">按各工具实际模型定价</div>
                </div>
                <div class="card lines">
                    <div class="label">敲了多少行代码</div>
                    <div class="value" id="totalLines">0</div>
                    <div class="sub">约 15 Token/行</div>
                </div>
                <div class="card books">
                    <div class="label">够写几本书了</div>
                    <div class="value" id="totalBooks">0</div>
                    <div class="sub">按 30,000 行/本书估算</div>
                </div>
                <div class="card events">
                    <div class="label">调了几次接口</div>
                    <div class="value" id="totalEvents">0</div>
                    <div class="sub">原始事件数量</div>
                </div>
            </div>

            <div class="agent-detail">
                <h3>各 Agent 用量明细</h3>
                <div class="table-wrap">
                    <table class="agent-table" id="agentTable">
                        <thead>
                            <tr>
                                <th>Agent</th>
                                <th class="num">输入 Token</th>
                                <th class="num">输出 Token</th>
                                <th class="num">缓存命中</th>
                                <th class="num">费用</th>
                                <th class="num">代码行数</th>
                                <th class="num">调用次数</th>
                                <th class="num">费用占比</th>
                            </tr>
                        </thead>
                        <tbody id="agentTableBody"></tbody>
                        <tfoot id="agentTableFoot"></tfoot>
                    </table>
                </div>
            </div>

            <div class="charts-row">
                <div class="chart-box">
                    <h3>各 Agent 消耗对比</h3>
                    <div id="barChart" style="width:100%;height:380px;"></div>
                </div>
                <div class="chart-box">
                    <h3>各 Agent 花费占比</h3>
                    <div id="pieChart" style="width:100%;height:380px;"></div>
                </div>
            </div>

            <div class="charts-row">
                <div class="chart-box" style="grid-column: 1 / -1;">
                    <div style="display:flex;align-items:center;justify-content:space-between;margin-bottom:14px;">
                        <h3 style="margin:0">消耗趋势</h3>
                        <div style="display:flex;gap:8px;">
                            <button class="trend-btn active" onclick="setTrendRange('1d')">近一天</button>
                            <button class="trend-btn" onclick="setTrendRange('1w')">近一周</button>
                            <button class="trend-btn" onclick="setTrendRange('1m')">近一月</button>
                        </div>
                    </div>
                    <div id="trendChart" style="width:100%;height:360px;"></div>
                </div>
            </div>

            <div class="charts-row">
                <div class="chart-box">
                    <h3>Token 分布（输入 / 输出 / 缓存命中）</h3>
                    <div id="tokenDistChart" style="width:100%;height:380px;"></div>
                    <div style="color:var(--text-muted);font-size:0.72em;margin-top:6px;text-align:center;">
                        * Cursor / Trae 的缓存命中为 0 是正常的：这些工具的本地日志不记录缓存数据
                    </div>
                </div>
                <div class="chart-box">
                    <h3>缓存命中率（不含 Cursor/Trae，本地无缓存日志）</h3>
                    <div id="cacheRateChart" style="width:100%;height:380px;"></div>
                    <div style="color:var(--text-muted);font-size:0.72em;margin-top:6px;text-align:center;">
                        * 仅 Claude Code / DeepSeek GUI / OpenCode / ZCode 支持缓存命中统计
                    </div>
                </div>
            </div>

            <div class="fun-metrics">
                <h3>趣味数据换算</h3>
                <div class="fun-grid" id="funMetrics"></div>
            </div>

            <div class="tools-section">
                <h3>内置 Agent 工具注册表</h3>
                <p>在 config.toml 中启用/禁用工具</p>
                <div class="tools-grid" id="toolsGrid"></div>
            </div>
        </div>

        <div class="footer">VibeStats v0.2 · 数据每日自动更新</div>
    </div>

    <script>
        const rawData = {stats_json};
        const toolColors = {{ {color_map_js} }};

        // 主题切换：浅色/深色，localStorage 持久化；ECharts 颜色走 CSS 变量，切换时重渲染
        function cssVar(name) {{ return getComputedStyle(document.documentElement).getPropertyValue(name).trim(); }}
        function currentTheme() {{ return document.documentElement.dataset.theme || 'light'; }}
        function applyThemeIcon() {{ document.getElementById('themeToggle').textContent = currentTheme() === 'dark' ? '🌙' : '☀'; }}
        function toggleTheme() {{
            var t = currentTheme() === 'dark' ? 'light' : 'dark';
            document.documentElement.dataset.theme = t;
            localStorage.setItem('vibestats-theme', t);
            applyThemeIcon();
            renderBarChart(); renderPieChart(); renderFunMetrics();
            loadTrendData(currentTrendRange);
            loadCacheStats();
        }}
        applyThemeIcon();

        document.addEventListener('DOMContentLoaded', () => {{
            updateSummary();
            renderBarChart();
            renderPieChart();
            loadTrendData('1d');
            loadCacheStats();
            renderFunMetrics();
            renderDailyReport();
            renderAgentDetail();
            loadBuiltinTools();
            loadAvailableDates();
            // 自动轮询：间隔读 localStorage（默认 60s，0=关闭）；设置页改动经 storage 事件即时重建
            var pollInterval = parseInt(localStorage.getItem('vibestats-poll-interval')) || 60;
            var pollTimer = null;
            function startPolling() {{
                if (pollTimer) clearInterval(pollTimer);
                if (pollInterval > 0) pollTimer = setInterval(() => {{ if (currentStart) fetchData(currentStart, currentEnd); }}, pollInterval * 1000);
            }}
            startPolling();
            // 跨标签同步：设置页改主题/轮询间隔时，本页即时响应
            window.addEventListener('storage', function(e) {{
                if (e.key === 'vibestats-poll-interval') {{
                    pollInterval = parseInt(e.newValue) || 60; startPolling();
                }} else if (e.key === 'vibestats-theme') {{
                    document.documentElement.dataset.theme = e.newValue || 'light';
                    applyThemeIcon(); renderBarChart(); renderPieChart(); renderFunMetrics();
                    loadTrendData(currentTrendRange); loadCacheStats();
                }}
            }});
        }});

        function updateSummary() {{
            const t = rawData.totals;
            document.getElementById('totalCost').textContent = '¥' + t.total_estimated_cost.toFixed(2);
            document.getElementById('totalLines').textContent = t.total_code_lines.toLocaleString();
            document.getElementById('totalBooks').textContent = Math.max(1, Math.floor(t.total_code_lines / 30000)).toLocaleString() + ' 本';
            document.getElementById('totalEvents').textContent = t.total_events.toLocaleString();
        }}

        function formatTokens(n) {{
            if (n >= 1_000_000) return (n / 1_000_000).toFixed(2) + 'M';
            if (n >= 1_000) return (n / 1_000).toFixed(1) + 'K';
            return n.toString();
        }}

        function renderDailyReport() {{
            const t = rawData.totals;
            const totalTokens = t.total_input_tokens + t.total_output_tokens + t.total_cache_read_tokens;
            const codeLines = t.total_code_lines;
            const books = Math.max(1, Math.floor(codeLines / 30000));

            // 找出花费最多的工具（按实际花费，不是按 token 数）
            let topTool = {{ name: '无', cost: 0 }};
            rawData.by_tool.forEach(tool => {{
                const toolCost = tool.daily_data.reduce((s, d) => s + d.estimated_cost, 0);
                if (toolCost > topTool.cost) {{
                    topTool = {{ name: tool.tool_name, cost: toolCost }};
                }}
            }});

            const toolDisplayName = {{
                'claude_code': 'Claude Code',
                'deepseek_gui': 'DeepSeek GUI',
                'cursor': 'Cursor',
                'codex': 'Codex',
                'copilot_jb': 'Copilot JB',
                'trae_cn': 'Trae',
                'lingma': 'Lingma',
                'opencoder': 'OpenCode',
                'windsurf': 'Windsurf',
                'aider': 'Aider',
                'cline': 'Cline',
                'roo_code': 'Roo Code',
                'continue_dev': 'Continue',
                'github_copilot': 'GitHub Copilot',
                'amazon_q': 'Amazon Q'
            }};

            const topName = toolDisplayName[topTool.name] || topTool.name;

            if (totalTokens === 0) {{
                document.getElementById('dailyReportText').innerHTML =
                    currentRangeLabel + '没有检测到 AI 编程工具的使用记录，开始 Vibe Coding 吧！';
                return;
            }}

            const html = `${{currentRangeLabel}}你一共消耗了 <span class="highlight">${{formatTokens(totalTokens)}}</span> Token，` +
                `相当于写了 <span class="highlight">${{codeLines.toLocaleString()}}</span> 行代码，` +
                `这些代码相当于写了 <span class="highlight">${{books}}</span> 本书，` +
                `你最喜欢用的是 <span class="tool-highlight" style="color:${{getToolColor(topTool.name)}}">${{topName}}</span>，` +
                `花费约 <span class="cost-highlight">¥${{t.total_estimated_cost.toFixed(2)}}</span>`;

            document.getElementById('dailyReportText').innerHTML = html;
        }}

        function getToolColor(name) {{ return toolColors[name] || '#94A3B8'; }}

        function renderBarChart() {{
            const chart = echarts.init(document.getElementById('barChart'));
            const tools = rawData.by_tool.map(t => t.tool_name);
            const costs = rawData.by_tool.map(t => t.daily_data.reduce((s, d) => s + d.estimated_cost, 0));
            const colors = rawData.by_tool.map(t => getToolColor(t.tool_name));

            chart.setOption({{
                tooltip: {{ trigger: 'axis', formatter: function(params) {{
                    let s = params[0].name + '<br/>';
                    params.forEach(p => s += p.marker + ' ¥' + p.value.toFixed(2));
                    return s;
                }}}},
                grid: {{ left: '3%', right: '4%', bottom: '15%', containLabel: true }},
                xAxis: {{
                    type: 'category',
                    data: tools,
                    axisLabel: {{ color: cssVar('--chart-text'), rotate: 35, fontSize: 11, interval: 0 }}
                }},
                yAxis: {{ type: 'value', axisLabel: {{ color: cssVar('--chart-text'), formatter: v => '¥' + v.toFixed(2) }} }},
                series: [{{
                    type: 'bar',
                    data: costs.map((v, i) => ({{ value: v, itemStyle: {{ color: colors[i] }} }})),
                    barWidth: '50%',
                    label: {{ show: true, position: 'top', color: cssVar('--chart-text'), fontSize: 10, formatter: p => '¥' + p.value.toFixed(2) }}
                }}]
            }});
            window.addEventListener('resize', () => chart.resize());
        }}

        function renderPieChart() {{
            const chart = echarts.init(document.getElementById('pieChart'));
            const pieData = rawData.by_tool
                .filter(t => t.daily_data.reduce((s, d) => s + d.estimated_cost, 0) > 0)
                .map(t => ({{
                    name: t.tool_name,
                    value: parseFloat(t.daily_data.reduce((s, d) => s + d.estimated_cost, 0).toFixed(4)),
                    itemStyle: {{ color: getToolColor(t.tool_name) }}
                }}));

            chart.setOption({{
                tooltip: {{ trigger: 'item', formatter: p => p.name + ': ¥' + p.value.toFixed(2) + ' (' + p.percent + '%)' }},
                series: [{{
                    type: 'pie', radius: ['35%', '70%'], avoidLabelOverlap: false,
                    itemStyle: {{ borderRadius: 10, borderColor: cssVar('--chart-border'), borderWidth: 2 }},
                    label: {{ show: true, color: cssVar('--chart-text'), formatter: '{{b}}\n{{d}}%' }},
                    emphasis: {{ label: {{ show: true, fontSize: 16, fontWeight: 'bold' }} }},
                    data: pieData
                }}]
            }});
            window.addEventListener('resize', () => chart.resize());
        }}

        // 各 Agent 用量明细表：按工具聚合 daily_data，列示输入/输出/缓存/费用/行数/调用次数/占比
        function renderAgentDetail() {{
            var tbody = document.getElementById('agentTableBody');
            var tfoot = document.getElementById('agentTableFoot');
            if (!tbody) return;
            var rows = rawData.by_tool.map(function(t) {{
                var d = t.daily_data;
                return {{
                    name: t.tool_name,
                    input: d.reduce(function(s, x) {{ return s + x.input_tokens; }}, 0),
                    output: d.reduce(function(s, x) {{ return s + x.output_tokens; }}, 0),
                    cache: d.reduce(function(s, x) {{ return s + x.cache_read_tokens; }}, 0),
                    cost: d.reduce(function(s, x) {{ return s + x.estimated_cost; }}, 0),
                    lines: d.reduce(function(s, x) {{ return s + x.code_lines_equivalent; }}, 0),
                    events: d.reduce(function(s, x) {{ return s + x.event_count; }}, 0),
                    color: getToolColor(t.tool_name)
                }};
            }}).sort(function(a, b) {{ return b.cost - a.cost; }});
            var totalCost = rows.reduce(function(s, r) {{ return s + r.cost; }}, 0);
            if (rows.length === 0) {{
                tbody.innerHTML = '<tr><td colspan="8" class="empty">暂无数据</td></tr>';
                tfoot.innerHTML = '';
                return;
            }}
            var fmtInt = function(v) {{ return v.toLocaleString(); }};
            var fmtCost = function(v) {{ return '¥' + v.toFixed(2); }};
            var html = '';
            rows.forEach(function(r) {{
                var pct = totalCost > 0 ? (r.cost / totalCost * 100).toFixed(1) : '0.0';
                html += '<tr>' +
                    '<td class="agent-name"><span class="dot" style="background:' + r.color + '"></span>' + r.name + '</td>' +
                    '<td class="num">' + fmtInt(r.input) + '</td>' +
                    '<td class="num">' + fmtInt(r.output) + '</td>' +
                    '<td class="num">' + fmtInt(r.cache) + '</td>' +
                    '<td class="num cost">' + fmtCost(r.cost) + '</td>' +
                    '<td class="num">' + fmtInt(r.lines) + '</td>' +
                    '<td class="num">' + fmtInt(r.events) + '</td>' +
                    '<td class="num">' + pct + '%</td>' +
                    '</tr>';
            }});
            tbody.innerHTML = html;
            var ti = rows.reduce(function(s, r) {{ return s + r.input; }}, 0);
            var to = rows.reduce(function(s, r) {{ return s + r.output; }}, 0);
            var tc = rows.reduce(function(s, r) {{ return s + r.cache; }}, 0);
            var tl = rows.reduce(function(s, r) {{ return s + r.lines; }}, 0);
            var te = rows.reduce(function(s, r) {{ return s + r.events; }}, 0);
            tfoot.innerHTML = '<tr class="total-row">' +
                '<td>合计</td>' +
                '<td class="num">' + fmtInt(ti) + '</td>' +
                '<td class="num">' + fmtInt(to) + '</td>' +
                '<td class="num">' + fmtInt(tc) + '</td>' +
                '<td class="num cost">' + fmtCost(totalCost) + '</td>' +
                '<td class="num">' + fmtInt(tl) + '</td>' +
                '<td class="num">' + fmtInt(te) + '</td>' +
                '<td class="num">100.0%</td>' +
                '</tr>';
        }}

        let trendChartInstance = null;
        let currentTrendRange = '1d';

        function setTrendRange(range) {{
            currentTrendRange = range;
            document.querySelectorAll('.trend-btn').forEach(b => b.classList.remove('active'));
            event.target.classList.add('active');
            loadTrendData(range);
        }}

        function loadTrendData(range) {{
            fetch('/api/trend?range=' + range)
                .then(r => r.json())
                .then(data => renderTrendChart(data))
                .catch(() => {{}});
        }}

        function renderTrendChart(data) {{
            if (!data) return;
            if (!trendChartInstance) {{
                trendChartInstance = echarts.init(document.getElementById('trendChart'));
                window.addEventListener('resize', () => trendChartInstance.resize());
            }}

            const buckets = data.buckets;
            const isHourly = data.granularity === 'hour';

            // 格式化 bucket 标签
            const labels = buckets.map(b => {{
                if (isHourly) {{
                    // "2026-06-09T13:00" -> "06/09 13:00"
                    const parts = b.split('T');
                    const d = parts[0].substring(5).replace('-', '/');
                    const t = parts[1] ? parts[1].substring(0, 5) : '';
                    return d + ' ' + t;
                }}
                // "2026-06-09" -> "06/09"
                return b.substring(5).replace('-', '/');
            }});

            const series = data.series.map(s => ({{
                name: s.tool_name, type: 'line', smooth: true, symbol: 'circle', symbolSize: 5,
                lineStyle: {{ color: getToolColor(s.tool_name), width: 2 }},
                itemStyle: {{ color: getToolColor(s.tool_name) }},
                areaStyle: {{ color: new echarts.graphic.LinearGradient(0, 0, 0, 1, [
                    {{ offset: 0, color: getToolColor(s.tool_name) + '30' }},
                    {{ offset: 1, color: getToolColor(s.tool_name) + '05' }}
                ])}},
                data: buckets.map(b => {{
                    const found = s.points.find(p => p.bucket === b);
                    return found ? parseFloat(found.estimated_cost.toFixed(4)) : 0;
                }})
            }}));

            trendChartInstance.setOption({{
                tooltip: {{ trigger: 'axis', formatter: function(params) {{
                    let s = params[0].axisValue + '<br/>';
                    params.forEach(p => {{
                        if (p.value > 0) s += p.marker + ' ' + p.seriesName + ': ¥' + p.value.toFixed(2) + '<br/>';
                    }});
                    return s;
                }}}},
                legend: {{ textStyle: {{ color: cssVar('--chart-text') }}, top: 0 }},
                grid: {{ left: '3%', right: '4%', bottom: '3%', top: 40, containLabel: true }},
                xAxis: {{
                    type: 'category', data: labels, boundaryGap: false,
                    axisLabel: {{ color: cssVar('--chart-text'), rotate: isHourly ? 30 : 0, fontSize: 11 }}
                }},
                yAxis: {{ type: 'value', axisLabel: {{ color: cssVar('--chart-text'), formatter: v => '¥' + v.toFixed(2) }} }},
                series: series
            }}, true);
        }}

        function renderFunMetrics() {{
            const t = rawData.totals;
            const totalTokens = t.total_input_tokens + t.total_output_tokens + t.total_cache_read_tokens;
            const codeLines = t.total_code_lines;

            // 平均值：昨日按每小时，上周/上月按每天，迄今按每天
            const avgLabel = currentDataRange === 'yesterday' ? '平均每小时消耗' : '平均每天消耗';
            const avgDivisor = currentDataRange === 'yesterday' ? 24
                : (currentDataRange === 'last_week' ? 7
                    : (currentDataRange === 'last_month' ? 30
                        : Math.max(1, rawData.dates ? rawData.dates.length : 1)));
            const avgTokens = Math.floor(totalTokens / avgDivisor);

            // 代码连起来围绕地球：假设每行代码显示宽度 15cm
            // 地球周长 = 40,075 km = 40,075,000 m = 267,166,667 行（按 0.15m/行）
            const earthLines = 267_166_667;
            const earthCircles = codeLines / earthLines;
            const earthDisplay = earthCircles >= 10.0
                ? earthCircles.toFixed(1) + ' 圈'
                : (earthCircles >= 1.0
                    ? earthCircles.toFixed(2) + ' 圈'
                    : earthCircles.toFixed(4) + ' 圈');

            // 英语单词：1 token ≈ 0.75 英语单词
            const words = Math.floor(totalTokens * 0.75);
            const wordsDisplay = words >= 1_000_000
                ? (words / 1_000_000).toFixed(2) + 'M'
                : words.toLocaleString();

            const metrics = [
                {{ label: '总 Token 消耗', value: formatTokens(totalTokens), desc: '输入 + 输出 + 缓存命中' }},
                {{ label: avgLabel, value: formatTokens(avgTokens), desc: '=' + avgTokens.toLocaleString() + ' Token' }},
                {{ label: '代码能绕地球', value: earthDisplay, desc: '按每行 15cm 显示宽度' }},
                {{ label: '英语单词当量', value: wordsDisplay + ' 词', desc: '≈ ' + (totalTokens * 0.75 / 1_000_000).toFixed(2) + 'M 词' }},
            ];
            document.getElementById('funMetrics').innerHTML = metrics.map(m => `
                <div class="fun-item">
                    <div class="fun-label">${{m.label}}</div>
                    <div class="fun-value">${{m.value}}</div>
                    <div style="color:var(--text-muted);font-size:0.78em;margin-top:2px">${{m.desc}}</div>
                </div>
            `).join('');
        }}

        function loadBuiltinTools() {{
            fetch('/api/builtin-tools').then(r => r.json()).then(tools => {{
                document.getElementById('toolsGrid').innerHTML = tools.map(t => `
                    <div class="tool-card ${{t.enabled ? 'enabled' : 'disabled'}}">
                        <div class="tool-dot" style="background:${{getToolColor(t.id)}};color:${{getToolColor(t.id)}}"></div>
                        <div>
                            <div class="tool-name">${{t.display_name}}</div>
                            <div class="tool-desc">${{t.description}}</div>
                        </div>
                        <span class="tool-status ${{t.enabled ? 'on' : 'off'}}">${{t.enabled ? '已启用' : '未启用'}}</span>
                    </div>
                `).join('');
            }}).catch(() => {{}});
        }}

        function loadAvailableDates() {{
            fetch('/api/dates').then(r => r.json()).then(dates => {{
                if (dates.length > 0) {{
                    document.getElementById('dateInput').value = dates[0];
                }}
            }}).catch(() => {{}});
        }}

        // ===== 缓存统计 =====
        let currentCacheStart = '';
        let currentCacheEnd = '';

        function loadCacheStats() {{
            const today = new Date();
            const y = new Date(today); y.setDate(y.getDate() - 1);
            const start = y.toISOString().split('T')[0];
            const end = start;
            currentCacheStart = start;
            currentCacheEnd = end;
            fetchCacheStats(start, end);
        }}

        function fetchCacheStats(start, end) {{
            currentCacheStart = start;
            currentCacheEnd = end;
            fetch('/api/cache-stats?start=' + start + '&end=' + end)
                .then(r => r.json())
                .then(data => renderCacheCharts(data))
                .catch(() => {{}});
        }}

        function renderCacheCharts(data) {{
            if (!data || !data.by_tool) return;

            // === Token 分布堆叠柱状图 ===
            const distChart = echarts.init(document.getElementById('tokenDistChart'));
            const tools = data.by_tool.map(t => t.tool_name);
            const toolDisplayNames = {{
                'claude_code': 'Claude Code', 'deepseek_gui': 'DeepSeek GUI',
                'cursor': 'Cursor', 'trae_cn': 'Trae', 'codex': 'Codex',
                'copilot_jb': 'Copilot JB', 'lingma': 'Lingma'
            }};

            distChart.setOption({{
                tooltip: {{ trigger: 'axis', axisPointer: {{ type: 'shadow' }},
                    formatter: function(params) {{
                        let s = params[0].axisValue + '<br/>';
                        let total = 0;
                        params.forEach(p => {{ total += p.value; s += p.marker + ' ' + p.seriesName + ': ' + formatTokens(p.value) + '<br/>'; }});
                        s += '<b>合计: ' + formatTokens(total) + '</b>';
                        return s;
                    }}
                }},
                legend: {{ textStyle: {{ color: cssVar('--chart-text') }}, top: 0 }},
                grid: {{ left: '3%', right: '4%', bottom: '15%', containLabel: true }},
                xAxis: {{
                    type: 'category',
                    data: tools.map(t => toolDisplayNames[t] || t),
                    axisLabel: {{ color: cssVar('--chart-text'), rotate: 30, fontSize: 11, interval: 0 }}
                }},
                yAxis: {{ type: 'value', axisLabel: {{ color: cssVar('--chart-text'), formatter: v => formatTokens(v) }} }},
                series: [
                    {{
                        name: '输入 Tokens', type: 'bar', stack: 'total',
                        data: data.by_tool.map(t => t.input_tokens),
                        itemStyle: {{ color: cssVar('--accent-cyan') }},
                        emphasis: {{ focus: 'series' }}
                    }},
                    {{
                        name: '输出 Tokens', type: 'bar', stack: 'total',
                        data: data.by_tool.map(t => t.output_tokens),
                        itemStyle: {{ color: cssVar('--accent-pink') }},
                        emphasis: {{ focus: 'series' }}
                    }},
                    {{
                        name: '缓存命中 Tokens', type: 'bar', stack: 'total',
                        data: data.by_tool.map(t => t.cache_read_tokens),
                        itemStyle: {{ color: cssVar('--accent-lime') }},
                        emphasis: {{ focus: 'series' }}
                    }}
                ]
            }});
            window.addEventListener('resize', () => distChart.resize());

            // === 缓存命中率仪表盘 ===
            const rateChart = echarts.init(document.getElementById('cacheRateChart'));
            const overallRate = data.totals.overall_cache_hit_rate;

            // 各工具的缓存命中率
            const rateData = data.by_tool
                .filter(t => (t.input_tokens + t.cache_read_tokens) > 0)
                .map(t => ({{
                    name: toolDisplayNames[t.tool_name] || t.tool_name,
                    value: parseFloat(t.cache_hit_rate.toFixed(1)),
                    color: getToolColor(t.tool_name)
                }}));

            rateChart.setOption({{
                tooltip: {{ formatter: function(p) {{ return p.name + ': ' + p.value + '%' + '<br/><span style=\"font-size:10px;color:#888\">(仅统计 Claude Code / DeepSeek GUI)</span>'; }} }},
                series: [
                    {{
                        name: '总体缓存命中率',
                        type: 'gauge',
                        center: ['50%', '55%'],
                        radius: '75%',
                        startAngle: 200, endAngle: -20,
                        min: 0, max: 100,
                        splitNumber: 10,
                        axisLine: {{
                            lineStyle: {{
                                width: 18,
                                color: [[0.3, '#FF2E7D'], [0.7, '#FFB800'], [1, '#7EE787']]
                            }}
                        }},
                        pointer: {{
                            itemStyle: {{ color: 'auto' }},
                            width: 4, length: '60%'
                        }},
                        axisTick: {{ distance: -18, length: 6, lineStyle: {{ color: cssVar('--gauge-tick'), width: 1 }} }},
                        splitLine: {{ distance: -18, length: 18, lineStyle: {{ color: cssVar('--gauge-tick'), width: 2 }} }},
                        axisLabel: {{ color: cssVar('--chart-text'), distance: 25, fontSize: 11 }},
                        detail: {{
                            valueAnimation: true,
                            formatter: function(v) {{ return v + '%'; }},
                            color: cssVar('--accent-cyan'),
                            fontSize: 28, fontWeight: 700,
                            offsetCenter: [0, '70%']
                        }},
                        title: {{
                            offsetCenter: [0, '85%'],
                            fontSize: 12,
                            color: cssVar('--chart-text'),
                        }},
                        data: [{{ value: overallRate, name: '命中率 (Claude Code + DeepSeek GUI)' }}]
                    }}
                ]
            }});
            window.addEventListener('resize', () => rateChart.resize());
        }}

        let currentRangeLabel = '昨日';
        let currentDataRange = 'yesterday';  // yesterday / last_week / last_month / all_time
        let currentStart = '{yesterday_str}', currentEnd = '{yesterday_str}';  // 轮询复用当前视图的日期范围

        function updateCardLabels(rangeLabel) {{
            currentRangeLabel = rangeLabel;
            const prefix = rangeLabel || '昨日';
            document.querySelector('.card.cost .label').textContent = prefix + '花费估算';
            document.querySelector('.card.lines .label').textContent = prefix + '代码行数当量';
            document.querySelector('.card.books .label').textContent = prefix + '写了多少本书';
            document.querySelector('.card.events .label').textContent = prefix + ' API 调用次数';
        }}

        function setRange(range) {{
            currentDataRange = range;
            document.querySelectorAll('.time-btn').forEach(b => b.classList.remove('active'));
            event.target.classList.add('active');

            const today = new Date();
            let start, end, label;

            if (range === 'yesterday') {{
                const y = new Date(today); y.setDate(y.getDate() - 1);
                start = end = y.toISOString().split('T')[0];
                label = '昨日 (' + start + ')';
                updateCardLabels('昨日');
            }} else if (range === 'last_week') {{
                const dow = today.getDay() || 7;
                const lastSunday = new Date(today); lastSunday.setDate(today.getDate() - dow);
                const lastMonday = new Date(lastSunday); lastMonday.setDate(lastSunday.getDate() - 6);
                start = lastMonday.toISOString().split('T')[0];
                end = lastSunday.toISOString().split('T')[0];
                label = '上周 (' + start + ' ~ ' + end + ')';
                updateCardLabels('上周');
            }} else if (range === 'all_time') {{
                // 从 /api/dates 获取最早日期
                start = '2020-01-01';
                end = today.toISOString().split('T')[0];
                label = '迄今 (全部数据)';
                updateCardLabels('累计');
                // 异步获取精确的最早日期
                fetch('/api/dates').then(r => r.json()).then(dates => {{
                    if (dates.length > 0) {{
                        const earliest = dates[dates.length - 1];
                        const latest = dates[0];
                        document.getElementById('dateLabel').textContent = '迄今 (' + earliest + ' ~ ' + latest + ')';
                        fetchData(earliest, latest);
                    }}
                }}).catch(() => {{}});
            }} else {{
                const firstDay = new Date(today.getFullYear(), today.getMonth() - 1, 1);
                const lastDay = new Date(today.getFullYear(), today.getMonth(), 0);
                start = firstDay.toISOString().split('T')[0];
                end = lastDay.toISOString().split('T')[0];
                label = '上月 (' + start + ' ~ ' + end + ')';
                updateCardLabels('上月');
            }}

            document.getElementById('dateLabel').textContent = label;
            document.getElementById('dateInput').value = end;

            if (range !== 'all_time') {{
                fetchData(start, end);
            }}
        }}

        function jumpToDate(date) {{
            if (!date) return;
            document.querySelectorAll('.time-btn').forEach(b => b.classList.remove('active'));
            document.getElementById('dateLabel').textContent = date + ' 数据';
            updateCardLabels(date.slice(5).replace('-', '/') + ' ');
            fetchData(date, date);
        }}

        function fetchData(start, end) {{
            currentStart = start; currentEnd = end;
            fetch(`/api/stats/range?start=${{start}}&end=${{end}}`)
                .then(r => r.json())
                .then(data => {{
                    rawData.by_tool = data.by_tool;
                    rawData.dates = data.dates;
                    rawData.totals = data.totals;
                    updateSummary();
                    renderBarChart();
                    renderPieChart();
                    renderFunMetrics();
                    renderDailyReport();
                    renderAgentDetail();
                }});
            loadTrendData(currentTrendRange);
            fetchCacheStats(start, end);
        }}
    </script>

    <!-- GSAP ANIMATION: START -->
    <script src="https://cdnjs.cloudflare.com/ajax/libs/gsap/3.12.2/gsap.min.js"></script>
    <script>
    (function() {{
        /* GSAP 动画 - 尊重 prefers-reduced-motion */
        if (window.matchMedia('(prefers-reduced-motion: reduce)').matches) return;

        /* ---- 1. Hero Glow 呼吸动画 ---- */
        gsap.to('.hero-glow', {{
            scale: 1.15,
            opacity: 0.7,
            duration: 4,
            ease: 'sine.inOut',
            yoyo: true,
            repeat: -1
        }});

        /* ---- 2. 入场动画 - 仅 Header ---- */
        gsap.set('.header', {{ opacity: 0, y: -30 }});
        gsap.set('.summary-cards .card', {{ opacity: 0, y: 30 }});

        /* ---- 3. 入场 Timeline ---- */
        var entranceTl = gsap.timeline({{
            defaults: {{ ease: 'power3.out', duration: 0.7 }}
        }});

        entranceTl
            .to('.header', {{ opacity: 1, y: 0 }}, 0.1)
            .to('.summary-cards .card', {{
                opacity: 1, y: 0,
                stagger: 0.1,
                duration: 0.6
            }}, 0.45);

        /* ---- 4. 数字滚动动画 ---- */
        function animateCountUp(el, targetText) {{
            /* 解析目标数值 */
            var prefix = '';
            var suffix = '';
            var numStr = targetText.replace(/[^0-9.]/g, '');
            var targetNum = parseFloat(numStr);

            if (isNaN(targetNum)) return;

            /* 提取前缀和后缀 */
            var match = targetText.match(/^([^0-9]*)([0-9.,]+)([^0-9]*)$/);
            if (match) {{
                prefix = match[1];
                suffix = match[3];
            }}

            var obj = {{ val: 0 }};
            gsap.to(obj, {{
                val: targetNum,
                duration: 1.2,
                ease: 'power2.out',
                delay: 0.6,
                onUpdate: function() {{
                    var current = obj.val;
                    var formatted;
                    if (targetText.indexOf(',') !== -1) {{
                        /* 带千分位格式 */
                        var intPart = Math.floor(current).toLocaleString();
                        var decMatch = numStr.match(/\\.(\d+)/);
                        if (decMatch) {{
                            var decLen = decMatch[1].length;
                            formatted = intPart + '.' + current.toFixed(decLen).split('.')[1];
                        }} else {{
                            formatted = intPart;
                        }}
                    }} else if (numStr.indexOf('.') !== -1) {{
                        var decLen = numStr.split('.')[1].length;
                        formatted = current.toFixed(decLen);
                    }} else {{
                        formatted = Math.floor(current).toLocaleString();
                    }}
                    el.textContent = prefix + formatted + suffix;
                }}
            }});
        }}

        /* 拦截 updateSummary 以触发数字动画 */
        var _origUpdateSummary = window.updateSummary;
        if (typeof _origUpdateSummary === 'function') {{
            window.updateSummary = function() {{
                _origUpdateSummary();
                /* 在数据填充后启动数字滚动 */
                var costEl = document.getElementById('totalCost');
                var linesEl = document.getElementById('totalLines');
                var booksEl = document.getElementById('totalBooks');
                var eventsEl = document.getElementById('totalEvents');
                if (costEl) animateCountUp(costEl, costEl.textContent);
                if (linesEl) animateCountUp(linesEl, linesEl.textContent);
                if (booksEl) animateCountUp(booksEl, booksEl.textContent);
                if (eventsEl) animateCountUp(eventsEl, eventsEl.textContent);
            }};
        }}

        /* ---- 6. Card hover 微交互 ---- */
        document.querySelectorAll('.summary-cards .card').forEach(function(card) {{
            var valueEl = card.querySelector('.value');
            card.addEventListener('mouseenter', function() {{
                gsap.to(valueEl, {{ scale: 1.12, duration: 0.25, ease: 'power2.out' }});
            }});
            card.addEventListener('mouseleave', function() {{
                gsap.to(valueEl, {{ scale: 1, duration: 0.25, ease: 'power2.out' }});
            }});
        }});
    }})();
    </script>
    <!-- GSAP ANIMATION: END -->

</body>
</html>"##, stats_json = stats_json, color_map_js = color_map_js, yesterday_str = yesterday_str);

    Ok(html)
}

/// 模型定价编辑页（独立 HTML，无 format! 插值，花括号按字面量处理）
fn render_pricing_html() -> String {
    r##"<!DOCTYPE html>
<html lang="zh-CN">
<head>
<meta charset="UTF-8">
<meta name="viewport" content="width=device-width, initial-scale=1.0">
<title>VibeStats · 模型定价</title>
<script>(function(){var t=localStorage.getItem('vibestats-theme')||'light';document.documentElement.dataset.theme=t;})();</script>
<style>
  :root { --bg:#F5F7FA; --card:#FFFFFF; --text:#0F172A; --muted:#64748B; --accent:#7C3AED; --border:rgba(15,23,42,0.12); --input-bg:#F8FAFC; --accent-hover:#6D28D9; --danger:#E11D48; --ok:#059669; }
  [data-theme="dark"] { --bg:#0B1120; --card:#1E293B; --text:#E2E8F0; --muted:#94A3B8; --accent:#7C3AED; --border:rgba(148,163,184,0.2); --input-bg:#0F172A; --accent-hover:#6D28D9; --danger:#F87171; --ok:#34D399; }
  * { box-sizing:border-box; }
  body { margin:0; font-family:-apple-system,BlinkMacSystemFont,"Segoe UI",Roboto,sans-serif; background:var(--bg); color:var(--text); min-height:100vh; padding:28px 16px; }
  .card { max-width:900px; margin:0 auto; background:var(--card); border:1px solid var(--border); border-radius:14px; padding:28px; }
  a.back { color:var(--muted); text-decoration:none; font-size:14px; }
  a.back:hover { color:var(--text); }
  h1 { font-size:24px; margin:14px 0 4px; }
  .tagline { color:var(--muted); margin:0 0 8px; font-size:14px; }
  table { width:100%; border-collapse:collapse; margin-top:18px; }
  th, td { padding:10px 8px; text-align:left; border-bottom:1px solid var(--border); font-size:14px; vertical-align:middle; }
  th { color:var(--muted); font-weight:600; font-size:12px; text-transform:uppercase; letter-spacing:0.5px; }
  td input { width:100%; background:var(--input-bg); border:1px solid var(--border); color:var(--text); border-radius:6px; padding:8px 10px; font-size:14px; }
  td input.name { font-family:ui-monospace,SFMono-Regular,Menlo,monospace; }
  td input:focus { outline:none; border-color:var(--accent); }
  .actions { display:flex; gap:12px; margin-top:22px; align-items:center; flex-wrap:wrap; }
  button { cursor:pointer; border:none; border-radius:8px; padding:10px 18px; font-size:14px; font-weight:600; }
  button.primary { background:var(--accent); color:#fff; }
  button.primary:hover { background:var(--accent-hover); }
  button.ghost { background:transparent; color:var(--text); border:1px solid var(--border); }
  button.danger { background:transparent; color:var(--danger); border:1px solid rgba(248,113,113,0.3); padding:6px 10px; font-size:12px; }
  #status { font-size:13px; color:var(--muted); margin-left:8px; }
  #status.ok { color:var(--ok); }
  #status.err { color:var(--danger); }
  .hint { color:var(--muted); font-size:12px; margin-top:14px; line-height:1.6; }
</style>
</head>
<body>
  <div class="card">
    <a class="back" href="/">← 返回 Dashboard</a>
    <a href="/settings" style="position:fixed;top:16px;right:20px;color:var(--muted);text-decoration:none;font-size:13px;z-index:100;padding:7px 14px;border:1px solid var(--border);border-radius:8px;background:var(--card);">⚙ 设置</a>
    <button id="themeToggle" onclick="toggleTheme()" title="切换浅色/深色" style="position:fixed;top:16px;right:90px;z-index:100;padding:7px 12px;border:1px solid var(--border);border-radius:8px;background:var(--card);color:var(--muted);font-size:15px;cursor:pointer;">☀</button>
    <h1>模型定价</h1>
    <p class="tagline" id="tagline">单位：人民币（¥）/ 百万 Token。保存后立即按新价重算全部历史费用。</p>
    <table>
      <thead>
        <tr>
          <th style="width:36%">模型名</th>
          <th>输入</th>
          <th>输出</th>
          <th>缓存读</th>
          <th style="width:72px"></th>
        </tr>
      </thead>
      <tbody id="rows"></tbody>
    </table>
    <div class="actions">
      <button class="ghost" onclick="addRow('',0,0,0,true)">+ 添加模型</button>
      <button class="primary" onclick="save()">保存并重算</button>
      <span id="status"></span>
    </div>
    <p class="hint">说明：列出的模型来自历史记录与已保存的覆盖项；名称保存时会自动转小写。删除某行并保存后，该模型将回退到内置硬编码定价。<br>「default」为未识别模型（日志无模型名）的兜底价；日志带原始计费的工具（如 DeepSeek GUI）按原始计费显示，改价不重算其历史费用。</p>
  </div>

<script>
var RATE = 7.2; // 美元→人民币汇率，须与 models.rs::DEFAULT_USD_TO_RMB 一致
function cssVar(n){ return getComputedStyle(document.documentElement).getPropertyValue(n).trim(); }
function currentTheme(){ return document.documentElement.dataset.theme || 'light'; }
function applyThemeIcon(){ document.getElementById('themeToggle').textContent = currentTheme()==='dark' ? '🌙' : '☀'; }
function toggleTheme(){ var t = currentTheme()==='dark' ? 'light' : 'dark'; document.documentElement.dataset.theme=t; localStorage.setItem('vibestats-theme',t); applyThemeIcon(); }
applyThemeIcon();
function esc(s){ return String(s).replace(/&/g,'&amp;').replace(/"/g,'&quot;').replace(/</g,'&lt;'); }
function num(n){ n = Number(n); if(!isFinite(n) || n===0){ return '0'; } return String(Math.round(n*1e6)/1e6); }
function setStatus(msg, cls){ var s=document.getElementById('status'); s.textContent=msg; s.className=cls||''; }

function row(name, input, output, cache, focusName){
  var tr = document.createElement('tr');
  tr.innerHTML =
      '<td><input class="name" type="text" value="'+esc(name)+'" placeholder="model-name"></td>'
    + '<td><input type="number" step="0.01" min="0" value="'+num(input)+'"></td>'
    + '<td><input type="number" step="0.01" min="0" value="'+num(output)+'"></td>'
    + '<td><input type="number" step="0.01" min="0" value="'+num(cache)+'"></td>'
    + '<td><button class="danger" onclick="this.closest(\'tr\').remove()">删除</button></td>';
  document.getElementById('rows').appendChild(tr);
  if(focusName){ tr.querySelector('input.name').focus(); }
}
function addRow(name,i,o,c,f){ row(name,i,o,c,f); }

function load(){
  setStatus('加载中...');
  fetch('/api/pricing').then(function(r){ return r.json(); }).then(function(res){
    RATE = res.rate || 7.2;
    var map = res.models || res;
    var tg = document.getElementById('tagline'); if(tg){ tg.textContent = '单位：人民币（¥）/ 百万 Token · 1 USD = '+RATE+' ¥。保存后立即按新价重算全部历史费用。'; }
    var tbody = document.getElementById('rows'); tbody.innerHTML='';
    var keys = Object.keys(map).sort();
    if(keys.length===0){ addRow('',0,0,0,false); }
    keys.forEach(function(k){ row(k, map[k].input*RATE, map[k].output*RATE, map[k].cache_read*RATE, false); });
    setStatus('');
  }).catch(function(e){ setStatus('加载失败: '+e, 'err'); });
}

function save(){
  var map = {};
  var ok = true;
  document.querySelectorAll('#rows tr').forEach(function(tr){
    var inputs = tr.querySelectorAll('input');
    var name = inputs[0].value.trim().toLowerCase();
    if(!name){ return; }
    var i = parseFloat(inputs[1].value);
    var o = parseFloat(inputs[2].value);
    var c = parseFloat(inputs[3].value);
    if(isNaN(i)||isNaN(o)||isNaN(c)||i<0||o<0||c<0){ ok=false; }
    map[name] = { input: (isNaN(i)?0:i)/RATE, output: (isNaN(o)?0:o)/RATE, cache_read: (isNaN(c)?0:c)/RATE };
  });
  if(!ok){ setStatus('存在无效数值，请检查输入', 'err'); return; }
  setStatus('保存并重算中...');
  fetch('/api/pricing', {
    method:'PUT',
    headers:{ 'Content-Type':'application/json' },
    body: JSON.stringify(map)
  }).then(function(r){
    if(!r.ok){ throw new Error('HTTP '+r.status); }
    return r.json();
  }).then(function(res){
    setStatus('已保存，重算 '+res.recomputed_dates+' 个日期 ✓', 'ok');
    setTimeout(function(){ setStatus(''); }, 6000);
  }).catch(function(e){ setStatus('保存失败: '+e.message, 'err'); });
}

load();
</script>
</body>
</html>"##.to_string()
}

/// 设置页（独立 HTML，无 format! 插值，花括号按字面量处理）
fn render_settings_html() -> String {
    r##"<!DOCTYPE html>
<html lang="zh-CN">
<head>
<meta charset="UTF-8">
<meta name="viewport" content="width=device-width, initial-scale=1.0">
<title>VibeStats · 设置</title>
<script>(function(){var t=localStorage.getItem('vibestats-theme')||'light';document.documentElement.dataset.theme=t;})();</script>
<style>
  :root { --bg:#F5F7FA; --card:#FFFFFF; --text:#0F172A; --muted:#64748B; --accent:#7C3AED; --border:rgba(15,23,42,0.12); --input-bg:#F8FAFC; --accent-hover:#6D28D9; --danger:#E11D48; --ok:#059669; }
  [data-theme="dark"] { --bg:#0B1120; --card:#1E293B; --text:#E2E8F0; --muted:#94A3B8; --accent:#7C3AED; --border:rgba(148,163,184,0.2); --input-bg:#0F172A; --accent-hover:#6D28D9; --danger:#F87171; --ok:#34D399; }
  * { box-sizing:border-box; }
  body { margin:0; font-family:-apple-system,BlinkMacSystemFont,"Segoe UI",Roboto,sans-serif; background:var(--bg); color:var(--text); min-height:100vh; padding:28px 16px; }
  .card { max-width:860px; margin:0 auto; background:var(--card); border:1px solid var(--border); border-radius:14px; padding:28px; }
  a.back { color:var(--muted); text-decoration:none; font-size:14px; }
  a.back:hover { color:var(--text); }
  h1 { font-size:24px; margin:14px 0 4px; }
  .tagline { color:var(--muted); margin:0 0 8px; font-size:14px; }
  section { margin-top:26px; padding-top:18px; border-top:1px solid var(--border); }
  section:first-of-type { border-top:none; padding-top:6px; }
  h2 { font-size:16px; margin:0 0 4px; }
  .sec-hint { color:var(--muted); font-size:12px; margin:0 0 14px; line-height:1.6; }
  .row { display:flex; align-items:center; gap:12px; margin:10px 0; flex-wrap:wrap; }
  .row > label { font-size:14px; min-width:120px; color:var(--text); }
  .row input[type=number], .row input[type=text] { background:var(--input-bg); border:1px solid var(--border); color:var(--text); border-radius:6px; padding:8px 10px; font-size:14px; }
  .row input[type=number] { width:120px; }
  .row .unit { color:var(--muted); font-size:13px; }
  .theme-pick { display:flex; gap:10px; }
  .theme-pick > label { display:flex; align-items:center; gap:6px; cursor:pointer; border:1px solid var(--border); border-radius:8px; padding:8px 14px; font-size:14px; background:var(--input-bg); min-width:auto; }
  .theme-pick input { accent-color:var(--accent); }
  table { width:100%; border-collapse:collapse; margin-top:8px; }
  th, td { padding:10px 8px; text-align:left; border-bottom:1px solid var(--border); font-size:14px; vertical-align:middle; }
  th { color:var(--muted); font-weight:600; font-size:12px; text-transform:uppercase; letter-spacing:0.5px; }
  td input[type=text] { width:100%; background:var(--input-bg); border:1px solid var(--border); color:var(--text); border-radius:6px; padding:7px 10px; font-size:13px; }
  td input:focus { outline:none; border-color:var(--accent); }
  td input[type=checkbox] { accent-color:var(--accent); width:18px; height:18px; cursor:pointer; }
  .tool-name { font-weight:600; }
  .tool-id { font-size:11px; color:var(--muted); font-weight:normal; }
  .actions { display:flex; gap:12px; margin-top:22px; align-items:center; flex-wrap:wrap; }
  button { cursor:pointer; border:none; border-radius:8px; padding:10px 18px; font-size:14px; font-weight:600; }
  button.primary { background:var(--accent); color:#fff; }
  button.primary:hover { background:var(--accent-hover); }
  #status { font-size:13px; color:var(--muted); margin-left:8px; }
  #status.ok { color:var(--ok); }
  #status.err { color:var(--danger); }
  .note { color:var(--muted); font-size:12px; margin-top:6px; line-height:1.6; }
</style>
</head>
<body>
  <div class="card">
    <a class="back" href="/">← 返回 Dashboard</a>
    <button id="themeToggle" onclick="toggleTheme()" title="切换浅色/深色" style="position:fixed;top:16px;right:20px;z-index:100;padding:7px 12px;border:1px solid var(--border);border-radius:8px;background:var(--card);color:var(--muted);font-size:15px;cursor:pointer;">☀</button>
    <h1>设置</h1>
    <p class="tagline">调整显示偏好与数据配置。</p>

    <section>
      <h2>显示偏好</h2>
      <p class="sec-hint">主题与轮询间隔保存在浏览器本地，即时生效于已打开的 Dashboard（无需点保存）。</p>
      <div class="row">
        <label>主题</label>
        <div class="theme-pick">
          <label><input type="radio" name="theme" value="light"> ☀ 浅色</label>
          <label><input type="radio" name="theme" value="dark"> 🌙 深色</label>
        </div>
      </div>
      <div class="row">
        <label>轮询间隔</label>
        <input id="pollInterval" type="number" min="10" step="1" style="width:100px"> <span class="unit">秒（0 或勾选下方即关闭）</span>
        <label style="min-width:auto"><input id="pollOff" type="checkbox"> 关闭轮询</label>
      </div>
    </section>

    <section>
      <h2>数据配置</h2>
      <p class="sec-hint">以下项保存到 config.toml。汇率改动后立即重算全部历史费用；调度时间与工具列表需重启服务后于下次调度生效。</p>
      <div class="row">
        <label>汇率</label>
        <input id="exchangeRate" type="number" min="0.01" step="0.01"> <span class="unit">1 USD = ? CNY</span>
      </div>
      <div class="row">
        <label>调度时间</label>
        <input id="scheduleTime" type="text" placeholder="HH:MM"> <span class="unit">每日统计时刻（24h）</span>
      </div>
      <div class="row" style="flex-direction:column;align-items:stretch">
        <label>启用工具</label>
        <table>
          <thead><tr><th style="width:48px">启用</th><th>工具</th><th>自定义日志路径（留空用默认）</th></tr></thead>
          <tbody id="toolRows"></tbody>
        </table>
      </div>
      <div class="actions">
        <button class="primary" onclick="save()">保存</button>
        <span id="status"></span>
      </div>
      <p class="note">说明：调度时间与工具列表改动后需重启 VibeStats 服务才会生效；汇率保存后立即重算历史费用。</p>
    </section>
  </div>

<script>
function currentTheme(){ return document.documentElement.dataset.theme || 'light'; }
function applyThemeIcon(){ document.getElementById('themeToggle').textContent = currentTheme()==='dark' ? '🌙' : '☀'; }
function applyTheme(t){
  document.documentElement.dataset.theme = t;
  localStorage.setItem('vibestats-theme', t);
  applyThemeIcon();
  document.querySelectorAll('input[name=theme]').forEach(function(r){ r.checked = (r.value===t); });
}
function toggleTheme(){ applyTheme(currentTheme()==='dark' ? 'light' : 'dark'); }
applyThemeIcon();

function esc(s){ return String(s).replace(/&/g,'&amp;').replace(/"/g,'&quot;').replace(/</g,'&lt;'); }
function setStatus(msg, cls){ var s=document.getElementById('status'); s.textContent=msg; s.className=cls||''; }

// 显示偏好：即时写 localStorage
function initDisplayPrefs(){
  var t = localStorage.getItem('vibestats-theme') || 'light';
  document.querySelectorAll('input[name=theme]').forEach(function(r){ r.checked = (r.value===t); });
  document.querySelectorAll('input[name=theme]').forEach(function(r){
    r.addEventListener('change', function(){ if(r.checked) applyTheme(r.value); });
  });
  var pi = parseInt(localStorage.getItem('vibestats-poll-interval'));
  if(isNaN(pi)){ pi = 60; }
  var off = document.getElementById('pollOff');
  var inp = document.getElementById('pollInterval');
  if(pi <= 0){ off.checked = true; inp.value = 60; inp.disabled = true; }
  else { off.checked = false; inp.value = pi; inp.disabled = false; }
  off.addEventListener('change', function(){
    if(off.checked){ localStorage.setItem('vibestats-poll-interval','0'); inp.disabled = true; }
    else { var v = parseInt(inp.value)||60; if(v<10){ v=10; inp.value=10; } localStorage.setItem('vibestats-poll-interval', String(v)); inp.disabled=false; }
  });
  inp.addEventListener('change', function(){
    var v = parseInt(inp.value); if(isNaN(v)||v<10){ v=10; inp.value=10; }
    if(!off.checked){ localStorage.setItem('vibestats-poll-interval', String(v)); }
  });
}

// 数据配置：GET 加载 / PUT 保存
function load(){
  setStatus('加载中...');
  fetch('/api/settings').then(function(r){ return r.json(); }).then(function(res){
    document.getElementById('exchangeRate').value = res.exchange_rate;
    document.getElementById('scheduleTime').value = res.schedule_time || '00:30';
    var tbody = document.getElementById('toolRows'); tbody.innerHTML='';
    var enabled = {}; (res.enabled_tools||[]).forEach(function(id){ enabled[id]=true; });
    var paths = res.custom_paths || {};
    (res.builtin_tools||[]).forEach(function(t){
      var tr = document.createElement('tr');
      tr.innerHTML = '<td><input type="checkbox" data-id="'+t.id+'"'+(enabled[t.id]?' checked':'')+'></td>'
        + '<td class="tool-name">'+esc(t.display_name)+'<div class="tool-id">'+esc(t.id)+'</div></td>'
        + '<td><input type="text" data-path="'+t.id+'" placeholder="'+esc(t.default_path||'默认路径')+'" value="'+esc(paths[t.id]||'')+'"></td>';
      tbody.appendChild(tr);
    });
    setStatus('');
  }).catch(function(e){ setStatus('加载失败: '+e, 'err'); });
}

function save(){
  var rate = parseFloat(document.getElementById('exchangeRate').value);
  if(isNaN(rate)||rate<=0){ setStatus('汇率必须为正数', 'err'); return; }
  var st = document.getElementById('scheduleTime').value.trim();
  if(!/^\d{1,2}:\d{2}$/.test(st)){ setStatus('调度时间格式应为 HH:MM', 'err'); return; }
  var enabled = []; var paths = {};
  document.querySelectorAll('#toolRows tr').forEach(function(tr){
    var cb = tr.querySelector('input[type=checkbox]');
    var inp = tr.querySelector('input[type=text]');
    var id = cb.getAttribute('data-id');
    if(cb.checked){ enabled.push(id); }
    var p = inp.value.trim();
    if(p){ paths[id]=p; }
  });
  setStatus('保存中...');
  fetch('/api/settings', {
    method:'PUT',
    headers:{ 'Content-Type':'application/json' },
    body: JSON.stringify({ exchange_rate: rate, schedule_time: st, enabled_tools: enabled, custom_paths: paths })
  }).then(function(r){
    if(!r.ok){ throw new Error('HTTP '+r.status); }
    return r.json();
  }).then(function(res){
    var msg = '已保存 ✓';
    if(res.recomputed_dates!=null){ msg = '已保存，重算 '+res.recomputed_dates+' 个日期 ✓'; }
    setStatus(msg, 'ok');
    setTimeout(function(){ setStatus(''); }, 6000);
  }).catch(function(e){ setStatus('保存失败: '+e.message, 'err'); });
}

initDisplayPrefs();
load();
</script>
</body>
</html>"##.to_string()
}
