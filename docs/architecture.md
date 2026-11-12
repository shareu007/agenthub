# AgentHub Core / CLI 架构

## 1. 边界与依赖

| Crate | 职责 | 不承担的职责 |
|---|---|---|
| `agenthub-core` | Agent、Session、Message、Source、Adapter、SessionSink、恢复计划和未来 Archive 类型 | SQLite、终端、网络 |
| `agenthub-adapters` | 发现来源、格式识别、转换、私有 SQLite 快照、二进制字段解码 | 自有索引、命令展示 |
| `agenthub-index` | schema 版本、来源状态、增量判断、原子更新、查询/FTS | Agent 格式细节、进程启动 |
| `agenthub-cli` | 配置、参数、文本/JSON、退出码、原生恢复进程 | 常驻服务、GUI |

Adapter 是 Rust trait，内建适配器静态注册，新增 Agent 只需实现 `agent/root/discover/read` 并在工厂注册。当前不定义跨动态库的 ABI，也不引入插件进程或网络协议。TUI/GUI 可以复用 Core 与 Index；CLI 不作为共享业务逻辑的唯一入口。

同步 `std::fs` / `BufReader` 配合单线程，避免 Tokio、线程池和监控服务的启动成本。无命令时不运行任何进程。一次同步的各来源串行导入；其他进程的查询可以通过索引自己的 WAL 读取最后一次已提交的数据。

## 2. 模型

`Session.key = <agent>:<native_id>`。它不包含项目名或源文件路径，原文件移动到 Codex `archived_sessions` 后保持身份。相同原生 ID 出现在不同 Agent 时不会冲突。当前同一个 Agent ID 被视为同一逻辑会话；若配置重复副本，后导入的副本成为当前来源，不提供同时跟踪同 ID 多份副本的承诺。

`project_path` 是 Agent 自己记录的工作目录，不从编码后的 Claude 文件夹名反推，避免路径转换歧义。`created_at/updated_at` 是可空的 Unix 毫秒值；不使用扫描时间冒充会话时间。标题优先原生自定义标题，否则采用首条用户文本摘要或 ID。

`Message` 保留原生 ID、parent ID、记录顺序、角色、类型、时间、可搜索文本与完整 JSON 数据。Pi 的树形历史和 Claude 的 parent UUID 不会被强行折叠为有效对话；顺序展示与分支结构分别保留。`message_count` 是原生记录数，包含头部和系统事件，并不等于问答轮数。

未知 JSON 事件保留全部字段。Codex `event_msg` 作为事件展示、保留原文，但不重复建立全文索引，避免它与 `response_item` 的镜像文本产生重复命中。AGY 二进制记录以 Base64 保存原始 step payload 及相关步骤列，未知格式有明确 warning，不伪装成已解码正文。

## 3. 数据库

数据库由 `application_id=0x41485542` 标记，`user_version=1` 管理迁移。已有的非 AgentHub 数据库会在任何 schema/WAL 设置之前拒绝，避免误把原 Agent 数据库当作索引。所有用户过滤值使用绑定参数。

| 表 | 内容 |
|---|---|
| `sources` | 来源 ID、Agent、配置根、文件路径、指纹、解析器版本、可用状态、同步时间 |
| `sessions` | 统一元数据、来源外键、可用性、记录数、解析警告 |
| `messages` | 流式写入的原始/规范化记录，以 `(session_key,ordinal)` 定位 |
| `documents` | 每条非镜像记录的文本，加上 `ordinal=-1` 的标题/项目文档 |
| `documents_fts` | 以 `documents` 为 external content 的 FTS5 trigram 索引 |

`documents` 的 INSERT/UPDATE/DELETE 触发器维护 FTS。更新时的旧文档删除和新文档写入在同一来源事务中完成，不留下旧命中。索引包含 Agent+更新时间、项目+更新时间、来源及原生 ID 的 B-tree 索引。正文会保留在 messages 和 documents 中；用少量磁盘冗余换取简单、可维护的查询与触发器逻辑。后续应在实测后再考虑压缩和更复杂的去重。

SQLite 自带编译的版本提供 FTS5 trigram，避免 Linux/macOS 系统 SQLite 配置差异。索引 `cache_size` 目标约 4 MiB，来源快照连接约 2 MiB；这些是 SQLite 缓存设置，不是整个进程的内存上限。默认无 mmap 和定时维护线程。

## 4. 增量与一致性

