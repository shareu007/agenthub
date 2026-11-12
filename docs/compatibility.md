# 固定版本、原生样本与恢复验收

验收日期：2026-11-12。机器可读矩阵：[agents.json](../compatibility/agents.json)。六个已核实版本固定如下：

| Agent | 固定版本 | 原生写出样本 | 原生恢复加载 | 模型续聊 |
| --- | --- | --- | --- | --- |
| AGY | 1.5.6 | 有：受控本地 Gemini SSE + 原生 CLI 写出 | 真实 `resume --exec` 终端显示唯一历史标记 | 未调用 |
| Claude Code | 2.1.322 | 无 | 未执行：未登录、无专用会话 | 未执行 |
| Codex | 0.167.0-alpha.4 | 无 | 未执行：无专用会话 | 未执行 |
| DeepSeek Harness (`dsh`) | 0.2.2-rc.1 | 有：受控本地 DeepSeek Messages SSE + 原生 CLI 写出 | 真实 `dsh --profile sdk` JSON-RPC 恢复校验 + `headless --session-id` 复核 | 未调用远端 |
| OpenCode | 1.19.5 | 有：本地原生 API + noReply | 真实 `resume --exec` 终端显示唯一历史标记 | 未调用 |
| Pi | 1.5.2 | 有：原生 SessionManager 写出 | 真实 `resume --exec` 终端 + RPC 身份/消息校验 | 未调用 |

Codex 是云环境自带预发布版，不能视为稳定版支持承诺。CLI `--version` 成功不是格式/恢复验收。真实恢复加载的定义是：原生程序读取原会话，终端显示仅存在于历史中的标记；Pi 与 DeepSeek Harness 另用各自原生 RPC（`pi --mode rpc` / `dsh --profile sdk`）核对会话 ID、工作目录和历史记录。这里只证明历史加载，不证明继续调用远端模型、执行工具或认证成功。证据见 [compatibility/evidence](../compatibility/evidence)。

## 样本来源与脱敏

样本不是私人对话的复制，而是在专用空目录中由固定版本的**真实原生写入器**产生的非敏感测试会话。测试文本和助手回复是受控输入；没有模型生成内容、凭据或私人项目。此方式保留真实格式并避免把未知字段里的私人数据带进仓库。

- `tests/fixtures/agy/1.5.6.sql`：从官方安装脚本 `https://antigravity.google/cli/install.sh` 安装 `agy 1.5.6`（SHA-512 校验通过）。在隔离 `HOME=/tmp/agenthub-agy-home` 与工作目录 `/tmp/agenthub-fixture-project` 下配置 `modelProvider="gemini"`、`GEMINI_API_KEY` 离线占位符及本地 `127.0.0.1` 受控 SSE 端点，执行 `agy -p` 真实写出 `conversation_summaries.db` 与 `conversations/<id>.db`，含用户提问（`step_type=14`）、助手思考与 `read_file` 工具调用（`step_type=15` 的 `20.3`/`20.7`）、工具执行结果（`step_type=132`）和离线助手回复（`step_type=15` 的 `20.8`/`20.1`）共 4 条记录。导出 `conversation_summaries` 表及会话库表定义与核心行；省略 `conversation_summaries.db` 内部 FTS5 影子表，并清空单次请求系统提示词侧表 `gen_metadata`/`executor_metadata` 的大体积行。
- `tests/fixtures/dsh/0.2.2-rc.1.jsonl.zstd`：从 npm 官方包 `@deepseek-ai/dsh@0.2.2-rc.1` 安装 `dsh` CLI。在隔离 `DSH_HOME` 与工作目录 `/tmp/agenthub-fixture-project` 下配置离线占位 `DEEPSEEK_API_KEY` 与本地 `127.0.0.1` 受控 Anthropic Messages SSE 端点，执行 `dsh --profile headless "Inspect AH_FIXTURE_20261112"` 真实写出 9 个串联 Zstandard frames 的 `session.v4.jsonl.zstd`，含 v4 header、`user/message`（用户提问与 `runtime-context` 快照）、`assistant/message`（`reasoning` + `glob` 工具调用 + 最终回复）、`tool/call`、`tool/result` 和 `session/title` 事件。
- `tests/fixtures/pi/1.5.2.jsonl`：从安装包 `dist/core/session-manager.js` 调用 `SessionManager.create('/tmp/agenthub-fixture-project', sessionDir)`、两次 `appendMessage` 写出；一次用户消息，一次明确标注为 offline 的助手消息。真实 Pi CLI 打开后追加了 `thinking_level_change`，样本共 4 条记录。UUID、记录 ID、parentId 和时间戳为原生生成；没有人工拼装 JSONL 格式。
- `tests/fixtures/opencode/1.19.5.sql`：真实 `opencode serve --hostname 127.0.0.1`，以 `/tmp/agenthub-fixture-project` 为 directory 创建 session；向 `/session/{id}/message` 提交 `noReply: true` 和唯一标记。导出原生 project/session/message/part/session_message 表定义和测试行，SQL 导入时临时关闭外键，结束后恢复，外键完整性检查通过。未包含认证、迁移历史或其他非必要表；该夹具用于 Adapter 读取，不作为可直接导入原生客户端的完整数据库快照。
- 四份文件均保留原生生成的测试身份与非敏感时间戳、版本和模型名称；路径仅包含固定测试目录及 OpenCode 内置 global 根 `/`。这是受控样本，不代表真实复杂工具调用、附件、分支或长对话已经验收。
- SHA-256 记在矩阵；`python3 scripts/verify_fixtures.py` 验证哈希与结构，Rust 测试验证实际解析和来源不变。
- 既有 `formats.rs` 中其他内联夹具均为合成数据，不能升级宣称为某固定真实版本的样本。

