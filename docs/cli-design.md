# CLI 界面设计

`agenthub` 和 `ah` 是同一程序的两个入口，`cargo install --locked --path crates/cli --force` 会同时安装。无参数默认进入 list；`help`、`--help` 或 `help list` 才显示使用指南。终端列表使用按需选择器，退出即结束，无后台进程；自动化保留独立 JSON / JSON Lines 输出。终端控制使用 crossterm，中文裁剪按显示宽度处理，不引入完整 TUI 框架。

## 浏览

默认选择 Time，不按 Agent/项目分组，所有会话按更新时间倒序排列（最新在前）。静态列表采用元信息卡片；交互界面参考 tig：固定状态栏、单行紧凑列表、列标题、元信息弱化、会话标题高亮、选中整行反色；光标移动时连续滚动，并在屏幕底部保留操作提示。静态输出示意：

```text
AgentHub · Sessions · 1 shown · group: Time · sort: Time

pi:01a1cd90-1c83-7753-bc2e-2d0c9bac8cf4  AgentHub native fixture: 渲染问题.
  2026-11-12 02:22:16 UTC · 4 records · source available
  Project: /tmp/agenthub-fixture-project

View: agenthub show <agent:id>
Resume: agenthub resume <agent:id> --exec
```

计数是原生记录数，包括头部、系统和设置事件，不冒充问答轮数。缓存来源标为 `cached only`。满页时显示下一页 offset 提示；这不表示已经知道后面一定有数据。过滤后的空结果给出同步/调整过滤器建议。

## 分类、排序与选择

```sh
ah                                      # 默认 list；终端内可选择
agenthub                                # 同上
ah help                                 # 使用指南，不读配置/索引
ah list --group-by agent --sort time     # 按 Agent 分类，组内最新优先
ah list --group-by project --sort time   # 按项目分类
ah list --group-by time --sort time      # 全局时间排序
ah list --group-by time --sort agent     # 全局 Agent 排序
ah list --group-by project --sort created --reverse
ah list --no-interactive                 # 只打印，不打开会话
ah list --group-by project --sort time --json
```

`--group-by` 默认为 `time`，表示全局列表；保留 `none` 作为兼容别名，也可选 `agent`、`project`。终端最顶栏仅显示 `AgentHub`。

`--sort` 支持 `time`（更新时间，别名 updated）、`created`（创建时间）、`agent`、`project`、`size`。大小默认从大到小，时间默认最新在前，Agent/项目默认 A-Z；`--reverse` 反转组内排序，分类本身仍 A-Z，未知时间/项目排末尾。分类和排序在数据库中针对所有匹配会话执行后，再应用 `--limit` / `--offset`，并非只重排当前页。JSON 保持扁平数组，增加 `size_bytes` 字段，顺序遵循相同规则。

当 stdin 和 stdout 都是终端且 TERM 不是 dumb 时，`list` 自动进入选择器：

| 按键 | 行为 |
| --- | --- |
| ↑/↓ 或 j/k | 选择上/下一个会话 |
| Home / End | 当前数据页第一项/最后一项 |
| PageUp / PageDown | 翻动屏幕内的显示页 |
| Enter | 原地恢复选中会话，等同 `resume <id> --exec`；搜索输入中只应用查询 |
| 1–6 / Tab / 左右键 | 1 All、2 Claude、3 Codex、4 OpenCode、5 Pi、6 AGY 分类入口；transfer 目标沿用相同 Agent 编号 |
| / | 搜索整个索引的正文、标题、项目和 ID，实时过滤，不限当前页 |
| 搜索中 Backspace / Ctrl-U | 删除字符 / 清空输入 |
| e | 修改持久显示标题（Enter 保存，Ctrl-U 清空，Esc 取消）；不改原生文件和 ID |
| d | 永久删除选中的原生会话；展示身份后输入 `DELETE` 并回车确认，Esc 取消 |
| t | 转移到新原生会话：选择目标、预览、y 确认，详见 session-transfer.md |
| g | 循环 Time（全局列表）/ Agent / 项目分类 |
| s | 循环更新时间 / 创建时间 / Agent / 项目排序 |
| r | 反转排序方向 |
| n / p | 下一页/上一页数据（每页默认 50 项） |
| q / Ctrl-C | 退出，不启动 Agent；搜索输入中 q 是普通文字 |
| Esc | 取消正在编辑的搜索；正常模式下先清除搜索，再次按才退出 |

顶部固定保留五种 Agent 和 All 的分类入口，包括计数为 0 的分类；数字键可直接切换，不需要翻过前一分类的全部数据。数量针对全部匹配索引统计，不受 50 条分页截断；0 表示当前过滤下没有已索引会话，需检查过滤条件或运行 ah sync，而不是隐藏该 Agent。分类数量遵循项目/时间/搜索条件，忽略当前 Agent tab 以保持其他分类可见。

`/` 打开实时搜索；Enter 只应用查询，不恢复会话。Esc 取消编辑恢复原查询；正常模式下 Esc 清除已应用搜索，或用 `/` 后 Ctrl-U、Enter 清除。查询使用参数绑定和字面短语，正文通过 FTS5/短词回退；多条正文命中同一会话时只列一次。

