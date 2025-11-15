# AI Agent 开发任务清单

## Phase 1: AI Agent 核心开发

### 任务 1: LanceDB Schema 设计与初始化

**目标**：定义 LanceDB 数据库 schema 并实现初始化逻辑

**子任务**：
- [ ] 设计 `articles` 表 schema
  - 字段：id (String), title (String), content (String), summary (String)
  - 字段：tags (List<String>), category (String)
  - 字段：vector (FixedSizeList<Float32, 512>) - 文本 embedding
  - 字段：created_at (Timestamp), updated_at (Timestamp)
  - 字段：file_path (String), word_count (Int32)

- [ ] 设计 `images` 表 schema
  - 字段：id (String), filename (String)
  - 字段：image_data (Binary) - 原图二进制
  - 字段：thumbnail (Binary) - 缩略图（200x200）
  - 字段：vector (FixedSizeList<Float32, 512>) - CLIP embedding
  - 字段：width (Int32), height (Int32), file_size (Int64)
  - 字段：created_at (Timestamp)

- [ ] 实现 `agent init` 命令
  - 创建 LanceDB 数据库目录
  - 初始化 articles 和 images 表
  - 创建全文索引（FTS）
  - 创建向量索引（IVF-PQ）

**文件位置**：
- `agent/src/db/schema.rs` - schema 定义
- `agent/src/db/init.rs` - 初始化逻辑
- `agent/src/main.rs` - CLI 命令入口

**依赖**：
```toml
[dependencies]
lancedb = "0.4"
arrow-schema = "50.0"
arrow-array = "50.0"
```

---

### 任务 2: Rig 框架集成

**目标**：集成 Rig AI Agent 框架，实现 LLM 调用

**子任务**：
- [ ] 初始化 Rig Agent 结构
  - 配置 OpenAI provider（支持 API key）
  - 配置 Ollama provider（可选，本地模型）
  - 实现 provider 切换逻辑

- [ ] 定义 Prompt 模板
  - 摘要生成 prompt：根据文章内容生成 100-200 字摘要
  - 标签提取 prompt：提取 3-5 个关键标签
  - 分类判断 prompt：判断文章属于哪个分类（Tech/Life/Thoughts/etc）

- [ ] 实现 LLM 调用封装
  - `generate_summary(content: &str) -> Result<String>`
  - `extract_tags(content: &str) -> Result<Vec<String>>`
  - `classify_article(content: &str) -> Result<String>`
  - 实现错误重试逻辑（最多 3 次）

- [ ] 实现文本 embedding 生成
  - 使用 OpenAI `text-embedding-3-small` 或本地模型
  - 输出 512 维向量（用于语义搜索）

**文件位置**：
- `agent/src/ai/rig_agent.rs` - Rig Agent 主逻辑
- `agent/src/ai/prompts.rs` - Prompt 模板
- `agent/src/ai/embedding.rs` - Embedding 生成

**依赖**：
```toml
[dependencies]
rig = "0.1"
reqwest = { version = "0.11", features = ["json"] }
tokio = { version = "1", features = ["full"] }
```

**Prompt 示例**：
```rust
const SUMMARY_PROMPT: &str = r#"
你是一个专业的文章摘要助手。请根据以下文章内容，生成一个简洁的中文摘要（100-200字）。

文章内容：
{content}

摘要：
"#;
```

---

### 任务 3: Markdown 处理流水线

**目标**：扫描、解析本地 Markdown 文件

**子任务**：
- [ ] 实现文件扫描逻辑
  - 递归扫描指定目录下的所有 `.md` 文件
  - 过滤隐藏文件和特殊目录（`.git`, `node_modules`）
  - 记录文件修改时间（用于增量更新）

- [ ] 实现 Frontmatter 解析
  - 使用 `yaml-rust` 解析 YAML frontmatter
  - 提取预定义的 title、tags、category（如果存在）
  - 如果 frontmatter 不存在，则全部由 AI 生成

- [ ] 实现 Markdown 正文提取
  - 移除 frontmatter 后的纯文本内容
  - 计算字数统计
  - 提取图片链接（`![](path/to/image.png)`）

- [ ] 实现文件监控（notify crate）
  - 监控文件创建、修改、删除事件
  - 增量处理变化的文件
  - 防抖动（debounce）逻辑（避免频繁触发）

**文件位置**：
- `agent/src/processor/markdown.rs` - Markdown 解析
- `agent/src/processor/scanner.rs` - 文件扫描
- `agent/src/watcher.rs` - 文件监控

**依赖**：
```toml
[dependencies]
notify = "6.0"
yaml-rust = "0.4"
walkdir = "2.4"
pulldown-cmark = "0.9"  # 用于提取文本
```

**处理流程**：
```
Markdown 文件
  ↓
解析 frontmatter（可选）
  ↓
提取正文纯文本
  ↓
提取图片链接
  ↓
生成文件元数据
  ↓
传递给 AI 处理模块
```

---

### 任务 4: 图片处理流水线

