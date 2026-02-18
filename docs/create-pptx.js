const pptxgen = require("pptxgenjs");
const pres = new pptxgen();

pres.layout = "LAYOUT_16x9";
pres.author = "LB7666";
pres.title = "AI 如何改变了我的学习方式";

// === Color Palette: Midnight Tech ===
const C = {
  bg: "0F172A", bgCard: "1E293B", bgCardLight: "334155",
  cyan: "06B6D4", amber: "F59E0B", emerald: "10B981", rose: "F43F5E",
  textPrimary: "F8FAFC", textSecondary: "94A3B8", textMuted: "64748B",
  white: "FFFFFF", border: "334155",
};
const FONT_H = "Consolas";
const FONT_B = "Calibri";

// Helper: fresh shadow object (avoid PptxGenJS mutation bug)
const mkShadow = () => ({ type: "outer", blur: 8, offset: 3, angle: 135, color: "000000", opacity: 0.3 });

// ============================================================
// SLIDE 1: Title
// ============================================================
let s1 = pres.addSlide();
s1.background = { color: C.bg };
// Top accent bar
s1.addShape(pres.shapes.RECTANGLE, { x: 0, y: 0, w: 10, h: 0.06, fill: { color: C.cyan } });
// Main title
s1.addText("AI 如何改变了我的学习方式", {
  x: 0.8, y: 1.2, w: 6.2, h: 1.0, fontSize: 32, fontFace: FONT_H,
  color: C.white, bold: true, align: "left", margin: 0,
});
// Subtitle
s1.addText("一个 Skill 驱动、本地优先的知识管理实践", {
  x: 0.8, y: 2.3, w: 6.2, h: 0.6, fontSize: 18, fontFace: FONT_B,
  color: C.cyan, align: "left", margin: 0,
});
// Three narrative lines
s1.addText([
  { text: "主线：AI 改变学习", options: { color: C.amber, breakLine: true } },
  { text: "副线：Skills > Agents", options: { color: C.emerald, breakLine: true } },
  { text: "暗线：本地优先", options: { color: C.textSecondary } },
], { x: 0.8, y: 3.6, w: 5, h: 1.4, fontSize: 14, fontFace: FONT_B, margin: 0 });
// Author + date
s1.addText("LB7666  |  2025-2026", {
  x: 0.8, y: 4.8, w: 4, h: 0.4, fontSize: 12, fontFace: FONT_B,
  color: C.textMuted, align: "left", margin: 0,
});
// Right side decorative block
s1.addShape(pres.shapes.RECTANGLE, {
  x: 7.2, y: 1.0, w: 2.4, h: 3.8, fill: { color: C.bgCard }, shadow: mkShadow(),
});
// Decorative cyan dot
s1.addShape(pres.shapes.OVAL, { x: 7.5, y: 1.3, w: 0.3, h: 0.3, fill: { color: C.cyan } });
s1.addShape(pres.shapes.OVAL, { x: 8.0, y: 1.3, w: 0.3, h: 0.3, fill: { color: C.amber } });
s1.addShape(pres.shapes.OVAL, { x: 8.5, y: 1.3, w: 0.3, h: 0.3, fill: { color: C.emerald } });
// Code-like text in decorative block
s1.addText([
  { text: "> AI + Skills", options: { color: C.cyan, breakLine: true, fontSize: 11 } },
  { text: "> Local-First", options: { color: C.emerald, breakLine: true, fontSize: 11 } },
  { text: "> LanceDB", options: { color: C.amber, breakLine: true, fontSize: 11 } },
  { text: "> Rust Full-Stack", options: { color: C.textSecondary, fontSize: 11 } },
], { x: 7.4, y: 1.9, w: 2.0, h: 2.5, fontFace: FONT_H, margin: 0 });

// Helper: section divider slide
function addSectionSlide(actLabel, title, subtitle) {
  let s = pres.addSlide();
  s.background = { color: C.bgCard };
  s.addShape(pres.shapes.RECTANGLE, { x: 0, y: 0, w: 10, h: 0.06, fill: { color: C.cyan } });
  s.addText(actLabel, {
    x: 0.8, y: 1.5, w: 8.4, h: 0.5, fontSize: 14, fontFace: FONT_H,
    color: C.cyan, align: "left", margin: 0, charSpacing: 4,
  });
  s.addText(title, {
    x: 0.8, y: 2.1, w: 8.4, h: 1.2, fontSize: 36, fontFace: FONT_H,
    color: C.white, bold: true, align: "left", margin: 0,
  });
  if (subtitle) {
    s.addText(subtitle, {
      x: 0.8, y: 3.4, w: 8.4, h: 0.6, fontSize: 16, fontFace: FONT_B,
      color: C.textSecondary, align: "left", margin: 0,
    });
  }
  return s;
}

// Helper: content slide with title
function addContentSlide(title) {
  let s = pres.addSlide();
  s.background = { color: C.bg };
  s.addShape(pres.shapes.RECTANGLE, { x: 0, y: 0, w: 10, h: 0.06, fill: { color: C.cyan } });
  s.addText(title, {
    x: 0.8, y: 0.3, w: 8.4, h: 0.7, fontSize: 24, fontFace: FONT_H,
    color: C.white, bold: true, align: "left", margin: 0,
  });
  // Thin separator
  s.addShape(pres.shapes.RECTANGLE, { x: 0.8, y: 1.05, w: 1.5, h: 0.04, fill: { color: C.cyan } });
  return s;
}

// ============================================================
// SLIDE 2: Section - Act 1 Opening
// ============================================================
addSectionSlide("第一幕", "开场：AI 编程元年", "2025-2026，模型能力从「补全代码」跳到「理解项目」");

