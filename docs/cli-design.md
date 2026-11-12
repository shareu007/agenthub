# CLI 界面设计

采用现有子命令模型，不增加 TUI、后台进程或额外终端渲染依赖。人类输出帮助用户发现下一步；自动化使用独立的 JSON / JSON Lines 接口。

## 浏览

列表按更新时间排序，采用两行元信息卡片，避免把完整 ID、中文标题和长项目路径挤进固定宽表格。输出示意：

```text
AgentHub · Sessions · 1 shown

pi:01a1cd90-1c83-7753-bc2e-2d0c9bac8cf4  AgentHub native fixture: 渲染问题.
  2026-11-12 02:22:16 UTC · 4 records · source available
  Project: /tmp/agenthub-fixture-project

View: agenthub show <agent:id>
Resume: agenthub resume <agent:id> --exec
```

计数是原生记录数，包括头部、系统和设置事件，不冒充问答轮数。缓存来源标为 `cached only`。满页时显示下一页 offset 提示；这不表示已经知道后面一定有数据。过滤后的空结果给出同步/调整过滤器建议。

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
- 完整性：不截断身份、路径、标题或正文，也不自行分页/启动 pager；适合终端滚动及管道。
- 恢复预览：保持 stdout 为可复制的 shell 命令；只有显式 `--exec` 才交接进程。
