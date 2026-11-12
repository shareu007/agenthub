# Session 管理修改与 review

本次覆盖 transfer 后定位、大小排序、持久显示名、原生永久删除、客户端检测及终端对比度。未新增 README。

## 行为与一致性

- transfer 后清除会隐藏新记录的项目/时间/搜索条件，按同一 SQL 排序表达式计算新 ID 的全局位置，再加载对应数据页并选中。测试使用反序 Size、每页 1 条及限制源时间的条件，避免只验证第一页。
- Schema v2 在一个事务中迁移 v1。大小从已有 raw 字节计算，支持原生文件已消失的缓存；显示名不被同步 upsert 覆盖，标题/项目 FTS 随修改原子更新。测试覆盖重新打开、强制同步、恢复原名、中文与无效标题。
- 删除预览不写原生来源。Claude/Codex/Pi 确认时重新验证原生文件身份、来源根和指纹，拒绝更换身份或符号链接指向其他来源的情况；不递归删除项目目录。
- OpenCode v1 使用官方 `session delete`，拒绝带子会话的删除，并用只读快照确认原生记录消失。退出码 0 而未删除时保留索引。预检和核查复用 WAL-aware 私有快照，不在来源创建 SHM。AGY、OpenCode 旧 JSON/v2 暂未验证原生删除契约，明确拒绝，不实现假删除。
- 原生删除成功后事务清理该会话的 messages、documents、FTS 和孤立 source。无回收站或逐次全库 VACUUM；索引清理失败会说明原生已经删除，可重试清理。
- 默认 sync 只对 PATH 中存在可执行文件的客户端执行读取。安装检测不启动 Agent，不检验登录；缺失时保留已有索引，离线导入需显式 `--include-unavailable`。列表提示 CLI 缺失，Enter 不退出选择器。
- 恢复先解析程序和相对 PATH，再切换项目；错误区分程序缺失、目录缺失和解释器/加载器问题。支持 `resume --cwd`。终端选中行使用默认调色板反色，无固定蓝/青背景；快捷键/时间灰色，标题加粗、Agent 青色、大小绿色。

## 性能与缓存

大小在同步原本就需要的单次 JSON 序列化上累计，不复制原文、不逐列表重算、不读取共享 DB 文件大小。显示名是用户持久数据，不是可随意丢弃的缓存。新增两个 metadata 字段，没有新增整库、历史搜索或 decoded-reply 缓存。

列表仍只保存当前数据页和分类计数；定位由 SQL 完成，无需加载全部会话对象。原生删除检查在交互确认后重新执行以防数据变化，非交互 `--yes` 不做多余的同轮预检。清理只检查当前会话的 source，不扫描所有孤立来源。生成标题改为流式截取前 120 字符，避免长首条消息产生完整临时字符串/单词数组。

手工性能复现：

```sh
python3 scripts/benchmark_sessions.py --binary target/release/ah
```

Linux x86_64 单次合成负载（1,001 会话 / 120,000 消息，源约 47.9 MiB，含 20,000 条消息的长分支）抽查：无变化同步约 16 ms，50 条列表及大小排序约 5–8 ms。普通列表观察到 RSS 约 5 MiB，完整长分支 transfer 预览约 34 MiB。数据基于热缓存和非独占环境，不是跨平台性能保证；2 ms 采样可能无法取得短命令的有效 RSS，脚本对此报告空值。索引约 216 MiB，保留了既有 raw/正文/FTS 磁盘取舍，没有宣称磁盘零冗余。

## 验证

- `cargo test --locked --workspace`：48 个 Rust 测试通过。
- `cargo clippy --locked --workspace --all-targets -- -D warnings`、release workspace 构建通过。
- SQL 8 项、原生夹具哈希/结构验证通过。
- 双入口 release PTY 验证：选择恢复/终端还原、所有分类、搜索、大小反序跨页 transfer、持久重命名、取消/确认永久删除、缺失 CLI、去除固定背景色及纯文本模式。
- OpenCode 1.19.5 真实删除 smoke：临时目录内先由原生 CLI 建库，再放入已有脱敏夹具；官方命令删除后确认 session/message/part 行全部移除、`integrity_check=ok`、AgentHub 无残留会话。不调用模型，不访问用户存储。

原生删除 smoke 需要固定版本可执行文件在 PATH：

```sh
python3 scripts/verify_native_delete.py --binary target/release/ah
```

本地验证为 Linux；macOS 通过现有 GitHub Actions 的 stable / Rust 1.85 双工具链矩阵检查，不将未运行的结果记为通过。AGY 原生删除仍是明确边界。