**目标**：扫描图片，生成 embedding 和缩略图

**子任务**：
- [ ] 实现图片文件扫描
  - 支持 `.jpg`, `.jpeg`, `.png`, `.webp` 格式
  - 读取图片尺寸和文件大小
  - 记录图片所属的 Markdown 文件

- [ ] 实现缩略图生成
  - 使用 `image` crate 生成 200x200 缩略图
  - 保持宽高比，居中裁剪
  - JPEG 格式，质量 85%

- [ ] 实现 CLIP embedding 生成
  - **选项 A**：调用本地 Python CLIP 服务（推荐）
    - 启动独立的 FastAPI 服务
    - POST 图片二进制到 `/embed` 端点
    - 返回 512 维向量
  - **选项 B**：使用 Rust ONNX Runtime（复杂）
    - 加载 CLIP ONNX 模型
    - 直接在 Rust 中推理

- [ ] 实现图片二进制读取
  - 读取原图完整二进制数据
  - 优化：图片 > 5MB 时压缩到 2MB（保持质量）

**文件位置**：
- `agent/src/processor/image.rs` - 图片处理主逻辑
- `agent/src/ai/clip_client.rs` - CLIP 服务客户端
- `agent/tools/clip_service.py` - Python CLIP 服务（可选）

**依赖**：
```toml
[dependencies]
image = "0.24"
reqwest = { version = "0.11", features = ["multipart"] }
```

**Python CLIP 服务示例**：
```python
# agent/tools/clip_service.py
from fastapi import FastAPI, File, UploadFile
from transformers import CLIPProcessor, CLIPModel
from PIL import Image
import torch
import io

app = FastAPI()
model = CLIPModel.from_pretrained("openai/clip-vit-base-patch32")
processor = CLIPProcessor.from_pretrained("openai/clip-vit-base-patch32")

@app.post("/embed")
async def embed_image(file: UploadFile = File(...)):
    image = Image.open(io.BytesIO(await file.read()))
    inputs = processor(images=image, return_tensors="pt")
    with torch.no_grad():
        outputs = model.get_image_features(**inputs)
    vector = outputs[0].numpy().tolist()
    return {"vector": vector}

# 启动：uvicorn clip_service:app --host 0.0.0.0 --port 8000
```

---

### 任务 5: LanceDB 写入逻辑

**目标**：将处理后的数据批量写入 LanceDB

**子任务**：
- [ ] 实现 Article 记录构造
  - 组装所有字段（id, title, content, summary, tags, category, vector, timestamps）
  - 生成唯一 ID（基于文件路径的哈希）
  - 转换为 Arrow RecordBatch

- [ ] 实现 Image 记录构造
  - 组装图片字段（id, filename, image_data, thumbnail, vector, metadata）
  - 生成唯一 ID
  - 转换为 Arrow RecordBatch

- [ ] 实现批量插入
  - 收集多个记录后批量插入（提高性能）
  - 每 100 条记录或 5 秒间隔触发一次写入
  - 错误处理：部分失败不影响其他记录

- [ ] 实现增量更新
  - 检测文件修改时间变化
  - 更新现有记录（基于 ID）
  - 删除文件时同步删除 LanceDB 记录

- [ ] 实现事务保证
  - 确保 article 和关联的 images 原子性写入
  - 失败时回滚（删除部分写入的数据）

**文件位置**：
- `agent/src/db/writer.rs` - LanceDB 写入逻辑
- `agent/src/db/models.rs` - 数据模型定义

**写入流程**：
```
处理后的数据
  ↓
构造 RecordBatch
  ↓
批量插入 LanceDB
  ↓
更新本地索引缓存
  ↓
记录日志
```

---

### 任务 6: 配置管理与日志

**目标**：实现配置解析和日志系统

**子任务**：
- [ ] 定义 `config.toml` 结构
  ```toml
  [watch]
  content_dir = "/path/to/blog"
  image_dir = "/path/to/blog/images"

  [lancedb]
  db_path = "./data/lancedb"
  batch_size = 100
  batch_interval_secs = 5

  [ai]
  provider = "openai"  # or "ollama"
  api_key = "sk-xxx"
  model = "gpt-4o-mini"
  embedding_model = "text-embedding-3-small"

  [clip]
  service_url = "http://localhost:8000/embed"
  timeout_secs = 30

  [logging]
  level = "info"  # debug, info, warn, error
  file = "./logs/agent.log"
  ```

- [ ] 实现配置解析
  - 使用 `config` crate 解析 TOML
  - 支持环境变量覆盖（`STATIC_FLOW_AI_API_KEY`）
  - 验证配置完整性（必填字段检查）

- [ ] 实现日志系统
  - 使用 `tracing` + `tracing-subscriber`
  - 同时输出到控制台和文件
  - 不同模块使用不同日志级别

- [ ] 实现进度显示
  - 使用 `indicatif` 显示处理进度条
  - 实时显示：已处理/总数、速度、预计剩余时间

