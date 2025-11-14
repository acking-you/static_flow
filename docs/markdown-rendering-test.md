# Markdown 渲染测试文档

本文档用于测试 StaticFlow 前端的 Markdown 渲染功能，包括代码语法高亮、数学公式和 Mermaid 图表支持。

## 目录

1. [代码语法高亮测试](#代码语法高亮测试)
2. [数学公式测试](#数学公式测试)
3. [Mermaid 图表测试](#mermaid-图表测试)
4. [混合内容测试](#混合内容测试)

---

## 代码语法高亮测试

### Rust 代码

```rust
use yew::prelude::*;

#[function_component(HelloWorld)]
pub fn hello_world() -> Html {
    let counter = use_state(|| 0);
    let increment = {
        let counter = counter.clone();
        Callback::from(move |_| counter.set(*counter + 1))
    };

    html! {
        <div>
            <h1>{ format!("Counter: {}", *counter) }</h1>
            <button onclick={increment}>{ "+1" }</button>
        </div>
    }
}
```

### JavaScript 代码

```javascript
function fibonacci(n) {
    if (n <= 1) return n;
    return fibonacci(n - 1) + fibonacci(n - 2);
}

const result = [1, 2, 3, 4, 5].map(n => fibonacci(n));
console.log(result); // [1, 1, 2, 3, 5]
```

### Python 代码

```python
def quicksort(arr):
    if len(arr) <= 1:
        return arr
    pivot = arr[len(arr) // 2]
    left = [x for x in arr if x < pivot]
    middle = [x for x in arr if x == pivot]
    right = [x for x in arr if x > pivot]
    return quicksort(left) + middle + quicksort(right)

print(quicksort([3, 6, 8, 10, 1, 2, 1]))
```

### Bash 脚本

```bash
#!/bin/bash
for i in {1..10}; do
    echo "Processing file $i..."
    if [ -f "input_$i.txt" ]; then
        cat "input_$i.txt" | grep "pattern" > "output_$i.txt"
    else
        echo "File not found!" >&2
    fi
done
```

### JSON 配置

```json
{
  "name": "static-flow-frontend",
  "version": "0.1.0",
  "dependencies": {
    "yew": "0.21",
    "wasm-bindgen": "0.2"
  },
  "devDependencies": {
    "trunk": "^0.20.0"
  }
}
```

### SQL 查询

```sql
SELECT
    u.username,
    COUNT(p.id) as post_count,
    MAX(p.created_at) as last_post
FROM users u
LEFT JOIN posts p ON u.id = p.user_id
WHERE u.is_active = true
GROUP BY u.id, u.username
HAVING post_count > 5
ORDER BY post_count DESC
LIMIT 10;
```

## 数学公式测试

### 行内公式

这是一个行内公式：$E = mc^2$，它描述了质能等价关系。

质量为 $m$ 的物体具有的能量为 $E$，其中 $c$ 是光速（约 $3 \times 10^8$ m/s）。

### 块级公式

二次方程的求根公式：

$$
x = \frac{-b \pm \sqrt{b^2 - 4ac}}{2a}
$$

傅里叶变换：

$$
F(\omega) = \int_{-\infty}^{\infty} f(t) e^{-i\omega t} dt
$$

矩阵乘法：

$$
\begin{bmatrix}
a & b \\
c & d
\end{bmatrix}
\begin{bmatrix}
x \\
y
\end{bmatrix}
=
\begin{bmatrix}
ax + by \\
cx + dy
\end{bmatrix}
$$

欧拉公式（数学中最美的公式之一）：

$$
e^{i\pi} + 1 = 0
$$

## 混合内容测试

### 示例：快速排序算法分析

快速排序的平均时间复杂度为 $O(n \log n)$，最坏情况为 $O(n^2)$。

**实现代码**：

```rust
fn quicksort<T: Ord>(arr: &mut [T]) {
    if arr.len() <= 1 {
        return;
    }
    let pivot = partition(arr);
    quicksort(&mut arr[0..pivot]);
    quicksort(&mut arr[pivot + 1..]);
}

fn partition<T: Ord>(arr: &mut [T]) -> usize {
    let pivot = arr.len() - 1;
    let mut i = 0;
    for j in 0..pivot {
        if arr[j] <= arr[pivot] {
            arr.swap(i, j);
            i += 1;
        }
    }
    arr.swap(i, pivot);
    i
}
```

**时间复杂度分析**：

设 $T(n)$ 为排序 $n$ 个元素的时间，分区操作需要 $O(n)$ 时间。

- 最好情况：每次都平分数组
  $$
  T(n) = 2T(n/2) + O(n) = O(n \log n)
  $$

- 最坏情况：每次只分出一个元素
  $$
  T(n) = T(n-1) + O(n) = O(n^2)
  $$

## 特殊符号测试

希腊字母：$\alpha, \beta, \gamma, \Delta, \Sigma, \Omega$

数学符号：$\sum_{i=1}^{n} i = \frac{n(n+1)}{2}$

积分：$\int_0^1 x^2 dx = \frac{1}{3}$

极限：$\lim_{x \to \infty} \frac{1}{x} = 0$

偏导数：$\frac{\partial f}{\partial x}$

向量：$\vec{v} = \langle x, y, z \rangle$

## 渲染验证清单

- [ ] Rust 代码块有语法高亮
- [ ] JavaScript 代码块有语法高亮
- [ ] Python 代码块有语法高亮
- [ ] Bash 代码块有语法高亮
- [ ] JSON 代码块有语法高亮
- [ ] SQL 代码块有语法高亮
- [ ] 行内公式 $E = mc^2$ 正确渲染
- [ ] 块级公式正确渲染且居中
- [ ] 复杂公式（矩阵、积分、求和）正确渲染
- [ ] 代码和公式混排正常显示

---

**注意**：如果公式或代码高亮未生效，请检查浏览器控制台是否有 JavaScript 错误。

---

## Mermaid 图表测试

### 流程图 (Flowchart)

```mermaid
graph TD
    A[开始] --> B{是否登录?}
    B -->|是| C[显示首页]
    B -->|否| D[跳转登录页]
    D --> E[输入账号密码]
    E --> F{验证成功?}
    F -->|是| C
    F -->|否| G[显示错误提示]
    G --> E
    C --> H[结束]
```

### 时序图 (Sequence Diagram)

```mermaid
sequenceDiagram
    participant U as 用户
    participant F as 前端
    participant B as 后端
    participant DB as 数据库

    U->>F: 发送登录请求
    F->>B: POST /api/login
    B->>DB: 查询用户信息
    DB-->>B: 返回用户数据
    B-->>F: 返回 JWT Token
    F-->>U: 跳转到首页
    
    Note over U,F: 用户已登录
    
    U->>F: 请求文章列表
    F->>B: GET /api/articles
    B->>DB: 查询文章
    DB-->>B: 返回文章列表
    B-->>F: JSON 数据
    F-->>U: 渲染文章列表
```

### 类图 (Class Diagram)

```mermaid
classDiagram
    class Article {
        +String id
        +String title
        +String content
        +Vec~String~ tags
        +String category
        +DateTime created_at
        +render() Html
        +to_json() String
    }
    
    class ArticleListItem {
        +String id
        +String title
        +String summary
        +Vec~String~ tags
        +from(Article) ArticleListItem
    }
    
    class Tag {
        +String name
        +String slug
        +count() usize
    }
    
    Article "1" --> "*" Tag : has
    ArticleListItem <|-- Article : derives from
```

### 状态图 (State Diagram)

```mermaid
stateDiagram-v2
    [*] --> Draft
    Draft --> Review: 提交审核
    Review --> Published: 审核通过
    Review --> Draft: 打回修改
    Published --> Archived: 归档
    Published --> Draft: 撤回编辑
    Archived --> [*]
    
    Draft: 草稿状态
    Review: 审核中
    Published: 已发布
    Archived: 已归档
```

### 甘特图 (Gantt Chart)

```mermaid
gantt
    title StaticFlow 开发计划
    dateFormat  YYYY-MM-DD
    section Week 1 前端
    初始化项目           :done, w1-1, 2024-11-01, 1d
    复刻老博客界面       :done, w1-2, after w1-1, 3d
    实现路由和组件       :done, w1-3, after w1-2, 2d
    Markdown渲染测试     :active, w1-4, after w1-3, 1d
    
    section Week 2 后端
    Axum API 开发        :w2-1, 2024-11-08, 3d
    Meilisearch 集成     :w2-2, after w2-1, 2d
    CLI 工具开发         :w2-3, after w2-2, 2d
    
    section Week 3 集成
    前后端对接           :w3-1, 2024-11-15, 2d
    AI 自动化功能        :w3-2, after w3-1, 3d
    测试和优化           :w3-3, after w3-2, 2d
```

### 饼图 (Pie Chart)

```mermaid
pie title 文章分类统计
    "Rust" : 35
    "Web" : 25
    "DevOps" : 15
    "AI" : 15
    "Productivity" : 10
```

### ER 图 (Entity Relationship)

```mermaid
erDiagram
    USER ||--o{ ARTICLE : writes
    USER {
        string id PK
        string username
        string email
        datetime created_at
    }
    ARTICLE ||--|{ TAG : has
    ARTICLE {
        string id PK
        string title
        string content
        string user_id FK
        datetime published_at
    }
    TAG {
        string id PK
        string name
        string slug
    }
    ARTICLE }o--|| CATEGORY : belongs_to
    CATEGORY {
        string id PK
        string name
        string description
    }
```

### Git 分支图 (Git Graph)

```mermaid
gitGraph
    commit id: "init project"
    commit id: "add frontend"
    branch feature/markdown
    checkout feature/markdown
    commit id: "add KaTeX"
    commit id: "add highlight.js"
    checkout main
    branch feature/mermaid
    checkout feature/mermaid
    commit id: "add mermaid support"
    checkout main
    merge feature/markdown
    merge feature/mermaid
    commit id: "update docs"
```

### 思维导图 (Mindmap)

```mermaid
mindmap
  root((StaticFlow))
    Frontend
      Yew Framework
      WebAssembly
      Trunk Builder
      Markdown Rendering
        Syntax Highlighting
        Math Formulas
        Mermaid Diagrams
    Backend
      Axum Server
      SQLite Database
      Meilisearch
    CLI Tool
      File Watcher
      Content Processor
      AI Integration
    Deployment
      Static Hosting
      Docker
      CI/CD
```

### 旅程图 (User Journey)

```mermaid
journey
    title 用户使用 StaticFlow 的旅程
    section 本地写作
      打开 Obsidian: 5: 用户
      编写 Markdown 文章: 5: 用户
      添加图片和代码块: 4: 用户
    section 自动同步
      CLI 监听文件变化: 5: CLI
      提取元数据: 4: CLI
      AI 生成摘要: 5: AI
      推送到后端: 5: CLI
    section 在线浏览
      访问博客首页: 5: 用户
      搜索文章: 5: 用户
      阅读文章内容: 5: 用户
      查看代码高亮: 5: 用户
```


## 渲染验证清单

### 代码高亮
- [ ] Rust 代码块有语法高亮
- [ ] JavaScript 代码块有语法高亮
- [ ] Python 代码块有语法高亮
- [ ] Bash 代码块有语法高亮
- [ ] JSON 代码块有语法高亮
- [ ] SQL 代码块有语法高亮

### 数学公式
- [ ] 行内公式 $E = mc^2$ 正确渲染
- [ ] 块级公式正确渲染且居中
- [ ] 复杂公式（矩阵、积分、求和）正确渲染
- [ ] 代码和公式混排正常显示

### Mermaid 图表
- [ ] 流程图 (Flowchart) 正确渲染
- [ ] 时序图 (Sequence Diagram) 正确渲染
- [ ] 类图 (Class Diagram) 正确渲染
- [ ] 状态图 (State Diagram) 正确渲染
- [ ] 甘特图 (Gantt Chart) 正确渲染
- [ ] 饼图 (Pie Chart) 正确渲染
- [ ] ER 图 (Entity Relationship) 正确渲染
- [ ] Git 分支图 (Git Graph) 正确渲染
- [ ] 思维导图 (Mindmap) 正确渲染
- [ ] 旅程图 (User Journey) 正确渲染

### 主题适配
- [ ] 切换到暗色主题后，Mermaid 图表自动使用暗色主题
- [ ] 切换到亮色主题后，Mermaid 图表自动使用亮色主题
- [ ] 代码高亮主题跟随系统主题切换

---

**调试提示**：
- 如果 Mermaid 图表未渲染，打开浏览器控制台检查是否有错误
- 确保 `window.mermaid` 已定义
- 检查 `.mermaid` 类的 div 元素是否正确生成
- 如果公式或代码高亮未生效，刷新页面（Ctrl+F5）清除缓存