// ============================================================
// SLIDE 3: AI Evolution Timeline
// ============================================================
let s3 = addContentSlide("AI 编程工具井喷式爆发");
// Left column: Models
s3.addShape(pres.shapes.RECTANGLE, {
  x: 0.5, y: 1.3, w: 4.3, h: 2.0, fill: { color: C.bgCard }, shadow: mkShadow(),
});
s3.addText("模型迭代", {
  x: 0.7, y: 1.4, w: 3.9, h: 0.4, fontSize: 16, fontFace: FONT_H,
  color: C.cyan, bold: true, margin: 0,
});
s3.addText([
  { text: "Claude  3.5 → 4.5/4.6 Opus", options: { breakLine: true, color: C.amber } },
  { text: "GPT  4o → o1 → 5.1/5.2/5.3 Codex", options: { breakLine: true, color: C.emerald } },
], { x: 0.7, y: 1.85, w: 3.9, h: 1.2, fontSize: 14, fontFace: FONT_H, margin: 0 });
// Right column: Tools
s3.addShape(pres.shapes.RECTANGLE, {
  x: 5.2, y: 1.3, w: 4.3, h: 2.0, fill: { color: C.bgCard }, shadow: mkShadow(),
});
s3.addText("工具爆发", {
  x: 5.4, y: 1.4, w: 3.9, h: 0.4, fontSize: 16, fontFace: FONT_H,
  color: C.cyan, bold: true, margin: 0,
});
s3.addText([
  { text: "Cursor / Windsurf / Kiro", options: { breakLine: true, color: C.textPrimary } },
  { text: "Claude Code / Codex CLI", options: { breakLine: true, color: C.textPrimary } },
  { text: "MCP 生态 / Skills 工作流", options: { breakLine: true, color: C.textPrimary } },
], { x: 5.4, y: 1.85, w: 3.9, h: 1.2, fontSize: 14, fontFace: FONT_H, margin: 0 });
// Bottom highlight
s3.addShape(pres.shapes.RECTANGLE, {
  x: 2.5, y: 3.6, w: 5, h: 0.6, fill: { color: C.bgCardLight },
});
s3.addText("「2025-2026：AI 编程元年」", {
  x: 2.5, y: 3.6, w: 5, h: 0.6, fontSize: 16, fontFace: FONT_H,
  color: C.amber, align: "center", valign: "middle", margin: 0,
});

// ============================================================
// SLIDE 4: Learning Paradigm Shift
// ============================================================
let s4 = addContentSlide("学习方式——彻底变了");
// Before card
s4.addShape(pres.shapes.RECTANGLE, {
  x: 0.5, y: 1.3, w: 4.3, h: 2.0, fill: { color: C.bgCard }, shadow: mkShadow(),
});
s4.addText("以前", {
  x: 0.7, y: 1.4, w: 1.5, h: 0.45, fontSize: 14, fontFace: FONT_H,
  color: C.rose, bold: true, margin: 0,
});
s4.addText("找教程 → 抄例子 → 踩坑 → 再找教程 → 再踩坑", {
  x: 0.7, y: 1.95, w: 3.9, h: 1.1, fontSize: 14, fontFace: FONT_B,
  color: C.textSecondary, margin: 0,
});
// After card
s4.addShape(pres.shapes.RECTANGLE, {
  x: 5.2, y: 1.3, w: 4.3, h: 2.0, fill: { color: C.bgCard }, shadow: mkShadow(),
});
s4.addText("现在", {
  x: 5.4, y: 1.4, w: 1.5, h: 0.45, fontSize: 14, fontFace: FONT_H,
  color: C.emerald, bold: true, margin: 0,
});
s4.addText("跟 AI 对话 → 理解原理 → 沉淀成文档 → 存进知识库", {
  x: 5.4, y: 1.95, w: 3.9, h: 1.1, fontSize: 14, fontFace: FONT_B,
  color: C.textPrimary, margin: 0,
});
// Bottom insight
s4.addShape(pres.shapes.RECTANGLE, {
  x: 0.5, y: 3.7, w: 9.0, h: 1.4, fill: { color: C.bgCardLight },
});
s4.addText("每一次学习，都变成了知识库里可检索、可复用的资产", {
  x: 0.7, y: 3.85, w: 8.6, h: 1.0, fontSize: 18, fontFace: FONT_B,
  color: C.amber, align: "center", valign: "middle", margin: 0,
});

// ============================================================
// SLIDE 5: Core Quote
// ============================================================
let s5 = pres.addSlide();
s5.background = { color: C.bg };
// Large quote block
s5.addShape(pres.shapes.RECTANGLE, {
  x: 1.0, y: 1.0, w: 8.0, h: 3.6, fill: { color: C.bgCard }, shadow: mkShadow(),
});
// Left accent bar
s5.addShape(pres.shapes.RECTANGLE, {
  x: 1.0, y: 1.0, w: 0.08, h: 3.6, fill: { color: C.cyan },
});
// Quote mark
s5.addText('"', {
  x: 1.4, y: 0.9, w: 1, h: 1.2, fontSize: 72, fontFace: "Georgia",
  color: C.cyan, margin: 0,
});
s5.addText("Don't build agents,\nbuild skills instead.", {
  x: 1.8, y: 1.8, w: 6.5, h: 1.4, fontSize: 32, fontFace: FONT_H,
  color: C.white, bold: true, margin: 0,
});
s5.addText("— Anthropic Expert Talk", {
  x: 1.8, y: 3.3, w: 6.5, h: 0.5, fontSize: 14, fontFace: FONT_B,
  color: C.textMuted, margin: 0, italic: true,
});
s5.addText("不要造 Agent，要造 Skill。这句话直接影响了整个架构决策。", {
  x: 1.0, y: 4.8, w: 8.0, h: 0.5, fontSize: 14, fontFace: FONT_B,
  color: C.textSecondary, align: "center", margin: 0,
});

// ============================================================
// SLIDE 6: Section - Act 2 StaticFlow
// ============================================================
addSectionSlide("第二幕", "StaticFlow 项目介绍", "本地优先的写作和知识管理系统");

