#![allow(dead_code)]

pub mod common {
    pub const GITHUB: &str = "GitHub";
    pub const BILIBILI: &str = "Bilibili";
    pub const SEARCH_PLACEHOLDER: &str = "搜索...";
    pub const LOADING: &str = "加载中...";
    pub const TERMINAL_PROMPT_CMD: &str = "$ ";
    pub const TERMINAL_PROMPT_OUTPUT: &str = "> ";
    pub const ARROW_RIGHT: &str = "→";
}

pub mod theme_toggle {
    pub const SWITCH_TO_LIGHT: &str = "切换到亮色模式";
    pub const SWITCH_TO_DARK: &str = "切换到暗色模式";
}

pub mod loading_spinner {
    pub const ARIA_LABEL: &str = "Loading";
}

pub mod pagination {
    pub const ARIA_NAV: &str = "分页";
    pub const ARIA_PREV: &str = "上一页";
    pub const ARIA_NEXT: &str = "下一页";
    pub const ARIA_GOTO_PAGE_TEMPLATE: &str = "跳转到第 {} 页";
}

pub mod scroll_to_top {
    pub const TOOLTIP: &str = "回到顶部";
}

pub mod toc_button {
    pub const TOOLTIP: &str = "目录";
}

pub mod error_banner {
    pub const TITLE: &str = "发生错误";
    pub const CLOSE_ARIA: &str = "关闭错误提示";
}

pub mod footer {
    pub const COPYRIGHT: &str = "© 2024 L_B__. All rights reserved.";
    pub const SOCIAL_ARIA: &str = "社交媒体";
}

pub mod header {
    pub const NAV_LATEST: &str = "最新";
    pub const NAV_POSTS: &str = "文章";
    pub const NAV_TAGS: &str = "标签";
    pub const NAV_CATEGORIES: &str = "分类";
    pub const NAV_MAIN_ARIA: &str = "主导航";
    pub const IMAGE_SEARCH_TITLE: &str = "图片搜索";
    pub const SEARCH_ARIA: &str = "搜索";
    pub const CLEAR_ARIA: &str = "清空";
    pub const OPEN_MENU_ARIA: &str = "打开菜单";
    pub const CLOSE_TOOLTIP: &str = "关闭";
    pub const MOBILE_NAV_ARIA: &str = "移动端导航";
    pub const BRAND_NAME: &str = "L_B__";
}

pub mod home {
    pub const STATS_ARTICLES: &str = "文章";
    pub const STATS_TAGS: &str = "标签";
    pub const STATS_CATEGORIES: &str = "分类";

    pub const TERMINAL_TITLE: &str = "system_info.sh";
    pub const CMD_SHOW_AVATAR: &str = "cat ./profile/avatar.jpg";
    pub const AVATAR_ALT: &str = "作者头像";
    pub const AVATAR_LINK_SR: &str = "前往文章列表";

    pub const CMD_SHOW_MOTTO: &str = "echo $MOTTO";
    pub const MOTTO: &str =
        "El Psy Kongroo | 世界线收束中... | Rustacean | Database 练习生，痴迷一切底层黑魔法";

    pub const CMD_SHOW_README: &str = "cat ./README.md";
    pub const INTRO: &str = "可视化博客 + Skill \
                             工作流：一键完成创作、分类、标签化、发布与部署；基于 LanceDB \
                             统一存储文章与图片，支持全文语义以及混合检索。";

    pub const CMD_SHOW_NAVIGATION: &str = "ls -l ./navigation/";
    pub const BTN_VIEW_ARTICLES: &str = "查看文章";
    pub const BTN_ARCHIVE: &str = "文章归档";

    pub const CMD_SHOW_SOCIAL: &str = "cat ./social_links.json";
    pub const CMD_SHOW_WRAPPED: &str = "./scripts/github-wrapped.sh --list-years";
    pub const CMD_SHOW_STATS: &str = "cat /proc/system/stats";

    pub const SYSTEM_UNIT_TOTAL: &str = "total";
    pub const POWERED_BY: &str = "POWERED BY";

    pub const GITHUB_WRAPPED_BADGE: &str = "NEW";
    pub const GITHUB_WRAPPED_SUBTITLE: &str = "年度代码回顾 →";
    pub const WRAPPED_MORE_YEARS_ARIA: &str = "查看更多年份";
    pub const WRAPPED_SELECT_YEAR: &str = "选择年份";
    pub const WRAPPED_LATEST_TAG: &str = "最新";
}

