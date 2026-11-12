# CLI 界面设计

`agenthub` 和 `ah` 是同一程序的两个入口，`cargo install --locked --path crates/cli --force` 会同时安装。无参数默认进入 list；`help`、`--help` 或 `help list` 才显示使用指南。终端列表使用按需选择器，退出即结束，无后台进程；自动化保留独立 JSON / JSON Lines 输出。终端控制使用 crossterm，中文裁剪按显示宽度处理，不引入完整 TUI 框架。

## 浏览

默认按 Agent 分类，组内按更新时间倒序。静态列表采用元信息卡片；交互列表随终端尺寸显示当前选中项与组标题，并在屏幕底部保留操作提示。静态输出示意：

```text
AgentHub · Sessions · 1 shown · group: Agent · sort: Time

── pi ──
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
ah list --group-by none --sort time      # 全局时间排序
ah list --group-by none --sort agent     # 全局 Agent 排序
ah list --group-by project --sort created --reverse
ah list --no-interactive                 # 只打印，不打开会话
ah list --group-by project --sort time --json
```

`--sort` 支持 `time`（更新时间，别名 updated）、`created`（创建时间）、`agent`、`project`。时间默认最新在前，Agent/项目默认 A-Z；`--reverse` 反转组内排序，分类本身仍 A-Z，未知时间/项目排末尾。分类和排序在数据库中针对所有匹配会话执行后，再应用 `--limit` / `--offset`，并非只重排当前页。JSON 保持扁平数组与原有字段，顺序遵循相同规则。

当 stdin 和 stdout 都是终端且 TERM 不是 dumb 时，`list` 自动进入选择器：

| 按键 | 行为 |
| --- | --- |
| ↑/↓ 或 j/k | 选择上/下一个会话 |
| Home / End | 当前数据页第一项/最后一项 |
| PageUp / PageDown | 翻动屏幕内的显示页 |
| Enter | 原地恢复选中会话，等同 `resume <id> --exec` |
| g | 循环按 Agent / 项目分类 / 不分类 |
| s | 循环更新时间 / 创建时间 / Agent / 项目排序 |
| r | 反转排序方向 |
| n / p | 下一页/上一页数据（每页默认 50 项） |
| q / Esc / Ctrl-C | 退出，不启动 Agent |

切换分类或排序会回到数据第一页。终端内的长文本按显示宽度裁剪，只影响预览；静态/JSON 不裁剪内容。来源不存在或恢复计划不支持时留在选择器显示错误，不启动原生程序。回车后先恢复正常终端及光标、退出选择屏幕、关闭索引连接，再将进程交给原生 CLI，保留其退出码与信号；原生 Agent 的登录要求和写入行为保持不变。

管道、重定向、`--json`、`TERM=dumb` 默认不进入交互。`--no-interactive` 可主动禁用；`--interactive` 则强制要求有效终端，和 JSON/重定向一起使用会报错。默认列表只读已有索引，不隐式同步；新安装无会话时先运行 `ah sync`。

`python3 scripts/verify_selector.py --bin-dir target/release` 使用真实 PTY 和隔离的原生进程替身，覆盖两个入口、选项切换、分页、取消、回车交接、失败来源和终端恢复；这不是原生 Agent 兼容性验证。

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