1. Adapter 完整发现来源，返回遍历错误和 `complete` 状态。
2. 计算来源身份：Agent + canonical path 的 BLAKE3；指纹使用依赖路径、文件大小、纳秒 mtime 以及 Unix inode/ctime。SQLite 依赖包含 WAL/rollback journal；旧 OpenCode 包含消息、part 文件和目录元数据。
3. 指纹和解析器版本都不变，且来源仍可用时直接跳过解析。
4. 对变化来源开启 `BEGIN IMMEDIATE`，暂时标记旧会话不可用；每个读到的会话重新写入元数据、记录和 FTS 文档。
5. `SessionSink.begin/message/finish` 逐条传递，只持有当前记录；SQLite 多会话库逐会话遍历，不建立全部历史的内存集合。消息插入使用 statement cache。
6. 再次检查来源指纹；读取失败、源变化、非法格式或不完整 Sink 生命周期使本次来源事务回滚。成功后提交。
7. 只有完整遍历且没有同步错误时，才把未发现的旧来源标记为缺失。缺失来源和库内已删除的会话保留缓存供查看，默认不参与列表和搜索。

当前增量粒度是“来源”：JSONL 为单文件，AGY 为单会话数据库，OpenCode 为共享数据库。JSONL 追加后会重读该文件；OpenCode 数据库变化后会重读其中全部会话。这样能正确处理压缩、修改、截断、撤销和旧记录编辑，避免只有 append offset 时的错误。未承诺大库变化后的行级增量；后续应依据实测引入适配器专用 revision，而不是依赖一律可靠的更新时间假设。

指纹是变更检测，不是内容哈希；`sync --force` 可强制重建。AGY 汇总库变化会触发关联会话重读；一次 Adapter 实例缓存并复用汇总快照，避免每会话复制同一文件。

JSONL 单条记录上限 32 MiB；过大的记录报错并保留上次成功索引。最后一条未闭合 JSON 且无换行时被视为正在写入的尾部，已完成记录可以提交，并保存警告。中间损坏、空文件或缺失必需头部会报错。完整但尚无换行的 JSON 记录仍可读取。

## 5. 严格只读的 SQLite 来源

不能只依靠 `SQLITE_OPEN_READ_ONLY`：SQLite 在某些情形下仍可能需要在原目录创建 `-shm` 文件。也不能直接用 `immutable=1` 读取活跃主文件，因为那会忽略未 checkpoint 的 WAL。

当前实现仅用 `File` 读取原始 DB 与 WAL 到权限私有的临时目录，复制前后核对源指纹。发现变化重试，最多 3 次；存在非空 rollback journal 则要求稍后重试。副本通过只读、`query_only` 连接开启读事务，所有可能的辅助文件只出现在私有副本目录。索引结束还会核对原来源的指纹。

这保留已提交的 WAL 数据，不请求原 Agent checkpoint，不修改原始会话库、汇总库或 SHM。代价是同步变化的 SQLite 来源时需要临时磁盘空间，约为相应 DB+WAL；复制按流进行。文件集合非常活跃时可能暂时失败，这是保留旧索引并重试的正常路径。

文件指纹检测并非与原 SQLite writer 协同的事务快照 API。它提供稳定复制前后的检测；无法承诺在恶意修改文件、强行恢复所有文件身份和时间元数据时仍能发现变化。这里按正常本地 Agent 的写入行为设计，不引入文件锁去阻塞原 Agent。

## 6. 恢复与配置

Core 生成结构化 `ResumeCommand {program,args,cwd,env}`；CLI 输出 shell 引号正确的计划，只有明确 `--exec` 时才启动进程，不执行 `sh -c`。恢复必须使用原会话，不能把索引当作新的原生会话文件写回。缺失来源不可恢复；项目目录丢失或程序未安装时输出启动错误。

`doctor` 检查来源、发现错误及 PATH 中的程序文件，不运行 Agent、不创建索引。它不是原生 CLI 能否认证/恢复的联网诊断。

## 7. Archive 与后续扩展

当前只定义 `ArchiveDraft` 和 `ArchiveRepository`，不提供实现。预期未来的 Archive 有自己的逻辑项目/文件夹实体，收藏记录引用 `session_key`，自定义标题和备注属于收藏层。这样修改收藏不会重写原 Agent 文件，也不会被下一次索引更新覆盖。原生的“archived session”目录只是来源位置，和 AgentHub 收藏语义不同。

快照引用应在未来采用单独模型；其身份、可用性、设备来源和原生恢复能力不能与当前可变来源混为一谈。当前不提前加入 SSH、同步调度、Git 关联、网络服务或跨 Agent 重建上下文的协议。

## 8. 验证边界

Rust 单元/集成测试已编写，但本环境没有 Rust 工具链且网络下载不可用，尚未运行；Linux/macOS CI 也尚未执行。`scripts/verify_sql.py` 可用 Python 标准库直接执行正式 schema 和查询，核验 FTS、中文短词、触发器、回滚、索引及数据库完整性，不能代替 Rust 编译或 Adapter 集成测试。没有在本环境启动真实 Agent 恢复会话。