pub mod search {
    pub const IMAGE_MODE_HINT: &str = "可输入文字检索图片，或选择一张图片开始相似图片搜索";
    pub const IMAGE_TEXT_RESULTS: &str = "TEXT TO IMAGE";
    pub const IMAGE_TEXT_SEARCHING: &str = "检索文本相关图片...";
    pub const IMAGE_TEXT_NO_RESULTS: &str = "暂无文搜图结果";
    pub const IMAGE_TEXT_MISS_TEMPLATE: &str = "未找到与「{}」语义相关的图片";
    pub const IMAGE_TEXT_FOUND_TEMPLATE: &str = "找到 {} 张语义相关图片";
    pub const EMPTY_KEYWORD_HINT: &str = "请在上方搜索框输入关键词";
    pub const SEARCH_LOADING: &str = "正在扫描数据库...";

    pub const KEYWORD_MISS_TEMPLATE: &str = "关键词检索未命中「{}」，建议切换到 Semantic 语义检索";
    pub const KEYWORD_FOUND_TEMPLATE: &str =
        "关键词检索找到 {} 篇结果；你也可以试试 Semantic 语义检索，通常更能理解上下文";
    pub const SEMANTIC_MISS_TEMPLATE: &str = "未找到与「{}」语义相关的文章";
    pub const SEMANTIC_FOUND_TEMPLATE: &str = "找到 {} 篇语义相关内容";

    pub const KEYWORD_GUIDE_BANNER: &str =
        "提示：你当前使用的是关键词检索。即使已有结果，也建议对比一下 Semantic 语义检索。";
    pub const SWITCH_TO_SEMANTIC: &str = "切换到 Semantic";
    pub const NO_RESULTS_TITLE: &str = "NO RESULTS FOUND";
    pub const KEYWORD_EMPTY_CARD_DESC: &str =
        "关键词检索没命中，建议切换到 Semantic 语义检索，它更擅长找语义相关内容。";
    pub const SEMANTIC_EMPTY_CARD_DESC: &str = "未找到语义相关结果，可尝试更具体的关键词。";
    pub const SWITCH_TO_SEMANTIC_CTA: &str = "改用 Semantic 语义检索";

    pub const SEARCH_ENGINE_BADGE: &str = "// SEARCH_ENGINE";
    pub const STATUS_SCANNING: &str = "SCANNING";
    pub const STATUS_READY: &str = "READY";
    pub const MODE_KEYWORD: &str = "Keyword";
    pub const MODE_SEMANTIC: &str = "Semantic";
    pub const MODE_IMAGE: &str = "Image";
    pub const RESULT_SCOPE: &str = "Result Scope";
    pub const RESULT_SCOPE_LIMITED_TEMPLATE: &str = "默认 {} 条";
    pub const RESULT_SCOPE_ALL: &str = "全部召回";
    pub const DISTANCE_FILTER: &str = "Distance Filter";
    pub const DISTANCE_FILTER_OFF: &str = "关闭";
    pub const DISTANCE_FILTER_STRICT: &str = "<= 0.8";
    pub const DISTANCE_FILTER_RELAXED: &str = "<= 1.2";
    pub const DISTANCE_FILTER_INPUT_PLACEHOLDER: &str = "输入最大距离";
    pub const DISTANCE_FILTER_APPLY: &str = "应用";
    pub const HIGHLIGHT_PRECISION: &str = "Highlight Precision";
    pub const HIGHLIGHT_FAST: &str = "Fast (Default)";
    pub const HIGHLIGHT_ENHANCED: &str = "Enhanced (Slower)";
    pub const HYBRID_PANEL_TITLE: &str = "Hybrid Search";
    pub const HYBRID_PANEL_DESC: &str =
        "混合检索会把向量召回与关键词召回做 RRF 融合，通常在语义与精确匹配之间更稳。";
    pub const HYBRID_DEFAULT_SCOPE_LIMIT_TEMPLATE: &str =
        "默认值：RRF K=60；Vector/FTS 候选窗口留空时跟随 Result Scope（当前 {}）。";
    pub const HYBRID_DEFAULT_SCOPE_ALL: &str =
        "默认值：RRF K=60；Vector/FTS 候选窗口留空时不设上限（全部召回模式）。";
    pub const HYBRID_ADVANCED_SHOW: &str = "展开高级参数";
    pub const HYBRID_ADVANCED_HIDE: &str = "收起高级参数";
    pub const HYBRID_ON: &str = "Hybrid ON";
    pub const HYBRID_OFF: &str = "Hybrid OFF";
    pub const HYBRID_RRF_K: &str = "RRF K（默认 60）";
    pub const HYBRID_VECTOR_LIMIT: &str = "Vector 候选窗口";
    pub const HYBRID_FTS_LIMIT: &str = "FTS 候选窗口";
    pub const HYBRID_VECTOR_LIMIT_SCOPE_TEMPLATE: &str = "Vector 候选窗口（留空跟随 {}）";
    pub const HYBRID_VECTOR_LIMIT_ALL: &str = "Vector 候选窗口（留空不设上限）";
    pub const HYBRID_FTS_LIMIT_SCOPE_TEMPLATE: &str = "FTS 候选窗口（留空跟随 {}）";
    pub const HYBRID_FTS_LIMIT_ALL: &str = "FTS 候选窗口（留空不设上限）";
    pub const HYBRID_APPLY: &str = "应用 Hybrid 参数";
    pub const IMAGE_TEXT_QUERY_TEMPLATE: &str = "当前描述：{}";
    pub const IMAGE_CATALOG: &str = "IMAGE CATALOG";
    pub const IMAGE_LOADING: &str = "加载图片中...";
    pub const IMAGE_EMPTY_HINT: &str = "暂无图片，请先运行 sf-cli write-images.";
    pub const SIMILAR_IMAGES: &str = "SIMILAR IMAGES";
    pub const IMAGE_SEARCHING: &str = "检索相似图片...";
    pub const IMAGE_NO_SIMILAR: &str = "暂无相似图片结果";
    pub const IMAGE_SELECT_HINT: &str = "点击上方图片开始搜索相似图片";
    pub const IMAGE_SCROLL_LOADING: &str = "滚动中，正在加载更多图片...";
    pub const IMAGE_SCROLL_HINT: &str = "继续向下滚动加载更多";
    pub const LIGHTBOX_CLOSE_ARIA: &str = "关闭图片预览";
    pub const LIGHTBOX_ZOOM_IN_ARIA: &str = "放大图片";
    pub const LIGHTBOX_ZOOM_OUT_ARIA: &str = "缩小图片";
    pub const LIGHTBOX_ZOOM_RESET_ARIA: &str = "重置图片缩放";
    pub const LIGHTBOX_DOWNLOAD: &str = "下载";
    pub const LIGHTBOX_IMAGE_ALT: &str = "预览图片";
    pub const LIGHTBOX_PREVIEW_FAILED: &str = "图片加载失败，可尝试在新标签打开：{}";
    pub const SEARCHING_SHORT: &str = "正在扫描...";
    pub const MATCH_BADGE: &str = "MATCH";
}