// ============================================================
// SLIDE 7: Project Structure
// ============================================================
let s7 = addContentSlide("项目结构一览");
// Directory tree card
s7.addShape(pres.shapes.RECTANGLE, {
  x: 0.5, y: 1.3, w: 4.5, h: 3.8, fill: { color: C.bgCard }, shadow: mkShadow(),
});
const dirs = [
  { name: "frontend/", desc: "Yew WASM 前端", col: C.cyan },
  { name: "backend/", desc: "Axum 后端 + LanceDB", col: C.emerald },
  { name: "shared/", desc: "前后端共享类型", col: C.amber },
  { name: "cli/", desc: "sf-cli · Agent 操作接口", col: C.textPrimary },
  { name: "skills/", desc: "7 个 AI Skills", col: C.rose },
  { name: "docs/", desc: "20+ 篇技术文档", col: C.textSecondary },
];
dirs.forEach((d, i) => {
  s7.addText(d.name, {
    x: 0.7, y: 1.5 + i * 0.55, w: 1.8, h: 0.45, fontSize: 13, fontFace: FONT_H,
    color: d.col, margin: 0,
  });
  s7.addText(d.desc, {
    x: 2.5, y: 1.5 + i * 0.55, w: 2.3, h: 0.45, fontSize: 12, fontFace: FONT_B,
    color: C.textSecondary, margin: 0,
  });
});
// Right side: key stats
s7.addShape(pres.shapes.RECTANGLE, {
  x: 5.3, y: 1.3, w: 4.2, h: 3.8, fill: { color: C.bgCard }, shadow: mkShadow(),
});
s7.addText("核心定位", {
  x: 5.5, y: 1.45, w: 3.8, h: 0.5, fontSize: 16, fontFace: FONT_H,
  color: C.cyan, bold: true, margin: 0,
});
s7.addText([
  { text: "AI 对话 + Skill 生成笔记", options: { breakLine: true, color: C.textPrimary } },
  { text: "Agent 通过 CLI 操作 LanceDB", options: { breakLine: true, color: C.textPrimary } },
  { text: "前端从 LanceDB 查询展示", options: { breakLine: true, color: C.textPrimary } },
  { text: "所有数据都在本地", options: { color: C.amber, bold: true } },
], { x: 5.5, y: 2.1, w: 3.8, h: 2.5, fontSize: 14, fontFace: FONT_B, margin: 0 });
// Bottom: language badge
s7.addShape(pres.shapes.RECTANGLE, {
  x: 3.5, y: 5.15, w: 3, h: 0.35, fill: { color: C.bgCardLight },
});
s7.addText("全部用 Rust 编写", {
  x: 3.5, y: 5.15, w: 3, h: 0.35, fontSize: 12, fontFace: FONT_H,
  color: C.amber, align: "center", valign: "middle", margin: 0,
});

// ============================================================
// SLIDE 8: Tech Stack
// ============================================================
let s8 = addContentSlide("技术栈选型");
const techItems = [
  { label: "前端", tech: "Yew → WebAssembly", detail: "Rust 编译到 WASM，浏览器原生运行", col: C.cyan },
  { label: "后端", tech: "Axum + Tokio", detail: "异步运行时，高性能 HTTP 服务", col: C.emerald },
  { label: "数据库", tech: "LanceDB (嵌入式)", detail: "无需额外进程，直接读写本地文件", col: C.amber },
  { label: "共享", tech: "shared crate", detail: "类型定义写一次，前后端共用，编译器检查", col: C.rose },
];
techItems.forEach((item, i) => {
  const yPos = 1.3 + i * 1.0;
  s8.addShape(pres.shapes.RECTANGLE, {
    x: 0.5, y: yPos, w: 9.0, h: 0.85, fill: { color: C.bgCard }, shadow: mkShadow(),
  });
  s8.addShape(pres.shapes.RECTANGLE, {
    x: 0.5, y: yPos, w: 0.06, h: 0.85, fill: { color: item.col },
  });
  s8.addText(item.label, {
    x: 0.8, y: yPos + 0.05, w: 1.0, h: 0.35, fontSize: 12, fontFace: FONT_H,
    color: item.col, bold: true, margin: 0,
  });
  s8.addText(item.tech, {
    x: 1.9, y: yPos + 0.05, w: 3.0, h: 0.35, fontSize: 14, fontFace: FONT_H,
    color: C.white, bold: true, margin: 0,
  });
  s8.addText(item.detail, {
    x: 1.9, y: yPos + 0.42, w: 6.5, h: 0.35, fontSize: 12, fontFace: FONT_B,
    color: C.textSecondary, margin: 0,
  });
});

// ============================================================
// SLIDE 9: Deployment Architecture
// ============================================================
let s9 = addContentSlide("部署架构");
const archBoxes = [
  { label: "GitHub Pages", sub: "前端静态文件", y: 1.25, col: C.cyan },
  { label: "Cloud Nginx :443", sub: "TLS 终止 + 路径过滤", y: 2.25, col: C.emerald },
  { label: "pb-mapper tunnel", sub: "本地 ↔ 云端 TCP 隧道", y: 3.25, col: C.amber },
  { label: "Local Backend + LanceDB", sub: "Axum :3000 · 数据永远在本地", y: 4.25, col: C.rose },
];
archBoxes.forEach((box) => {
  s9.addShape(pres.shapes.RECTANGLE, {
    x: 1.0, y: box.y, w: 4.5, h: 0.8, fill: { color: C.bgCard }, shadow: mkShadow(),
  });
  s9.addShape(pres.shapes.RECTANGLE, {
    x: 1.0, y: box.y, w: 0.06, h: 0.8, fill: { color: box.col },
  });
  s9.addText(box.label, {
    x: 1.3, y: box.y + 0.05, w: 4.0, h: 0.4, fontSize: 14, fontFace: FONT_H,
    color: C.white, bold: true, margin: 0,
  });
  s9.addText(box.sub, {
    x: 1.3, y: box.y + 0.42, w: 4.0, h: 0.3, fontSize: 11, fontFace: FONT_B,
    color: C.textSecondary, margin: 0,
  });
});
// Arrows
[1.25, 2.25, 3.25].forEach((y) => {
  s9.addText("▼", {
    x: 2.9, y: y + 0.78, w: 0.7, h: 0.45, fontSize: 16, fontFace: FONT_B,
    color: C.textMuted, align: "center", valign: "middle", margin: 0,
  });
});
// Right side: key points
s9.addShape(pres.shapes.RECTANGLE, {
  x: 6.0, y: 1.25, w: 3.5, h: 3.8, fill: { color: C.bgCard }, shadow: mkShadow(),
});
s9.addText("架构亮点", {
  x: 6.2, y: 1.4, w: 3.1, h: 0.5, fontSize: 16, fontFace: FONT_H,
  color: C.cyan, bold: true, margin: 0,
});
s9.addText([
  { text: "Admin 接口仅本地可访问", options: { breakLine: true, color: C.textPrimary, fontSize: 13 } },
  { text: "网络层隔离，无需认证代码", options: { breakLine: true, color: C.textSecondary, fontSize: 12 } },
  { text: "", options: { breakLine: true, fontSize: 8 } },
  { text: "数据备份：Git Xet", options: { breakLine: true, color: C.textPrimary, fontSize: 13 } },
  { text: "LanceDB → HuggingFace Datasets", options: { breakLine: true, color: C.textSecondary, fontSize: 12 } },
  { text: "Git 管理数据库快照", options: { color: C.textSecondary, fontSize: 12 } },
], { x: 6.2, y: 2.0, w: 3.1, h: 2.8, fontFace: FONT_B, margin: 0 });

