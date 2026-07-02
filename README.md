# VibeStats

> 让每一次 Token 燃烧都有迹可循

本地轻量级 AI 编程工具 Token 消耗统计与趣味看板。支持 ZCode、Claude Code、Cursor、DeepSeek GUI、Trae、Codex 等主流 AI 编程工具，自动解析本地日志，按实际模型定价统计费用，生成赛博朋克风格的数据可视化 Dashboard。**每 30 秒实时刷新**，数据完全本地存储，零上传。

![Dashboard Preview](https://via.placeholder.com/800x450/0B0F1A/00E5FF?text=VibeStats+Dashboard)

[![License](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)
[![Rust](https://img.shields.io/badge/rust-1.75+-orange.svg)](https://www.rust-lang.org)

## 🚀 快速开始

### 下载即用（推荐）

从 [Releases](https://github.com/whitequeen306/VibeStats/releases) 下载最新版本：

- **Windows**: `VibeStats.exe`（双击即用，需 WebView2 运行时，Win10/11 一般已自带）
- **macOS / Linux**: 从源码构建（见下方）

**使用步骤**：
1. 双击运行 `VibeStats.exe`
2. 自动弹出原生窗口并加载 Dashboard（http://localhost:7890）
3. 查看你的 AI 编程消耗统计

首次运行会自动生成配置文件、创建桌面快捷方式并设置开机自启。关闭窗口后程序最小化到系统托盘，在后台继续运行（含 30 秒实时刷新）。

### 从源码构建

需要 Rust 1.75+。

```bash
git clone https://github.com/whitequeen306/VibeStats.git
cd vibestats
cargo build --release
```

编译产物在 `target/release/vibestats`。

## ✨ 功能特性

### 📊 数据统计
- **多工具支持**：ZCode、Claude Code、Cursor、DeepSeek GUI、Trae、Codex 等
- **精确计费**：按各工具实际使用的模型定价（Claude Sonnet、GLM-5.2、DeepSeek-V4-Pro 等），支持配置覆盖
- **30 秒实时刷新**：后台每 30 秒重新解析日志并重算统计，Dashboard 近实时反映用量（可配置，可关闭）
- **T+1 定时任务**：每日凌晨自动统计前一天数据并发送晨报通知，支持补偿机制
- **增量 + 快照混合解析**：增量式记录文件指针去重，快照式按 (工具,日期) 替换，避免重复累加

### 🎯 数据看板
- **实时 Dashboard**：赛博朋克风格可视化界面，顶部"● 实时刷新中 · 最后更新"脉冲徽标
- **各 Agent 用量明细**：按工具列出输入/输出/缓存/费用/代码行数/调用次数/费用占比
- **消耗趋势**：近一天/近一周/近一月折线图
- **工具对比**：各 Agent 花费对比柱状图 + 占比扇形图
- **Token 分布**：输入/输出/缓存命中堆叠柱状图
- **缓存命中率**：仪表盘显示总体命中率

### 🎮 趣味数据
- **代码行数当量**：按 15 Token/行估算
- **写了多少本书**：按 30,000 行/本书换算
- **代码绕地球**：按 15cm/行显示宽度，计算能绕地球几圈
- **英语单词当量**：1 Token ≈ 0.75 英语单词
- **平均消耗**：昨日按每小时，上周/上月按每天

### 🔔 系统通知
- 每日上午 8 点推送昨日消耗报告
- 跨平台支持（Windows Toast / macOS Notification Center / Linux notify-send）

### 🛠️ 系统托盘
- 关闭窗口最小化到托盘
- 托盘菜单：显示仪表盘 / 退出 VibeStats
- Token Ring 风格 Logo

## 📦 支持的工具

| 工具 | 类型 | Token 数据 | 费用计算 |
|------|------|-----------|---------|
| **ZCode** | CLI | ✅ 完整（含缓存） | ✅ 精确 |
| **Claude Code** | CLI | ✅ 完整 | ✅ 精确 |
| **DeepSeek GUI** | 桌面端 | ✅ 完整 | ✅ 精确（日志含 costUsd） |
| **OpenCode** | CLI | ✅ 完整（含缓存） | ✅ 精确 |
| **Cursor** | AI IDE | ⚠️ 估算（请求数） | ✅ 按模型定价 |
| **Trae CN** | AI IDE | ⚠️ 估算（日志解析） | ✅ 按模型定价 |
| **Codex** | CLI | ❌ 仅事件 | ⚠️ 默认定价 |
| **Copilot (JetBrains)** | 插件 | ❌ 仅事件 | ⚠️ 默认定价 |
| **Windsurf** | AI IDE | ❌ 仅事件 | ⚠️ 默认定价 |
| **Cline / Roo Code** | VS Code 扩展 | ❌ 仅事件 | ⚠️ 默认定价 |
| **GitHub Copilot** | VS Code 扩展 | ❌ 仅事件 | ⚠️ 默认定价 |
| **Continue** | VS Code 扩展 | ❌ 仅事件 | ⚠️ 默认定价 |
| **Amazon Q** | 插件 | ❌ 仅事件 | ⚠️ 默认定价 |
| **通义灵码** | 插件 | ❌ 仅事件 | ⚠️ 默认定价 |

> ✅ = 精确数据 | ⚠️ = 估算 | ❌ = 仅记录事件次数

## 📖 CLI 用法

```bash
# 默认：启动桌面窗口 + 后台调度器
./vibestats

# 仅启动 HTTP 服务器（无窗口，适合服务器）
./vibestats --serve

# 仅后台调度器（无窗口无 HTTP）
./vibestats --headless

# 立即执行一次统计
./vibestats --run-now

# 全量重建（清空数据库重新解析所有日志）
./vibestats --rebuild

# 仅解析日志并打印
./vibestats --parse-only
```

## ⚙️ 配置

配置文件位于：
- Windows: `%LOCALAPPDATA%\vibestats\config.toml`
- macOS: `~/Library/Application Support/vibestats/config.toml`
- Linux: `~/.local/share/vibestats/config.toml`

示例配置见 [config.example.toml](config.example.toml)。

```toml
# 启用的工具列表
enabled_tools = ["zcode", "claude_code", "cursor", "deepseek_gui", "trae_cn", "codex"]

# 每日统计时间（24 小时制，晨报通知触发点）
schedule_time = "00:30"

# 实时刷新间隔（秒）：后台每隔该时长重新解析日志并重算统计
# 默认 30，设为 0 则禁用实时刷新（仅靠每日定时任务）
refresh_interval_secs = 30

# HTTP 服务器端口
serve_port = 7890

# 美元→人民币汇率（1 USD = ? CNY），改动后自动重算全部历史费用
exchange_rate = 7.2

# 自定义日志路径（可选，覆盖默认路径）
[custom_paths]
cursor = "D:/MyLogs/Cursor/logs"
claude_code = "D:/AI/Claude/logs"

# 模型定价覆盖（可选，每百万 Token 美元价，覆盖内置硬编码定价）
[pricing_overrides."glm-5.2"]
input = 0.6
output = 2.2
cache_read = 0.11
```

## 🔒 数据隐私

**所有数据纯本地处理**：
- ✅ 日志文件从本地磁盘读取，不上传
- ✅ SQLite 数据库存放在本地数据目录
- ✅ 不连接任何远程 API
- ✅ 不开启任何网络请求

## 🏗️ 技术栈

- **后端**: Rust + actix-web + rusqlite
- **桌面窗口**: wry + tao（原生 WebView，不是浏览器）
- **前端**: ECharts + 原生 HTML/CSS
- **调度**: 自定义 T+1 定时任务 + 补偿机制
- **系统托盘**: tray-icon + Windows API

## 📝 更新日志

### v0.2.1 (2026-07-02)
- 🐛 修复 `daily_stats` 陈旧导致统计偏低 18%（快照式解析器刷新 raw_events 后未重算已有日期）
- ✨ 新增 **30 秒实时刷新**：后台 `refresh_loop` 每 30s 重新解析+重算，Dashboard 近实时反映用量
- 🎨 Dashboard 顶部新增"● 实时刷新中 · 最后更新 HH:MM:SS"脉冲徽标
- ⚙️ 新增配置项 `refresh_interval_secs`（默认 30，0=禁用）

### v0.2.0 (2026-07-01)
- ✨ 新增"各 Agent 用量明细"表格（输入/输出/缓存/费用/代码行数/调用次数/费用占比）
- 🐛 修复 7 项 Token 统计准确性问题（DeepSeek/GLM 定价、Codex 模型归属、缓存统计等）
- 🐛 修复定价覆盖表导致费用高估（GLM 1.4/4.4/0.26 → 0.6/2.2/0.11）
- ⚙️ 设置页支持汇率调整并即时重算历史费用

### v0.1.0 (2026-06-10)
- ✨ 初始版本发布
- 📊 支持 13 种主流 AI 编程工具
- 🎯 按实际模型定价计算费用
- 🎮 趣味数据换算（代码行数、绕地球、英语单词）
- 🔔 跨平台系统通知
- 🛠️ 系统托盘支持

## 🤝 贡献

欢迎提交 Issue 和 Pull Request！

## 📄 License

MIT License - 详见 [LICENSE](LICENSE) 文件