pub mod categories_page {
    pub const HERO_INDEX: &str = "Category Index";
    pub const HERO_TITLE: &str = "知识图谱";
    pub const HERO_DESC_TEMPLATE: &str = "探索 {} 个领域，汇聚 {} 篇文章";
    pub const HERO_BADGE_TEMPLATE: &str = "{} CATEGORIES";
    pub const EMPTY: &str = "暂无分类";
    pub const COUNT_TEMPLATE: &str = "{} 篇";
}

pub mod tags_page {
    pub const HERO_INDEX: &str = "Tag Index";
    pub const HERO_TITLE: &str = "标签索引";
    pub const HERO_DESC_TEMPLATE: &str = "汇总 {} 个标签，覆盖 {} 篇文章";
    pub const TAG_COUNT_TEMPLATE: &str = "{} 标签";
    pub const ARTICLE_COUNT_TEMPLATE: &str = "{} 文章";
    pub const EMPTY: &str = "暂无标签";
    pub const CLOUD_ARIA: &str = "标签云";
}

pub mod posts_page {
    pub const HERO_INDEX: &str = "Latest Articles";
    pub const HERO_TITLE: &str = "时间线";

    pub const DESC_EMPTY_FILTERED: &str = "当前筛选下暂无文章，换个标签或分类试试？";
    pub const DESC_EMPTY_ALL: &str = "暂时还没有文章，敬请期待。";
    pub const DESC_FILTERED_TEMPLATE: &str = "共找到 {} 篇文章匹配当前筛选。";
    pub const DESC_ALL_TEMPLATE: &str = "现在共有 {} 篇文章，按年份倒序排列。";

    pub const FILTER_CLEAR: &str = "清除";
    pub const EMPTY: &str = "暂无文章可展示。";

    pub const YEAR_COUNT_TEMPLATE: &str = "{} 篇";
    pub const COLLAPSE: &str = "收起";
    pub const EXPAND_REMAINING_TEMPLATE: &str = "展开剩余 {} 篇";
    pub const YEAR_TOGGLE_ARIA_TEMPLATE: &str = "切换 {} 年文章折叠状态";

    pub const PUBLISHED_ON_TEMPLATE: &str = "Published on {}";
}

pub mod latest_articles_page {
    pub const HERO_INDEX: &str = "Latest Articles";
    pub const HERO_TITLE: &str = "最新文章";
    pub const HERO_DESC: &str = "甄选近期发布的内容，持续更新";
    pub const EMPTY: &str = "暂无文章";
}

