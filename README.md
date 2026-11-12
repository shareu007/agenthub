# AgentHub

轻量、按需运行的 Coding Agent 会话索引，使用 Rust、SQLite 和 FTS5。支持 Claude Code、Codex、OpenCode、Pi、Google Antigravity CLI（AGY）的适配器，面向 Linux 和 macOS；没有常驻后台进程。

当前阶段是 Core + CLI。**适配器存在不代表所有版本均已通过真实恢复测试**，版本、样本和实际验证结果见 [兼容性验收](docs/compatibility.md)。AGY 尚无已核实的版本或真实样本。

## 安装与开发

需要 Rust 1.85+、C 编译器（用于内置 SQLite），SQL 检查另需 Python 3。

```sh
cargo build --locked --release --workspace
./target/release/agenthub --help
# 可选：安装到 Cargo bin 目录
cargo install --locked --path crates/cli
```

```sh
cargo test --locked --workspace
cargo clippy --locked --workspace --all-targets -- -D warnings
python3 scripts/verify_sql.py
python3 scripts/verify_fixtures.py
```

CI 配置覆盖 Linux/macOS、stable/1.85.0；本次实际运行的是 Linux 1.85.0。

## 使用

```sh
agenthub doctor
agenthub sync
agenthub list --agent codex
agenthub list --project /absolute/project --since 2026-11-01 --until 2026-12-01
agenthub search '渲染' --agent pi
agenthub show pi:SESSION_ID
agenthub show pi:SESSION_ID --offset 10 --limit 20
agenthub resume pi:SESSION_ID         # 只打印原生恢复命令
agenthub resume pi:SESSION_ID --exec  # 交接给原生程序
```

列表和搜索默认返回最多 50 项；用 `--limit` / `--offset` 分页。项目过滤采用路径精确匹配。时间过滤针对会话更新时间，`--since` 包含边界、`--until` 不包含边界；日期和界面时间均为 UTC。

`--color auto|always|never` 控制人类输出；默认仅终端着色，尊重 `NO_COLOR` 和 `TERM=dumb`。重定向不自动着色。完整 ID、路径和正文不截断，中文不会被切开。使用方式和界面示例见 [CLI 设计](docs/cli-design.md)。

### 脚本接口

```sh
agenthub list --json
agenthub search rendering --json
agenthub show codex:SESSION_ID --json > transcript.jsonl
agenthub show codex:SESSION_ID --raw > native-records.jsonl
agenthub resume codex:SESSION_ID --json
```

`list/search/doctor` 输出 JSON 数组，`sync` 输出以 Agent 为键的 JSON 对象；`show --json` 输出一条 session 头及逐条 message 包装对象，采用 JSON Lines。`show --raw` 仅输出原生记录 JSON Lines，不输出界面头；同时使用 `--json` 时以包装对象为准。JSON 模式即使 `--color always` 也不含 ANSI 装饰。

参数错误退出 2；运行/同步/发现失败退出 1；正常命令退出 0。`doctor` 没有安装原生 CLI 或来源目录不存在不算发现失败；它不检查登录或完整格式兼容性。`resume --exec` 在 Unix 下保留原生程序退出码与信号。

### 配置

默认读取 `$XDG_CONFIG_HOME/agenthub/config.toml`（未设置时为 `~/.config/agenthub/config.toml`），参见 [配置示例](agenthub.example.toml)。通过 `--config` / `AGENTHUB_CONFIG` 显式指定配置；通过 `--db` / `AGENTHUB_DB` 指定独立索引路径。默认索引为 `$XDG_DATA_HOME/agenthub/index.db`。

来源路径可设置为绝对路径或 `~/...`；相对路径按运行目录解析。不要将索引指向原 Agent 数据库。无需配置模型密钥即可索引和搜索；真实恢复需要安装对应原生 CLI，继续模型对话还需要该 CLI 自己的认证。

## 数据边界

- AgentHub 读取原始数据，SQLite 来源通过私有 DB+WAL 副本访问；不会要求原 Agent checkpoint。
- 索引含对话原文，属于敏感本地数据，新索引文件以 `0600` 创建。来源变化同步失败时保留上次成功索引。
- 增量粒度是来源：JSONL 文件变化会重读整文件，OpenCode 共用数据库变化会重读其中的会话。`sync --force` 可强制重建。
- 来源消失后保留缓存；用 `--include-missing` 列出，不能从缓存直接恢复原生会话。
- Claude 子代理附件不展开；Pi 保留完整记录树而非仅活动分支；AGY 附件/轨迹侧表未解码，未知编码保留 raw 并警告。
- `resume --exec` 后由原生 Agent 接管，原生程序可能修改自己的会话数据；只读承诺针对 AgentHub 的发现、读取和索引过程。

Archive 当前仅预留逻辑收藏接口，没有归档命令；GUI、跨设备同步、SSH 和跨 Agent 上下文恢复均不在当前阶段。

[架构](docs/architecture.md) · [格式调查](docs/session-formats.md) · [版本与真实恢复](docs/compatibility.md)