// ============================================================
// SLIDE 10: Section - Act 3 Features
// ============================================================
addSectionSlide("第三幕", "功能演示走查", "7 个核心功能，每个 ≤75 秒");

// ============================================================
// SLIDE 11: Hybrid Search
// ============================================================
let s11 = addContentSlide("功能 1：混合搜索");
// Search diagram
s11.addShape(pres.shapes.RECTANGLE, {
  x: 0.5, y: 1.3, w: 9.0, h: 1.6, fill: { color: C.bgCard }, shadow: mkShadow(),
});
// Three search components
const searchParts = [
  { label: "FTS 全文检索", x: 0.7, col: C.cyan },
  { label: "+", x: 3.3, col: C.textMuted },
  { label: "向量语义检索", x: 3.8, col: C.emerald },
  { label: "→", x: 6.7, col: C.textMuted },
  { label: "RRF 融合排序", x: 7.2, col: C.amber },
];
searchParts.forEach((p) => {
  s11.addText(p.label, {
    x: p.x, y: 1.7, w: 2.5, h: 0.6, fontSize: 16, fontFace: FONT_H,
    color: p.col, bold: true, margin: 0, valign: "middle",
  });
});
// Cross-language demo → FTS vs Vector vs Hybrid
s11.addShape(pres.shapes.RECTANGLE, {
  x: 0.5, y: 3.2, w: 4.3, h: 1.8, fill: { color: C.bgCard }, shadow: mkShadow(),
});
s11.addText("FTS 全文检索「网络协议」", {
  x: 0.7, y: 3.3, w: 3.9, h: 0.45, fontSize: 14, fontFace: FONT_H,
  color: C.cyan, margin: 0,
});
s11.addText("→ 命中 1 篇直接包含关键词的文章", {
  x: 0.7, y: 3.8, w: 3.9, h: 0.5, fontSize: 12, fontFace: FONT_B,
  color: C.textSecondary, margin: 0,
});
s11.addShape(pres.shapes.RECTANGLE, {
  x: 5.2, y: 3.2, w: 4.3, h: 1.8, fill: { color: C.bgCard }, shadow: mkShadow(),
});
s11.addText("向量语义检索「网络协议」", {
  x: 5.4, y: 3.3, w: 3.9, h: 0.45, fontSize: 14, fontFace: FONT_H,
  color: C.emerald, margin: 0,
});
s11.addText("→ 命中多篇：HTTP 解析、TCP 实现…", {
  x: 5.4, y: 3.8, w: 3.9, h: 0.5, fontSize: 12, fontFace: FONT_B,
  color: C.textSecondary, margin: 0,
});
// Bottom note
s11.addText("混合检索：内容直接命中排前，语义相关补充在后", {
  x: 0.5, y: 5.1, w: 9.0, h: 0.4, fontSize: 12, fontFace: FONT_B,
  color: C.amber, align: "center", margin: 0,
});

// ============================================================
// SLIDE 12: AI Comment System + SVG Charts
// ============================================================
let s12 = addContentSlide("功能亮点：AI 评论 & Skill 工作流");
// Left: Comment system
s12.addShape(pres.shapes.RECTANGLE, {
  x: 0.5, y: 1.3, w: 4.3, h: 3.8, fill: { color: C.bgCard }, shadow: mkShadow(),
});
s12.addText("AI 评论回复系统", {
  x: 0.7, y: 1.45, w: 3.9, h: 0.5, fontSize: 16, fontFace: FONT_H,
  color: C.cyan, bold: true, margin: 0,
});
s12.addText([
  { text: "选中文本 → 精确评论", options: { breakLine: true, bullet: true } },
  { text: "提交 → LanceDB 任务队列", options: { breakLine: true, bullet: true } },
  { text: "Admin 审核 → Codex Worker", options: { breakLine: true, bullet: true } },
  { text: "SSE 实时推送 AI 回复", options: { breakLine: true, bullet: true } },
  { text: "SHA256 指纹，零个人信息", options: { bullet: true } },
], {
  x: 0.7, y: 2.1, w: 3.9, h: 2.8, fontSize: 13, fontFace: FONT_B,
  color: C.textSecondary, margin: 0,
});
// Right: Skill Workflow
s12.addShape(pres.shapes.RECTANGLE, {
  x: 5.2, y: 1.3, w: 4.3, h: 3.8, fill: { color: C.bgCard }, shadow: mkShadow(),
});
s12.addText("Skill 工作流打通", {
  x: 5.4, y: 1.45, w: 3.9, h: 0.5, fontSize: 16, fontFace: FONT_H,
  color: C.amber, bold: true, margin: 0,
});
s12.addText([
  { text: "7 个 Skill 串联完整工作流", options: { breakLine: true, bullet: true } },
  { text: "对话 → 生成 → 翻译 → 发布", options: { breakLine: true, bullet: true } },
  { text: "全链路 Agent 自动化", options: { breakLine: true, bullet: true } },
  { text: "Markdown 文件即 Agent 大脑", options: { bullet: true } },
], {
  x: 5.4, y: 2.1, w: 3.9, h: 2.8, fontSize: 13, fontFace: FONT_B,
  color: C.textSecondary, margin: 0,
});