pub mod category_detail_page {
    pub const UNNAMED: &str = "未命名分类";
    pub const EMPTY_TEMPLATE: &str = "分类「{}」下暂无文章，换个分类看看？";
    pub const INVALID_NAME: &str = "请输入有效的分类名称。";
    pub const COLLECTION_BADGE: &str = "Category Collection";
    pub const HIGHLIGHT_COUNT_TEMPLATE: &str = "{} 篇精选内容";
    pub const NO_CONTENT: &str = "暂无内容";
    pub const YEAR_POSTS_TEMPLATE: &str = "{} 篇文章";
}

pub mod tag_detail_page {
    pub const UNNAMED: &str = "未命名标签";
    pub const EMPTY_TEMPLATE: &str = "标签「{}」下暂无文章，换个标签看看？";
    pub const INVALID_NAME: &str = "请输入有效的标签名称。";
    pub const ARCHIVE_BADGE: &str = "Tag Archive";
    pub const COLLECTED_COUNT_TEMPLATE: &str = "{} 篇收录文章";
    pub const NO_CONTENT: &str = "暂无文章";
}

pub mod article_detail_page {
    pub const VIEW_ORIGINAL_IMAGE: &str = "查看原图";
    pub const ARTICLE_META_ARIA: &str = "文章元信息";
    pub const ARTICLE_BODY_ARIA: &str = "文章正文";
    pub const DETAILED_SUMMARY_ARIA: &str = "文章详细总结";
    pub const TAGS_TITLE: &str = "标签";
    pub const RELATED_TITLE: &str = "相关推荐";
    pub const RELATED_LOADING: &str = "加载相关推荐中...";
    pub const NO_RELATED: &str = "暂无相关推荐";
    pub const LANG_SWITCH_LABEL: &str = "语言";
    pub const LANG_SWITCH_ZH: &str = "中文";
    pub const LANG_SWITCH_EN: &str = "English";
    pub const DETAILED_SUMMARY_TITLE_ZH: &str = "快速导读";
    pub const DETAILED_SUMMARY_TITLE_EN: &str = "Quick Brief";
    pub const OPEN_BRIEF_BUTTON_ZH: &str = "查看导读";
    pub const OPEN_BRIEF_BUTTON_EN: &str = "Open Brief";
    pub const CLOSE_BRIEF_ARIA: &str = "关闭快速导读";
    pub const CLOSE_BRIEF_BUTTON: &str = "关闭";

    pub const WORD_COUNT_TEMPLATE: &str = "{} 字";
    pub const READ_TIME_TEMPLATE: &str = "约 {} 分钟";

    pub const NOT_FOUND_TITLE: &str = "文章未找到";
    pub const NOT_FOUND_DESC: &str = "抱歉，没有找到对应的文章，请返回列表重试。";

    pub const BACK_TOOLTIP: &str = "返回";
    pub const CLOSE_IMAGE_ARIA: &str = "关闭图片";
    pub const LIGHTBOX_ZOOM_IN_ARIA: &str = "放大图片";
    pub const LIGHTBOX_ZOOM_OUT_ARIA: &str = "缩小图片";
    pub const LIGHTBOX_ZOOM_RESET_ARIA: &str = "重置图片缩放";
    pub const DEFAULT_IMAGE_ALT: &str = "文章图片";
    pub const IMAGE_PREVIEW_FAILED: &str = "图片加载失败，可尝试在新标签打开：{}";
}

pub mod not_found_page {
    pub const TERMINAL_TITLE: &str = "error.sh";
    pub const CMD_LOOKUP: &str = "curl http://localhost:8080$(location.pathname)";
    pub const ERROR_PREFIX: &str = "ERROR: ";
    pub const ERROR_CODE: &str = "404 Not Found";
    pub const ERROR_DETAIL: &str = "The requested resource could not be found on this server.";

    pub const CMD_SUGGESTIONS: &str = "cat /var/log/suggestions.log";
    pub const SUGGESTION_1: &str = "抱歉，你要找的页面走丢了... 可能是被外星人劫持了 👽";
    pub const SUGGESTION_2: &str = "建议：检查 URL 拼写，或者返回首页重新探索。";

    pub const CMD_AVAILABLE_ROUTES: &str = "ls -l ./available_routes/";
    pub const BTN_HOME: &str = "返回首页";
    pub const BTN_LATEST: &str = "最新文章";
    pub const BTN_ARCHIVE: &str = "文章归档";
}


pub mod mock {
    pub const ARTICLE_TITLE_TEMPLATE: &str = "示例文章 {} - {} 技术与思考";
    pub const ARTICLE_SUMMARY_TEMPLATE: &str = "这是一篇关于 {} 的示例文章，涵盖实践要点与思考。";
}
