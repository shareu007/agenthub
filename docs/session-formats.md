# Session 存储格式调查

调查日期：2026-11-12。优先参考上游文档/源码，并只读检查本机已安装 Agent 的存储结构。没有将真实对话、认证文件或专有二进制复制进仓库；测试均使用合成数据。下述结论区分官方文档与本机观测，不保证未测试的新版本自动兼容。

## Claude Code

- 官方：[管理会话](https://code.claude.com/docs/en/sessions)、[设置](https://code.claude.com/docs/en/settings)。文档说明本地项目会话和 `claude --resume`。
- 来源：`$CLAUDE_CONFIG_DIR/projects/<encoded-project>/<id>.jsonl`，默认 `~/.claude/projects`。
- 本机观测：JSONL 不一定以用户消息开头，可先出现 `last-prompt`、`mode`、`permission-mode`、`attachment` 和 `file-history-snapshot`。消息有 `type/sessionId/cwd/uuid/parentUuid/timestamp/message`；正文是字符串或 Anthropic content block 数组。
- 实现：只发现项目目录内的主会话 JSONL（深度 2）；使用 `sessionId`，必要时用文件名 ID，工作目录读取 `cwd`。支持自定义标题、摘要、工具调用/结果、思考内容与未知记录原文。嵌套 subagent transcript 常共享父会话 ID，当前不独立导入，避免覆盖主会话；不承诺把子代理附件的完整内容展开进主对话。
- 恢复：`claude --resume <id>`，携带项目工作目录和对应 `CLAUDE_CONFIG_DIR`。

## Codex CLI

- 上游：[OpenAI Codex](https://github.com/openai/codex)、[协议源码](https://github.com/openai/codex/blob/main/codex-rs/protocol/src/protocol.rs)。本机安装包 `@openai/codex 0.154.0`。
- 来源：`$CODEX_HOME/sessions/YYYY/MM/DD/rollout-*.jsonl` 和 `$CODEX_HOME/archived_sessions`，默认 `~/.codex`。读取 rollout 文本，不依赖派生状态数据库或 `history.jsonl`。
- 本机观测：首条 `session_meta` 的 payload 包含 `id/cwd`，其他记录包括 `response_item`、`event_msg`、`turn_context`、`world_state`、`token_usage_record`，外层带 timestamp，部分带 ordinal。
- 实现：消息/工具调用/结果/推理摘要来自 `response_item`；`event_msg` 保留并展示为事件，避免重复建立对话正文索引。其他事件保留原 JSON；显示顺序使用文件顺序。缺少 `session_meta` 的未知旧格式明确报错。
- 恢复：`codex resume <id>`，携带 `CODEX_HOME` 和工作目录。读取 archived 文件不代表所有 Codex 版本均允许直接恢复它；不会修改原始归档状态。

## OpenCode

- 上游：[CLI 文档](https://opencode.ai/docs/cli/)、[当前表定义](https://github.com/anomalyco/opencode/blob/dev/packages/core/src/session/sql.ts)、[消息类型](https://github.com/anomalyco/opencode/blob/dev/packages/schema/src/session-message.ts)。
- 来源：`$XDG_DATA_HOME/opencode/opencode.db`，默认 `~/.local/share/opencode`；主数据库优先于遗留 JSON 树，避免迁移残留重复导入。
- 本机观测：有 `session/message/part` 表，同时存在仅存 `agent-switched` 事件的 `session_message` 表。只看到新表就切换读取会丢失全部旧消息，因此需要按会话代际识别并合并。
- 当前上游 v2：`session_v2`，消息 `session_message`（含 `type/seq/time_created/data`），助手片段 `session_part`（含 `ordinal/type/data`）。按消息 seq 和片段 ordinal 排序。上游 `data` 不一定含 type，角色来自关系列。
- 实现支持：v1 `session/message/part`；过渡期 `session + message + session_message`；v2 `session_v2/session_message/session_part`。按表/列能力探测，未知 schema 不做“成功但零会话”的猜测。多记录 JSON 全部保留，工具 state 等未知结构以原 JSON 展示。
- 旧 JSON：`storage/session/<project-id>/<session-id>.json`、`storage/message/<session-id>/*.json`、`storage/part/<message-id>/*.json`（也兼容 `<session-id>/<message-id>/` 的 part 路径）。最早的 `storage/session/info` + `metadata.assistant` 代际未承诺支持。
- 恢复：`opencode --session <id>`。自定义来源必须仍符合 `<XDG_DATA_HOME>/opencode/opencode.db`；其他布局需要使用对应原生安装自行恢复。

## Pi Coding Agent

- 上游：[SessionManager 源码](https://github.com/badlogic/pi-mono/blob/main/packages/coding-agent/src/core/session-manager.ts)、[CLI 参数源码](https://github.com/badlogic/pi-mono/blob/main/packages/coding-agent/src/cli/args.ts)。本机 `@earendil-works/pi-coding-agent 1.5.0`，会话版本 3。
- 来源：`~/.pi/agent/sessions/<encoded-cwd>/*.jsonl`；读取 `PI_CODING_AGENT_DIR`，并支持显式 `PI_CODING_AGENT_SESSION_DIR` 来源覆盖。
- 首条 `type=session`，含 `version/id/timestamp/cwd/parentSession`。后续 `type=message`，有条目 ID、parentId 和 message；也有 `session_info`、`model_change`、compaction、branch_summary、自定义事件与上下文编辑。
- 实现：保留所有条目与 parentId；自定义会话名取最后的 `session_info.name`。读取完整历史树，不做当前 leaf 重放。高于 v3 的版本保留原文并给出警告。
- 恢复：`pi --session <absolute-path>`，不能把 `--resume <id>` 当作直接恢复参数；`--resume` 是交互选择器。

## Google Antigravity CLI（agy）

- 官方：[会话管理](https://antigravity.google/docs/cli/conversations)、[CLI 参数](https://antigravity.google/docs/cli/reference)、[设置](https://antigravity.google/docs/cli/using/)、[更新记录](https://antigravity.google/docs/changelog)。官方文档确认 workspace scope 与 `--conversation`；未发布稳定磁盘 schema。
- 本机观测的 CLI 存储根是 `~/.gemini/antigravity-cli`，而非旧 Antigravity IDE 的 `.pb` 文件目录，也不是 Gemini CLI 的 JSON chats。
- 汇总库：`conversation_summaries.db`，`conversation_summaries` 表，字段有 `conversation_id/title/preview/workspace_uris/last_modified_time/parent_conversation_id/...`。另有内部 FTS 表；AgentHub 不借用其可能不完整的搜索正文来冒充完整对话。
- 每会话库：`conversations/<id>.db`。本机 `steps` 表包含 `idx/step_type/status/has_subtrajectory/metadata/error_details/permissions/task_details/render_info/step_payload/step_format`，并存在 `trajectory_meta`、`gen_metadata`、`executor_metadata`、`parent_references`、`trajectory_metadata_blob`、`battle_mode_infos`。
- 已观测的 `step_format=0` payload 是包含步骤类型、metadata 和 oneof 内容的完整 Protobuf wire message。用户步骤类型 14 的文本路径是 `19.2`（回退 `19.3.1`）；助手步骤 15 的显示文本路径是 `20.8`（回退 `20.1`）；步骤创建秒时间来自 `5.1.1`。这些路径是本机观测，不是 Google 提供的兼容性承诺。
- 实现：按 idx 读取全部 steps；解码已核实的用户/助手路径，其他步骤展示有字段号的 wire tree，并保留所有原 step payload 及相关步骤列的 Base64。未知 step_format、损坏 protobuf 或缺失已知文本字段标记 opaque 和 warning。侧表中的附件/轨迹不展开；不能宣称所有工具步骤已转换为统一语义对象，也不包含上游未保存在 steps 中的内容。
- `workspace_uris` 解析本地 `file:///...` 和 `file://localhost/...`，按 UTF-8 percent decoding 还原路径；不把远程 URI 当成本地工作目录。
- 恢复：`agy --conversation <id>`，在原项目目录启动。未查证可覆盖存储根的原生参数，因此自定义 AGY 来源只提供索引/读取，恢复入口明确报错。

## 安全降级与升级策略

每个版本化格式都应由合成 fixture 体现已验证形状。未知列可忽略；缺失必需表、非法 JSON 或源复制不稳定则整来源失败，并保持原索引。未知记录保留 raw，不安静删除。适配器行为发生变化时递增 Index 的 `PARSER_VERSION`，使旧缓存被重新解析。支持范围扩大需要补 fixture 和实际构建验证。