// ============================================================
// SLIDE 13: Bilingual + sf-cli
// ============================================================
let s13 = addContentSlide("双语翻译 & Agent CLI 接口");
// Left: Bilingual
s13.addShape(pres.shapes.RECTANGLE, {
  x: 0.5, y: 1.3, w: 4.3, h: 2.0, fill: { color: C.bgCard }, shadow: mkShadow(),
});
s13.addText("双语 + Mermaid", {
  x: 0.7, y: 1.45, w: 3.9, h: 0.5, fontSize: 16, fontFace: FONT_H,
  color: C.emerald, bold: true, margin: 0,
});
s13.addText([
  { text: "AI 整篇理解后重写英文版", options: { breakLine: true, bullet: true } },
  { text: "Mermaid 全屏 / 下载 / 复制", options: { breakLine: true, bullet: true } },
  { text: "超 15 行代码块自动折叠", options: { bullet: true } },
], {
  x: 0.7, y: 2.05, w: 3.9, h: 1.1, fontSize: 13, fontFace: FONT_B,
  color: C.textSecondary, margin: 0,
});
// Right: sf-cli
s13.addShape(pres.shapes.RECTANGLE, {
  x: 5.2, y: 1.3, w: 4.3, h: 2.0, fill: { color: C.bgCard }, shadow: mkShadow(),
});
s13.addText("sf-cli：Agent 操作接口", {
  x: 5.4, y: 1.45, w: 3.9, h: 0.5, fontSize: 16, fontFace: FONT_H,
  color: C.cyan, bold: true, margin: 0,
});
s13.addText([
  { text: "Coding Agent 一键调用", options: { breakLine: true, bullet: true } },
  { text: "写入 / 查询 / 索引 / 同步", options: { breakLine: true, bullet: true } },
  { text: "Skill 通过 CLI 操作 LanceDB", options: { bullet: true } },
], {
  x: 5.4, y: 2.05, w: 3.9, h: 1.1, fontSize: 13, fontFace: FONT_B,
  color: C.textSecondary, margin: 0,
});
// Bottom: CLI command example (Agent 自动执行)
s13.addShape(pres.shapes.RECTANGLE, {
  x: 0.5, y: 3.6, w: 9.0, h: 1.5, fill: { color: "0D1117" }, shadow: mkShadow(),
});
s13.addText([
  { text: "# Agent 通过 Skill 自动调用：", options: { breakLine: true, color: C.textMuted } },
  { text: "$ sf-cli sync-notes --db-path .../lancedb \\", options: { breakLine: true, color: C.emerald } },
  { text: "    --dir ./content --recursive --generate-thumbnail", options: { color: C.textSecondary } },
], {
  x: 0.7, y: 3.75, w: 8.6, h: 1.2, fontSize: 12, fontFace: FONT_H, margin: 0,
});

// ============================================================
// SLIDE 14: API Monitoring + 7 Skills
// ============================================================
let s14 = addContentSlide("功能 6-7：行为监控 & 7 个 AI Skills");
// Left: API monitoring
s14.addShape(pres.shapes.RECTANGLE, {
  x: 0.5, y: 1.3, w: 4.3, h: 2.2, fill: { color: C.bgCard }, shadow: mkShadow(),
});
s14.addText("API 行为监控", {
  x: 0.7, y: 1.45, w: 3.9, h: 0.5, fontSize: 16, fontFace: FONT_H,
  color: C.rose, bold: true, margin: 0,
});
s14.addText([
  { text: "状态码 / 响应耗时 / 设备类型", options: { breakLine: true, bullet: true } },
  { text: "浏览器 / OS / IP 地区", options: { breakLine: true, bullet: true } },
  { text: "LanceDB 列存，聚合查询快", options: { bullet: true } },
], {
  x: 0.7, y: 2.1, w: 3.9, h: 1.2, fontSize: 13, fontFace: FONT_B,
  color: C.textSecondary, margin: 0,
});
// Right: 7 Skills
s14.addShape(pres.shapes.RECTANGLE, {
  x: 5.2, y: 1.3, w: 4.3, h: 3.8, fill: { color: C.bgCard }, shadow: mkShadow(),
});
s14.addText("7 个 AI Skills", {
  x: 5.4, y: 1.45, w: 3.9, h: 0.5, fontSize: 16, fontFace: FONT_H,
  color: C.cyan, bold: true, margin: 0,
});
const skills = [
  { name: "cli-publisher", desc: "发布文章到 LanceDB" },
  { name: "bilingual-translation", desc: "中译英 + 双语摘要" },
  { name: "summary-architect", desc: "结构化摘要生成" },
  { name: "comment-responder", desc: "AI 评论回复" },
  { name: "deep-dive-writer", desc: "技术深度文档" },
  { name: "git-xet-publisher", desc: "数据同步 HuggingFace" },
  { name: "caddy-proxy", desc: "HTTPS 反向代理" },
];
skills.forEach((sk, i) => {
  s14.addText(sk.name, {
    x: 5.4, y: 2.05 + i * 0.42, w: 2.2, h: 0.38, fontSize: 10, fontFace: FONT_H,
    color: C.amber, margin: 0,
  });
  s14.addText(sk.desc, {
    x: 7.6, y: 2.05 + i * 0.42, w: 1.7, h: 0.38, fontSize: 10, fontFace: FONT_B,
    color: C.textSecondary, margin: 0,
  });
});
// Bottom insight
s14.addShape(pres.shapes.RECTANGLE, {
  x: 0.5, y: 3.8, w: 4.3, h: 1.3, fill: { color: C.bgCardLight },
});
s14.addText("每个 Skill = 一个 Markdown 文件\nAI 加载后就能执行，不需要写框架代码", {
  x: 0.7, y: 3.9, w: 3.9, h: 1.0, fontSize: 13, fontFace: FONT_B,
  color: C.textPrimary, margin: 0,
});

// ============================================================
// SLIDE 15: Section - Act 4 Learning Demo (Climax)
// ============================================================
addSectionSlide("第四幕  ⭐ 视频高潮", "C++20 协程学习实战", "AI 到底怎么改变了我的学习方式");

