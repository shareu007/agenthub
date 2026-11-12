# Session transfer

参考 [showagent](https://github.com/aytzey/showagent/tree/87e8a4c38a2262dd1a62624f3780b51ee5f48f6d) 的会话移交流程，实现为独立 Rust crate `agenthub-transfer`。它将已有索引中的对话复制成新的原生会话，不修改源会话、不搬移项目文件、不自动调用模型。

## 命令与列表入口

```sh
ah transfer codex:SOURCE_ID --to pi --dry-run
ah transfer codex:SOURCE_ID --to pi
ah transfer pi:SOURCE_ID --to claude --last 100
ah transfer pi:SOURCE_ID --to opencode --cwd /absolute/project
ah transfer claude:SOURCE_ID --to codex --json
# convert 是同一命令的别名
ah convert codex:SOURCE_ID --to pi --dry-run
```

`--dry-run` 只展示源 ID、目标、项目、目标存储根、消息数量和丢弃项，不创建目标目录或原生会话。正式执行创建新 ID，输出文件、恢复命令，并刷新目标 Agent 的本地索引。刷新失败会明确告知原生会话已经创建，避免仅为刷新而重复生成会话；可以运行 `ah sync`。

列表中选择会话后按 `t`：按 `1` Claude、`2` Codex、`3` OpenCode、`4` Pi 选择目标；检查预览后按 `y` 写入，Esc/q 取消。确认前不会写入。预览需要至少 60×16 的终端。完成后切换至目标分类，并选择新会话。按 Enter 才启动原生程序；转移本身不启动模型。

部分原生格式会从首条用户消息重新派生标题；不迁移原生标题索引或运行配置。

同 Agent 转移也可用，行为是复制/分支，原 ID 不变，新副本获得另一个 ID。CLI `--last N` 按用户/助手消息条数截取，不是 N 对问答；列表入口默认复制完整的可转移活动分支。

## 支持范围

| 来源 | 读取与选择 | 目标 |
| --- | --- | --- |
| Claude | 主 transcript 最后对话叶的 parentUuid 链；排除 sidechain | 新 JSONL、UUID 和父子链，写到 `<CLAUDE_CONFIG_DIR>/projects/<encoded-project>` |
| Codex | response_item 的 user/assistant message；不复制镜像 event_msg | 新 rollout JSONL；同时生成 history 所需的 user/agent message 事件 |
| OpenCode | 原生 text parts、用户 text | 使用原生 `opencode import`，不直接写其数据库；仅启用 1.x 导入协议，实测版本 1.19.5 |
| Pi | v2/v3 沿最后叶的 parentId 链；丢弃废弃分支 | 原生 v3 JSONL 与新的消息树 |
| AGY | 已成功解码的用户/助手文本；不透明步骤不能当正文 | 不支持目标写入：尚无已核实的安全原生导入协议 |

目标存储根使用 AgentHub `[sources]` 配置及既有环境变量规则。Claude 的目标根必须叫 `projects`，OpenCode 目标目录必须符合 `<XDG_DATA_HOME>/opencode` 布局；Pi 的生成文件可用绝对路径恢复。目标项目必须存在且为绝对路径，默认使用源项目，也可用 `--cwd` 显式改变。

普通文件目标使用私有临时文件、fsync 和不可覆盖的原子落盘，文件权限 `0600`、新目录 `0700`（Unix）。新会话不会覆盖已有文件。文件发布后若目录同步或索引刷新失败，会返回已创建会话的身份和警告，不引导重复写入。OpenCode 通过原生 CLI 导入，检查退出码和原生确认 ID；限制运行时间及日志量。未知 2.x 导入协议明确拒绝，不能猜测写数据库。

## 保留和丢弃

只移交用户和助手的可见文字。工具调用/结果、推理内容、附件/图片、权限审批、运行配置、模型及 token 状态、subagent 内部数据不会移交；原生格式中必要的 API/model 字段标记为合成历史，不假装来自目标模型。切换后继续生成时，需要在目标原生 Agent 中确认可用模型和认证。

Pi/Claude 的分支链存在重复 ID、环或缺少必需父记录时拒绝转移，不把所有分支拼成一个对话。Pi 无 ID 的旧格式按线性记录读取；声称是 v2/v3 却缺少必需 ID 时拒绝。

源是最后一次成功索引的快照；需要最新内容时先 `ah sync`。原始来源已消失时仍可显式复制缓存，并在预览标注。文字按原值复制，不进行秘密字符串替换；JSON/文本预览不回显完整对话。收集阶段限制为 64 MiB 可转移文本及一百万条结构记录，超限拒绝，避免无界加载。

## 验证证据

- Rust 单测：三个文件目标的 Adapter 回读、只复制文本、新 ID/文件不覆盖、Unix 权限、Pi 活动分支、截取、坏链拒绝；CLI 测试验证 dry-run 不写目标、创建后立即可查、源不变、AGY 目标拒绝。
- PTY 回归：列表 `t` 预览与取消、确认后创建新 Codex 会话、源保持不变，不调用真实模型。
- 受控非敏感原生会话测试（Linux）：OpenCode 1.19.5 → Pi 1.5.2，真实 Pi RPC/终端确认新 ID 和历史；Pi 1.5.2 → OpenCode 1.19.5，经原生导入后终端确认历史；Pi → Codex 0.167.0-alpha.4，真实 app-server `thread/resume` 确认新 ID 和历史。
- Claude 2.1.322 尚未登录：只完成格式回读，不能宣称真实恢复加载通过。AGY 1.5.6 已合并原生写出/恢复证据；新增原生夹具转移测试确认只复制可见回复，排除思考及工具步骤。
- 所有原生测试均不发起 `turn/start` 或模型请求，不等于模型续聊或工具执行验收。详细机器证据在 `compatibility/evidence/transfer-*.json`。

可对已创建的专用测试会话重跑原生探针：

```sh
python3 scripts/verify_native.py --agenthub /path/to/ah --config /path/to/test-config.toml \
  --session pi:NEW_ID --expect UNIQUE_TEST_MARKER --probe pi-rpc --output /tmp/pi-result.json
python3 scripts/verify_native.py --agenthub /path/to/ah --config /path/to/test-config.toml \
  --session codex:NEW_ID --expect UNIQUE_TEST_MARKER --probe codex-loader --output /tmp/codex-result.json
```

showagent 是设计/格式互操作参考，并非运行依赖；未引入其删除、MCP、更新器、远程分享导入等功能。参考版本及 MIT 许可见 `THIRD_PARTY_NOTICES.md`。

AGY 1.5.6 夹具 → Pi 1.5.2 的原生 RPC 历史加载已验证（transfer-agy-to-pi.json）。

AGY 可见回复从索引已保留的原始 Protobuf 按需提取，不新增 decoded-reply 缓存字段；旧格式记录同样保留这些原始字节。显示和全文搜索仍可包含原始思考/工具记录，transfer 则只选已核实的可见正文路径。