切换分类或排序会回到数据第一页。终端内的长文本按显示宽度裁剪，只影响预览；静态/JSON 不裁剪内容。来源不存在或恢复计划不支持时留在选择器显示错误，不启动原生程序。回车后先恢复正常终端及光标、退出选择屏幕、关闭索引连接，再将进程交给原生 CLI，保留其退出码与信号；原生 Agent 的登录要求和写入行为保持不变。

管道、重定向、`--json`、`TERM=dumb` 默认不进入交互。`--no-interactive` 可主动禁用；`--interactive` 则强制要求有效终端，和 JSON/重定向一起使用会报错。默认列表只读已有索引，不隐式同步；新安装无会话时先运行 `ah sync`。

`python3 scripts/verify_selector.py --bin-dir target/release` 使用真实 PTY 和隔离的原生进程替身，覆盖两个入口、五分类与大于 50 条的索引、全文/中文搜索与取消、标题/选中行高亮、无颜色模式、选项切换、分页、转移预览/确认、回车交接、失败来源和终端恢复；这不是原生 Agent 兼容性验证。

## 搜索与阅读

搜索输出命中数、查询、完整会话 ID、命中记录号和摘要。标题/项目文档使用 `title/project` 标签，不向用户展示内部 ordinal=-1。空结果提示缩短关键词或刷新索引。

对话顶部展示身份、标题、更新时间、记录数、项目和警告；每条记录展示角色、编号、原始 kind 和 UTC 时间。未知时间显示 `unknown time`。原始记录和 JSON 不经过装饰渲染。

## 同步、诊断和错误

同步逐 Agent 显示 `OK/ERROR` 及 indexed/unchanged/missing/discovered。部分失败返回非零退出码，并说明失败来源保留上次成功索引。

`doctor` 展示来源路径、发现状态、来源数量、原生 CLI 路径、发现警告和错误；提示其不能证明已登录、格式可读或真实恢复成功。来源缺失是可选 Agent 的正常状态。发现不完整/错误返回非零。

## 终端与脚本

- 颜色：`--color auto|always|never`；auto 检查终端、NO_COLOR 和 TERM=dumb。
- JSON：原有字段和结构保持；不加入人类摘要、颜色或提示。
- raw：只输出原生记录 JSON Lines，可直接交给 JSON 工具。
- 安全：人类输出过滤 transcript 中的终端控制字符；不执行正文中的 ANSI/OSC 控制序列。
- 完整性：静态/JSON 输出不截断身份、路径、标题或正文，不启动外部 pager；交互预览按终端宽度裁剪和分屏。
- 恢复预览：保持 stdout 为可复制的 shell 命令；显式 `--exec` 或选择器中按 Enter 时交接进程。

## 可用客户端、大小与会话管理

默认 `sync` 只读取当前 PATH 中有可执行文件的 Agent。检测不启动 Agent、不验证登录，不把 shell 函数或别名当成安装；缺失客户端会跳过并保留旧索引。显式 `--agent` 指定未安装的客户端返回错误。离线历史导入可用 `sync --include-unavailable`。旧列表记录仍可查阅，缺失 CLI 显示 `[CLI missing]`，按 Enter 会在列表内解释原因。

恢复会先校验工作目录、解析可执行文件，再切换到会话项目；相对 PATH 在原工作目录解析。项目已移动时可用 `ah resume <id> --cwd /new/project --exec`。

`SIZE` / JSON `size_bytes` 统计索引内该会话原生记录的紧凑 JSON UTF-8 字节数（含记录头/事件，AGY 的二进制字段按保留的 Base64 JSON 计算）。这用于比较会话内容规模，并非单条会话的物理磁盘占用；共享数据库整文件大小不应重复计给每个会话。数据在同步时累计，列表不遍历文件或正文。`ah list --sort size --group-by time` 可查看全局大小排序；`s` 轮换到 Size，`r` 反转。

transfer 成功后直接刷新目标索引，切换目标 Agent，清除搜索/项目/时间筛选。保留分类、排序和方向，计算新 ID 的完整排序位置，自动翻到所在数据页并选中它；无需手动 sync。原生创建成功但索引刷新失败时显示单独提示，避免重复创建。

```sh
ah rename pi:SESSION_ID "新的标题"  # 持久显示名，同步/重启后保留
ah rename pi:SESSION_ID --reset    # 恢复原生标题
ah delete pi:SESSION_ID            # 预览；不删除
ah delete pi:SESSION_ID --yes      # 不可撤销：删除原生会话及派生索引
```

永久删除支持 Claude/Codex/Pi 的单会话 JSONL 文件，以及已验证的 OpenCode v1 SQLite 存储。删除前重新核对会话身份和文件变化；OpenCode 通过官方 `session delete`，然后核对原生记录已消失，拒绝会递归删除子会话的操作。删除成功后清除派生 messages/documents/FTS，不建立回收站。AGY 暂无已验证的删除 API，提示从 `agy` 的 `/resume` 删除；OpenCode 旧 JSON 或未验证 v2 存储也明确拒绝，不会仅隐藏列表而假装已完成原生删除。

配色使用终端默认前景/背景的加粗反色作为选中行，不再固定蓝/青背景或黑字。标题加粗，Agent/组标题青色，大小绿色，时间和快捷键灰色；选中行统一使用终端默认反色。`--color never` / `NO_COLOR` 继续可用，纯文本保留 `>` 选择标记。