// ============================================================
// SLIDE 16: Traditional vs AI Learning
// ============================================================
let s16 = addContentSlide("传统路径 vs AI 路径");
// Left: Traditional
s16.addShape(pres.shapes.RECTANGLE, {
  x: 0.5, y: 1.3, w: 4.3, h: 3.5, fill: { color: C.bgCard }, shadow: mkShadow(),
});
s16.addShape(pres.shapes.RECTANGLE, {
  x: 0.5, y: 1.3, w: 4.3, h: 0.06, fill: { color: C.rose },
});
s16.addText("传统路径", {
  x: 0.7, y: 1.5, w: 3.9, h: 0.5, fontSize: 18, fontFace: FONT_H,
  color: C.rose, bold: true, margin: 0,
});
s16.addText([
  { text: "打开 cppreference", options: { breakLine: true, bullet: true } },
  { text: "看模板元编程语法", options: { breakLine: true, bullet: true } },
  { text: "找 CppCon 视频看 2 小时", options: { breakLine: true, bullet: true } },
  { text: "抄 demo 编译报错", options: { breakLine: true, bullet: true } },
  { text: "StackOverflow 答案过期", options: { bullet: true } },
], {
  x: 0.7, y: 2.1, w: 3.9, h: 2.5, fontSize: 14, fontFace: FONT_B,
  color: C.textSecondary, margin: 0,
});
// Right: AI
s16.addShape(pres.shapes.RECTANGLE, {
  x: 5.2, y: 1.3, w: 4.3, h: 3.5, fill: { color: C.bgCard }, shadow: mkShadow(),
});
s16.addShape(pres.shapes.RECTANGLE, {
  x: 5.2, y: 1.3, w: 4.3, h: 0.06, fill: { color: C.emerald },
});
s16.addText("AI 路径", {
  x: 5.4, y: 1.5, w: 3.9, h: 0.5, fontSize: 18, fontFace: FONT_H,
  color: C.emerald, bold: true, margin: 0,
});
s16.addText([
  { text: "跟 AI 对话理解原理", options: { breakLine: true, bullet: true } },
  { text: "追问 why 和 how", options: { breakLine: true, bullet: true } },
  { text: "跨语言对比建立心智模型", options: { breakLine: true, bullet: true } },
  { text: "Skill 结构化沉淀", options: { breakLine: true, bullet: true } },
  { text: "Agent 通过 CLI 自动入库", options: { bullet: true } },
], {
  x: 5.4, y: 2.1, w: 3.9, h: 2.5, fontSize: 14, fontFace: FONT_B,
  color: C.textPrimary, margin: 0,
});

// ============================================================
// SLIDE 17: Three Rounds of AI Dialogue
// ============================================================
let s17 = addContentSlide("三轮 AI 对话：逐层深入");
const rounds = [
  {
    num: "第一轮", q: "promise_type 机制 + 最小 Generator",
    a: "生命周期、initial_suspend、yield_value", col: C.cyan,
  },
  {
    num: "第二轮", q: "co_await awaiter 三件套执行时序",
    a: "AI 直接输出 sequence diagram", col: C.emerald,
  },
  {
    num: "第三轮", q: "C++20 vs Rust async 三维度对比",
    a: "编译器变换 / 内存布局 / 调度器", col: C.amber,
  },
];
rounds.forEach((r, i) => {
  const yPos = 1.3 + i * 1.35;
  s17.addShape(pres.shapes.RECTANGLE, {
    x: 0.5, y: yPos, w: 9.0, h: 1.15, fill: { color: C.bgCard }, shadow: mkShadow(),
  });
  s17.addShape(pres.shapes.RECTANGLE, {
    x: 0.5, y: yPos, w: 0.06, h: 1.15, fill: { color: r.col },
  });
  s17.addText(r.num, {
    x: 0.8, y: yPos + 0.08, w: 1.2, h: 0.4, fontSize: 14, fontFace: FONT_H,
    color: r.col, bold: true, margin: 0,
  });
  s17.addText(r.q, {
    x: 2.0, y: yPos + 0.08, w: 7.2, h: 0.4, fontSize: 14, fontFace: FONT_B,
    color: C.white, margin: 0,
  });
  s17.addText("→ " + r.a, {
    x: 2.0, y: yPos + 0.55, w: 7.2, h: 0.4, fontSize: 12, fontFace: FONT_B,
    color: C.textSecondary, margin: 0,
  });
});
// Bottom insight
s17.addText("AI 的价值不是给你答案——是帮你快速建立正确的心智模型", {
  x: 0.5, y: 5.1, w: 9.0, h: 0.4, fontSize: 13, fontFace: FONT_B,
  color: C.amber, align: "center", margin: 0, italic: true,
});

// ============================================================
// SLIDE 18: C++20 vs Rust Comparison Table
// ============================================================
let s18 = addContentSlide("跨语言对比：C++20 Coroutine vs Rust async");
const tableHeader = [
  { text: "维度", options: { fill: { color: C.bgCardLight }, color: C.cyan, bold: true, fontSize: 13, fontFace: FONT_H } },
  { text: "C++20 Coroutine", options: { fill: { color: C.bgCardLight }, color: C.cyan, bold: true, fontSize: 13, fontFace: FONT_H } },
  { text: "Rust async/await", options: { fill: { color: C.bgCardLight }, color: C.cyan, bold: true, fontSize: 13, fontFace: FONT_H } },
];
const tableRows = [
  [
    { text: "编译器变换", options: { color: C.amber, fontSize: 12, fontFace: FONT_H } },
    { text: "状态机 + 堆分配\ncoroutine frame", options: { color: C.textPrimary, fontSize: 12, fontFace: FONT_B } },
    { text: "状态机 + 编译期确定\n大小的 Future enum", options: { color: C.textPrimary, fontSize: 12, fontFace: FONT_B } },
  ],
  [
    { text: "内存布局", options: { color: C.amber, fontSize: 12, fontFace: FONT_H } },
    { text: "堆上分配\npromise_type 控制分配器", options: { color: C.textPrimary, fontSize: 12, fontFace: FONT_B } },
    { text: "栈上或 Box::pin\n编译器计算 Future 大小", options: { color: C.textPrimary, fontSize: 12, fontFace: FONT_B } },
  ],
  [
    { text: "调度器", options: { color: C.amber, fontSize: 12, fontFace: FONT_H } },
    { text: "无内置调度器\n需自行实现或用库", options: { color: C.textPrimary, fontSize: 12, fontFace: FONT_B } },
    { text: "运行时提供\ntokio / async-std", options: { color: C.textPrimary, fontSize: 12, fontFace: FONT_B } },
  ],
];
s18.addTable([tableHeader, ...tableRows], {
  x: 0.5, y: 1.3, w: 9.0,
  border: { pt: 0.5, color: C.border },
  colW: [1.8, 3.6, 3.6],
  rowH: [0.5, 0.9, 0.9, 0.9],
  fill: { color: C.bgCard },
});

