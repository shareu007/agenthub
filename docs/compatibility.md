# 固定版本、原生样本与恢复验收

验收日期：2026-11-12。机器可读矩阵：[agents.json](../compatibility/agents.json)。四个已核实版本固定如下；**AGY 尚不能负责任地填入版本号**。

| Agent | 固定版本 | 原生写出样本 | 原生恢复加载 | 模型续聊 |
| --- | --- | --- | --- | --- |
| AGY | 未核实（阻塞） | 无，只有合成 protobuf 回归 | 未执行 | 未执行 |
| Claude Code | 2.1.322 | 无 | 未执行：未登录、无专用会话 | 未执行 |
| Codex | 0.167.0-alpha.4 | 无 | 未执行：无专用会话 | 未执行 |
| OpenCode | 1.19.5 | 有：本地原生 API + noReply | 真实 `resume --exec` 终端显示唯一历史标记 | 未调用 |
| Pi | 1.5.2 | 有：原生 SessionManager 写出 | 真实 `resume --exec` 终端 + RPC 身份/消息校验 | 未调用 |

Codex 是云环境自带预发布版，不能视为稳定版支持承诺。CLI `--version` 成功不是格式/恢复验收。真实恢复加载的定义是：原生程序读取原会话，终端显示仅存在于历史中的标记；Pi 另用 RPC 核对会话 ID 和两条历史消息。这里只证明历史加载，不证明继续调用模型、执行工具或认证成功。证据见 [compatibility/evidence](../compatibility/evidence)。

## 样本来源与脱敏

样本不是私人对话的复制，而是在专用空目录中由固定版本的**真实原生写入器**产生的非敏感测试会话。测试文本和助手回复是受控输入；没有模型生成内容、凭据或私人项目。此方式保留真实格式并避免把未知字段里的私人数据带进仓库。

- `tests/fixtures/pi/1.5.2.jsonl`：从安装包 `dist/core/session-manager.js` 调用 `SessionManager.create('/tmp/agenthub-fixture-project', sessionDir)`、两次 `appendMessage` 写出；一次用户消息，一次明确标注为 offline 的助手消息。真实 Pi CLI 打开后追加了 `thinking_level_change`，样本共 4 条记录。UUID、记录 ID、parentId 和时间戳为原生生成；没有人工拼装 JSONL 格式。
- `tests/fixtures/opencode/1.19.5.sql`：真实 `opencode serve --hostname 127.0.0.1`，以 `/tmp/agenthub-fixture-project` 为 directory 创建 session；向 `/session/{id}/message` 提交 `noReply: true` 和唯一标记。导出原生 project/session/message/part/session_message 表定义和测试行，SQL 导入时临时关闭外键，结束后恢复，外键完整性检查通过。未包含认证、迁移历史或其他非必要表；该夹具用于 Adapter 读取，不作为可直接导入原生客户端的完整数据库快照。
- 两份文件均保留原生生成的测试身份与非敏感时间戳、版本和模型名称；路径仅包含固定测试目录及 OpenCode 内置 global 根 `/`。这是受控样本，不代表真实复杂工具调用、附件、分支或长对话已经验收。
- SHA-256 记在矩阵；`python3 scripts/verify_fixtures.py` 验证哈希与结构，Rust 测试验证实际解析和来源不变。
- 既有 `formats.rs` 中其他内联夹具均为合成数据，不能升级宣称为某固定真实版本的样本。

原生包安装来源为 npm 官方命名空间：`@anthropic-ai/claude-code@2.1.322`、`@earendil-works/pi-coding-agent@1.5.2`、`opencode-ai@1.19.5`。本机 OpenCode 通用包装包安装器失败，按其提示安装对应平台包 `opencode-linux-x64@1.19.5`，执行 `--version` 核实。没有安装同名但来源不明的 `agy` npm 包。

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

脚本使用真实 PTY 执行 `agenthub resume <id> --exec`，不发送 prompt；在发现历史标记后终止本次创建的进程组。超时、首次信任提示、登录界面等都会阻止通过，不自动代替用户接受原生许可提示。Pi 可增加 `--probe pi-rpc`，由 AgentHub 生成的恢复计划启动真实 Pi RPC，核对原会话 ID、消息和标记，不调用模型。

原生程序应在 PATH 中，并与矩阵的精确版本相符；先设置其受支持的配置/数据目录变量指向隔离目录。不要使用生产会话：原生程序加载时可以更新自己的会话文件。终端探针适用于 Linux/macOS；本次只在 Linux 执行。

## AGY 优先验收清单与当前阻塞

1. 从 Google 官方文档确定 Linux/macOS 安装来源和精确版本，执行 `agy --version` 后再填写 pin。当前没有安装包或历史真实样本；官方 `antigravity.google` 域名被现行代理策略拒绝。网络要求已保存到云环境草稿，尚未应用。
2. 安装并按原生流程登录，在隔离测试项目中创建包含唯一标记的短对话、工具记录和附件；不使用私人工作会话，也不把密钥写入夹具。
3. 核实 conversation_summaries.db 与 steps 的实际 schema/编码，按原生写出内容建立 fixture 和来源记录；升级已支持字段必须补回归。未知二进制和侧表不能仅通过删除字段来声称完成脱敏/完整支持。
4. 将测试会话同步进 AgentHub，在原生支持的默认存储布局下运行终端探针，验证 `agy --conversation <id>` 的历史标记和身份。自定义根仍明确拒绝未经证实的恢复参数。
5. 在已认证环境中另行验证模型续聊，并核对原会话身份；这与不调用模型的恢复加载探针分开记录。

仅开放网络还不等于 AGY 完成：仍需官方安装包、认证和实际写出的会话。Claude 和 Codex 同样需要各自版本下的专用原生会话与恢复证据。此矩阵明确保留这些未完成项。