**文件位置**：
- `agent/src/config.rs` - 配置管理
- `agent/src/main.rs` - 日志初始化

**依赖**：
```toml
[dependencies]
config = "0.13"
tracing = "0.1"
tracing-subscriber = { version = "0.3", features = ["env-filter"] }
indicatif = "0.17"
```

---

### 任务 7: CLI 命令实现

**目标**：实现完整的 CLI 工具

**子任务**：
- [ ] `agent init` - 初始化 LanceDB 和配置
- [ ] `agent sync <path>` - 一次性同步目录
- [ ] `agent watch <path>` - 实时监控目录变化
- [ ] `agent status` - 查看数据库状态（文章数、图片数）
- [ ] `agent clear` - 清空数据库（危险操作，需确认）

**文件位置**：
- `agent/src/main.rs` - CLI 入口和命令路由
- `agent/src/commands/` - 各命令实现

**依赖**：
```toml
[dependencies]
clap = { version = "4.4", features = ["derive"] }
```

**CLI 使用示例**：
```bash
# 初始化
cargo run -- init

# 同步现有文件
cargo run -- sync ~/Documents/MyBlog

# 实时监控
cargo run --release -- watch ~/Documents/MyBlog

# 查看状态
cargo run -- status
```

---

### 任务 8: 测试与验证

**目标**：确保 Agent 功能正确性

**子任务**：
- [ ] 单元测试
  - Markdown 解析测试
  - Frontmatter 提取测试
  - 配置解析测试

- [ ] 集成测试
  - 创建测试 Markdown 文件
  - 运行 sync 命令
  - 验证 LanceDB 中的数据正确性
  - 验证 AI 生成的摘要、标签合理

- [ ] 性能测试
  - 测试处理 1000 篇文章的速度
  - 测试处理 500 张图片的速度
  - 验证内存占用 < 500MB

- [ ] 端到端测试
  - 本地创建新 Markdown 文件
  - Agent 自动检测并同步
  - Backend API 能正确读取数据
  - Frontend 能正确显示文章

**测试数据准备**：
```bash
# 创建测试数据目录
mkdir -p test_data/articles
mkdir -p test_data/images

# 生成测试文章
for i in {1..100}; do
  echo "---\ntitle: Test Article $i\n---\n\n# Content\n\nThis is test article $i." > test_data/articles/article_$i.md
done
```

---

## 开发顺序建议

1. **Week 1 - 基础设施**
   - 任务 1: LanceDB Schema 设计
   - 任务 6: 配置管理
   - 任务 7: CLI 框架

2. **Week 2 - 核心处理**
   - 任务 3: Markdown 处理
   - 任务 5: LanceDB 写入（无 AI，先用占位数据）

3. **Week 3 - AI 集成**
   - 任务 2: Rig 框架集成
   - 任务 4: 图片处理（先不做 CLIP，用占位向量）

4. **Week 4 - 完善与优化**
   - 完成 CLIP embedding
   - 任务 8: 测试与验证
   - 性能优化和 Bug 修复

---

## 技术难点与解决方案

### 难点 1: CLIP Embedding 生成
**问题**：Rust 生态缺少成熟的 CLIP 库

**解决方案**：
- 短期：使用独立的 Python FastAPI 服务
- 长期：考虑使用 `ort-rs`（ONNX Runtime Rust 绑定）+ CLIP ONNX 模型

### 难点 2: 大量图片的批量处理
**问题**：图片二进制占用大量内存

**解决方案**：
- 流式处理：一次只加载一张图片到内存
- 压缩策略：大于 5MB 的图片自动压缩
- 异步处理：使用 tokio 并发处理多张图片

### 难点 3: LLM API 调用失败
**问题**：网络不稳定或 API 限流

**解决方案**：
- 指数退避重试（1s → 2s → 4s）
- 本地缓存：已生成的摘要/标签持久化到磁盘
- 降级策略：LLM 失败时使用简单规则（如提取前 100 字作为摘要）

---

## 里程碑验证标准

### Milestone 1: 基础流程打通
- ✅ 能扫描并解析 Markdown 文件
- ✅ 能将数据写入 LanceDB
- ✅ 能通过 Python 查询到写入的数据

### Milestone 2: AI 功能完成
- ✅ LLM 能生成合理的摘要、标签、分类
- ✅ 图片能生成 CLIP embedding
- ✅ 向量搜索能返回相似结果

### Milestone 3: 生产就绪
- ✅ 处理 1000 篇文章耗时 < 10 分钟
- ✅ 文件监控能在 5 秒内同步变化
- ✅ 错误处理完善，不会因单个文件失败崩溃
- ✅ 日志清晰，便于问题排查

---

## 下一步行动

1. **立即开始**：任务 1（LanceDB Schema 设计）
2. **并行准备**：搭建 Python CLIP 服务（用于任务 4）
3. **技术调研**：深入学习 Rig 框架示例代码

**预计总工时**：80-100 小时（约 3-4 周全职开发）