// ============================================================
// SLIDE 19: Knowledge Precipitation Flow
// ============================================================
let s19 = addContentSlide("Skill 驱动的知识沉淀");
const steps = [
  { num: "1", label: "AI 对话理解概念", detail: "跟 AI 讨论，建立心智模型", col: C.cyan },
  { num: "2", label: "Skill 结构化", detail: "tech-impl-deep-dive-writer 重组", col: C.emerald },
  { num: "3", label: "双语翻译", detail: "bilingual-translation-publisher", col: C.amber },
  { num: "4", label: "Agent 自动发布", detail: "sf-cli write-article 入库", col: C.rose },
  { num: "5", label: "语义搜索验证", detail: "中英文查询均可命中", col: C.cyan },
];
steps.forEach((step, i) => {
  const yPos = 1.3 + i * 0.82;
  // Number circle
  s19.addShape(pres.shapes.OVAL, {
    x: 0.6, y: yPos + 0.08, w: 0.45, h: 0.45, fill: { color: step.col },
  });
  s19.addText(step.num, {
    x: 0.6, y: yPos + 0.08, w: 0.45, h: 0.45, fontSize: 16, fontFace: FONT_H,
    color: C.bg, bold: true, align: "center", valign: "middle", margin: 0,
  });
  // Label + detail
  s19.addText(step.label, {
    x: 1.3, y: yPos + 0.02, w: 3.5, h: 0.35, fontSize: 15, fontFace: FONT_H,
    color: C.white, bold: true, margin: 0,
  });
  s19.addText(step.detail, {
    x: 1.3, y: yPos + 0.38, w: 3.5, h: 0.3, fontSize: 12, fontFace: FONT_B,
    color: C.textSecondary, margin: 0,
  });
});
// Right side: Skill contract card
s19.addShape(pres.shapes.RECTANGLE, {
  x: 5.5, y: 1.3, w: 4.0, h: 3.8, fill: { color: C.bgCard }, shadow: mkShadow(),
});
s19.addText("Skill 写作契约", {
  x: 5.7, y: 1.45, w: 3.6, h: 0.5, fontSize: 16, fontFace: FONT_H,
  color: C.cyan, bold: true, margin: 0,
});
s19.addText([
  { text: "声明式名词短语标题", options: { breakLine: true, bullet: true } },
  { text: "机制优先，代码位置作佐证", options: { breakLine: true, bullet: true } },
  { text: "总体到细节的层次结构", options: { breakLine: true, bullet: true } },
  { text: "", options: { breakLine: true, fontSize: 6 } },
  { text: "AI 不是帮你写文章", options: { breakLine: true, color: C.amber, bold: true } },
  { text: "是帮你把零散理解组织成结构化知识", options: { color: C.amber } },
], {
  x: 5.7, y: 2.1, w: 3.6, h: 2.8, fontSize: 13, fontFace: FONT_B,
  color: C.textSecondary, margin: 0,
});

// ============================================================
// SLIDE 20: Complete Learning Loop
// ============================================================
let s20 = addContentSlide("完整闭环");
// Flow diagram using boxes and arrows
const flowItems = [
  { label: "AI 对话理解", x: 0.3, col: C.cyan },
  { label: "Skill\n生成笔记", x: 2.2, col: C.emerald },
  { label: "Skill\n结构化", x: 4.1, col: C.amber },
  { label: "Agent\n自动发布", x: 6.0, col: C.rose },
  { label: "向量索引\n可搜索", x: 7.9, col: C.cyan },
];
flowItems.forEach((item) => {
  s20.addShape(pres.shapes.RECTANGLE, {
    x: item.x, y: 1.5, w: 1.6, h: 1.2, fill: { color: C.bgCard }, shadow: mkShadow(),
  });
  s20.addShape(pres.shapes.RECTANGLE, {
    x: item.x, y: 1.5, w: 1.6, h: 0.06, fill: { color: item.col },
  });
  s20.addText(item.label, {
    x: item.x, y: 1.65, w: 1.6, h: 0.9, fontSize: 12, fontFace: FONT_H,
    color: C.white, align: "center", valign: "middle", margin: 0,
  });
});
// Arrows between flow items
[1.9, 3.8, 5.7, 7.6].forEach((x) => {
  s20.addText("→", {
    x: x, y: 1.7, w: 0.3, h: 0.8, fontSize: 20, fontFace: FONT_B,
    color: C.textMuted, align: "center", valign: "middle", margin: 0,
  });
});
// Return arrow (bottom)
s20.addShape(pres.shapes.RECTANGLE, {
  x: 1.1, y: 3.0, w: 7.8, h: 0.04, fill: { color: C.textMuted },
});
s20.addText("↑ 下次学习时语义搜索召回", {
  x: 2.5, y: 2.85, w: 5, h: 0.4, fontSize: 11, fontFace: FONT_B,
  color: C.textMuted, align: "center", margin: 0,
});
// Big insight text
s20.addShape(pres.shapes.RECTANGLE, {
  x: 0.5, y: 3.6, w: 9.0, h: 1.6, fill: { color: C.bgCard }, shadow: mkShadow(),
});
s20.addText([
  { text: "AI 改变学习方式的核心：", options: { breakLine: true, color: C.cyan, bold: true, fontSize: 18 } },
  { text: "", options: { breakLine: true, fontSize: 6 } },
  { text: "不是让你学得更快，", options: { breakLine: true, color: C.textPrimary, fontSize: 16 } },
  { text: "是让学习成果有了可检索、可积累、可复用的载体。", options: { color: C.amber, fontSize: 16, bold: true } },
], { x: 0.8, y: 3.7, w: 8.4, h: 1.4, fontFace: FONT_B, margin: 0 });

// ============================================================
// SLIDE 21: Section - Act 5 pb-mapper
// ============================================================
addSectionSlide("第五幕", "pb-mapper 简介", "本地后端如何被前端访问？TCP 隧道");