原生包安装来源为 npm 官方命名空间：`@anthropic-ai/claude-code@2.1.322`、`@deepseek-ai/dsh@0.2.2-rc.1`、`@earendil-works/pi-coding-agent@1.5.2`、`opencode-ai@1.19.5`（对应平台包 `opencode-linux-x64@1.19.5`）；AGY 来自 Google 官方安装脚本 `https://antigravity.google/cli/install.sh`（`1.5.6`）。

## 重跑真实恢复测试

原生检查独立于常规 CI，使用标准库脚本；缺失版本、程序、会话、登录或预期标记均以 blocked 和退出 2 报告，绝不算通过。脚本只保存状态与终端输出哈希，不保存对话正文或环境值。

准备专用原生会话，写入唯一的非敏感标记；让原生程序自己持久化。通过 AgentHub 配置指向该存储，运行 `sync` 后：

```sh
python3 scripts/verify_native.py \
  --agenthub /absolute/path/to/agenthub \
  --config /absolute/path/to/disposable-config.toml \
  --session pi:NATIVE_SESSION_ID \
  --expect AH_FIXTURE_20261112 \
  --output /tmp/pi-native-evidence.json
```

脚本使用真实 PTY 执行 `agenthub resume <id> --exec`，不发送 prompt；在发现历史标记后终止本次创建的进程组。超时、首次信任提示、登录界面等都会阻止通过，不自动代替用户接受原生许可提示。Pi 可增加 `--probe pi-rpc`，DeepSeek Harness 可使用 `--probe dsh-rpc`，由 AgentHub 生成的恢复计划启动真实原生 RPC 服务，核对原会话 ID、工作目录和标记，不调用远端模型。

原生程序应在 PATH 中，并与矩阵的精确版本相符；先设置其受支持的配置/数据目录变量指向隔离目录。不要使用生产会话：原生程序加载时可以更新自己的会话文件。终端探针适用于 Linux/macOS；本次只在 Linux 执行。

## AGY 与 DeepSeek Harness 验收结果与剩余边界

1. 已从 Google 官方安装脚本 `https://antigravity.google/cli/install.sh` 安装并核实 Linux `x86_64` 版本 `1.5.6`（`agy --version`）；已从 npm 安装并核实 `@deepseek-ai/dsh@0.2.2-rc.1`（`dsh --version`）。
2. 已通过各自原生 CLI + 本地受控 SSE 端点在隔离目录写出包含唯一标记、思考、工具调用与工具结果的真实会话（`tests/fixtures/agy/1.5.6.sql` 与 `tests/fixtures/dsh/0.2.2-rc.1.jsonl.zstd`），无私人工作会话或真实密钥写入夹具。
3. 已核实 AGY 的 SQLite + Protobuf wire 路径与 DeepSeek Harness 的串联 Zstandard frames（`.jsonl.zstd`）/多代际会话文件结构（`session[.vN].jsonl[.zstd]`），递增 `PARSER_VERSION=3` 并建立回归测试。
4. 已运行 `scripts/verify_native.py` 验证 AGY 终端恢复（`compatibility/evidence/agy-terminal.json`）及 DeepSeek Harness SDK JSON-RPC 会话恢复（`compatibility/evidence/dsh-rpc.json`）。
5. 真实远端模型续聊与外部插件（如 DeepSeek Harness 尚未内置于 npm 包的 `github:deepseek-harness/turtle-ui` TUI 插件）仍不在当前验收范围内；Claude 和 Codex 同样仍需各自版本下的专用原生会话与恢复证据。
