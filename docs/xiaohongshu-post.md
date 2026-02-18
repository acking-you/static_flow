# 小红书推广文案

AI 改变了我的学习方式，于是我用 Rust + LanceDB 造了一套知识系统

---

AI 时代，你还在学完就忘吗？

我是一个 Rust 开发者，过去一年 AI 编程工具井喷式爆发——Claude 4.6 Opus、GPT-5.3 Codex、Claude Code、Skills 工作流……但对我来说，最大的变化不是写代码更快了，而是学习方式彻底变了。

以前学新技术：找教程 → 抄例子 → 踩坑 → 搜 StackOverflow → 学完就忘 → 循环。

现在完全不同：跟 AI 对话理解原理 → Skill 自动生成结构化笔记 → Agent 一键发布到知识库 → 语义搜索随时复用。

每一次学习，都变成了可检索、可积累、可复用的知识资产。

于是我用 Rust 全栈造了 StaticFlow——一个本地优先的 AI 知识管理系统（实际上是我的个人博客延伸而来）。

我手动写几句有意思的点：这个项目就是我的个人博客，前端用github pages，后端在本地通过pb-mapper映射出去（一个很早之前写的很好用的公网映射工具），而数据则是直接存在huggingface上😂，几乎全链路白嫖

技术栈：
- 前端：Yew 框架，编译成 WebAssembly
- 后端：Axum + Tokio 异步运行时
- 数据库：LanceDB 嵌入式向量数据库
- 工具链：sf-cli（给 Coding Agent 用的操作接口）
- AI 能力：7 个 Skill 文件串联完整工作流

核心功能：
- 混合搜索：FTS 全文 + 向量语义，RRF 融合排序
- AI 评论：选中文本精确评论，Codex 读全文生成回复
- Skill 工作流：从对话到发布全链路自动化
- 双语支持：AI 整篇理解后重写英文版本
- 本地优先：数据永远在自己手里，不依赖云服务

Anthropic 专家说过："Don't build agents, build skills instead."
StaticFlow 的 7 个 Skill 就是这句话的实践——每个 Skill 是一个 Markdown 文件，AI 加载后就知道该怎么执行任务，不需要写框架代码。

项目完全开源，欢迎来玩：
GitHub：https://github.com/acking-you/static_flow
网站：https://acking-you.github.io

#Rust #LanceDB #AI编程 #开源项目 #知识管理 #向量数据库 #ClaudeCode #WebAssembly #本地优先 #AI学习 #全栈开发 #程序员日常