// ============================================================
// SLIDE 22: pb-mapper Details
// ============================================================
let s22 = addContentSlide("pb-mapper：本地 ↔ 云端 TCP 隧道");
// Left: architecture mini diagram
s22.addShape(pres.shapes.RECTANGLE, {
  x: 0.5, y: 1.3, w: 4.5, h: 3.8, fill: { color: C.bgCard }, shadow: mkShadow(),
});
s22.addText("工作原理", {
  x: 0.7, y: 1.45, w: 4.1, h: 0.5, fontSize: 16, fontFace: FONT_H,
  color: C.cyan, bold: true, margin: 0,
});
const mapperFlow = [
  { label: "本地 Axum :3000", y: 2.1, col: C.rose },
  { label: "pb-mapper (本地端)", y: 2.7, col: C.amber },
  { label: "pb-mapper (云端)", y: 3.3, col: C.amber },
  { label: "Nginx :443 (TLS)", y: 3.9, col: C.emerald },
  { label: "GitHub Pages 前端", y: 4.5, col: C.cyan },
];
mapperFlow.forEach((m) => {
  s22.addShape(pres.shapes.RECTANGLE, {
    x: 0.9, y: m.y, w: 3.8, h: 0.45, fill: { color: C.bgCardLight },
  });
  s22.addShape(pres.shapes.RECTANGLE, {
    x: 0.9, y: m.y, w: 0.05, h: 0.45, fill: { color: m.col },
  });
  s22.addText(m.label, {
    x: 1.15, y: m.y, w: 3.4, h: 0.45, fontSize: 12, fontFace: FONT_H,
    color: C.textPrimary, valign: "middle", margin: 0,
  });
});
// Right: key features
s22.addShape(pres.shapes.RECTANGLE, {
  x: 5.3, y: 1.3, w: 4.2, h: 3.8, fill: { color: C.bgCard }, shadow: mkShadow(),
});
s22.addText("核心特性", {
  x: 5.5, y: 1.45, w: 3.8, h: 0.5, fontSize: 16, fontFace: FONT_H,
  color: C.amber, bold: true, margin: 0,
});
s22.addText([
  { text: "TCP 层透传", options: { breakLine: true, bullet: true, color: C.textPrimary } },
  { text: "HTTP headers 完整保留", options: { breakLine: true, bullet: true, color: C.textPrimary } },
  { text: "连接复用", options: { breakLine: true, bullet: true, color: C.textPrimary } },
  { text: "心跳保活", options: { breakLine: true, bullet: true, color: C.textPrimary } },
  { text: "断线重连", options: { breakLine: true, bullet: true, color: C.textPrimary } },
  { text: "", options: { breakLine: true, fontSize: 6 } },
  { text: "Rust 实现，下期详解", options: { color: C.textMuted, italic: true } },
], {
  x: 5.5, y: 2.1, w: 3.8, h: 2.8, fontSize: 13, fontFace: FONT_B, margin: 0,
});

// ============================================================
// SLIDE 23: Section - Act 6 Summary
// ============================================================
addSectionSlide("第六幕", "总结", "三个核心观点");

// ============================================================
// SLIDE 24: Three Core Insights
// ============================================================
let s24 = addContentSlide("三个核心观点");
const insights = [
  {
    title: "长期记忆 > 短期记忆",
    body: "每次学习产出可检索的知识\nAI 随时调用，辅助新的学习",
    col: C.cyan,
  },
  {
    title: "Skills > Agents",
    body: "7 个 Skill 文件\n= 7 个可复用的 AI 工作流",
    col: C.emerald,
  },
  {
    title: "本地优先",
    body: "数据在自己手里\n不依赖云服务",
    col: C.amber,
  },
];
insights.forEach((ins, i) => {
  const xPos = 0.5 + i * 3.1;
  s24.addShape(pres.shapes.RECTANGLE, {
    x: xPos, y: 1.3, w: 2.8, h: 3.5, fill: { color: C.bgCard }, shadow: mkShadow(),
  });
  s24.addShape(pres.shapes.RECTANGLE, {
    x: xPos, y: 1.3, w: 2.8, h: 0.06, fill: { color: ins.col },
  });
  // Big number
  s24.addText(String(i + 1), {
    x: xPos + 0.2, y: 1.5, w: 0.8, h: 0.8, fontSize: 48, fontFace: FONT_H,
    color: ins.col, bold: true, margin: 0,
  });
  s24.addText(ins.title, {
    x: xPos + 0.2, y: 2.4, w: 2.4, h: 0.6, fontSize: 18, fontFace: FONT_H,
    color: C.white, bold: true, margin: 0,
  });
  s24.addText(ins.body, {
    x: xPos + 0.2, y: 3.1, w: 2.4, h: 1.4, fontSize: 13, fontFace: FONT_B,
    color: C.textSecondary, margin: 0,
  });
});

// ============================================================
// SLIDE 25: CTA / Closing
// ============================================================
let s25 = pres.addSlide();
s25.background = { color: C.bg };
s25.addShape(pres.shapes.RECTANGLE, { x: 0, y: 0, w: 10, h: 0.06, fill: { color: C.cyan } });
// Big closing text
s25.addText("StaticFlow", {
  x: 0.8, y: 1.0, w: 8.4, h: 1.0, fontSize: 44, fontFace: FONT_H,
  color: C.white, bold: true, align: "center", margin: 0,
});
s25.addText("开源  ·  本地优先  ·  Skill 驱动", {
  x: 0.8, y: 2.0, w: 8.4, h: 0.6, fontSize: 18, fontFace: FONT_B,
  color: C.cyan, align: "center", margin: 0,
});
// Links card
s25.addShape(pres.shapes.RECTANGLE, {
  x: 2.0, y: 3.0, w: 6.0, h: 1.6, fill: { color: C.bgCard }, shadow: mkShadow(),
});
s25.addText([
  { text: "GitHub: github.com/acking-you/static-flow", options: { breakLine: true, color: C.textPrimary } },
  { text: "Website: acking-you.github.io", options: { breakLine: true, color: C.textPrimary } },
  { text: "", options: { breakLine: true, fontSize: 6 } },
  { text: "Star · Fork · Issue · 一起交流", options: { color: C.amber } },
], {
  x: 2.3, y: 3.15, w: 5.4, h: 1.3, fontSize: 14, fontFace: FONT_H,
  align: "center", margin: 0,
});
// Next episode teaser
s25.addShape(pres.shapes.RECTANGLE, {
  x: 2.5, y: 4.85, w: 5.0, h: 0.5, fill: { color: C.bgCardLight },
});
s25.addText("下期预告：pb-mapper — Rust TCP 隧道实现", {
  x: 2.5, y: 4.85, w: 5.0, h: 0.5, fontSize: 13, fontFace: FONT_B,
  color: C.textSecondary, align: "center", valign: "middle", margin: 0,
});

// ============================================================
// WRITE FILE
// ============================================================
const outPath = "/home/ts_user/rust_pro/static_flow/docs/ai-learning-staticflow.pptx";
pres.writeFile({ fileName: outPath }).then(() => {
  console.log("PPTX created: " + outPath);
}).catch((err) => {
  console.error("Error:", err);
});