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
    pub const NAV_MUSIC: &str = "音乐";
    pub const NAV_LLM: &str = "LLM";
    pub const NAV_MAIN_ARIA: &str = "主导航";
    pub const IMAGE_SEARCH_TITLE: &str = "图片搜索";
    pub const IMAGE_LIBRARY_TITLE: &str = "图片库";
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
    pub const STATS_MUSIC: &str = "音乐库";
    pub const STATS_IMAGES: &str = "图片库";

    pub const TERMINAL_TITLE: &str = "system_info.sh";
    pub const CMD_SHOW_AVATAR: &str = "cat ./profile/avatar.jpg";
    pub const AVATAR_ALT: &str = "作者头像";
    pub const AVATAR_LINK_SR: &str = "前往文章列表";

    pub const CMD_SHOW_MOTTO: &str = "echo $MOTTO";
    pub const MOTTO: &str = "El Psy Kongroo | Rustacean | 痴迷底层黑魔法的 Database 练习生";

    pub const CMD_SHOW_README: &str = "cat ./README.md";
    pub const INTRO: &str = "本地优先的全栈 Rust 内容平台 — 文章 · 音乐 · 图片统一托管于 \
                             LanceDB，支持全文 / 语义 / 混合检索，结合 AI Skill \
                             工作流驱动内容创作与自动化运维";

    pub const CMD_SHOW_LLM_ACCESS: &str = "cat ./llm-access.md";
    pub const LLM_ACCESS_HINT: &str =
        "免费中转 API Key 按余量放出，接 Codex 写代码、养龙虾🦞 两不误 (๑•̀ㅂ•́)و✧ 复制配置直接开用~";
    pub const BTN_LLM_ACCESS: &str = "🦞 获取 Key";

    pub const CMD_SHOW_NAVIGATION: &str = "ls -l ./navigation/";
    pub const BTN_VIEW_ARTICLES: &str = "文章";
    pub const BTN_ARCHIVE: &str = "归档";
    pub const BTN_SEARCH_STATICFLOW: &str = "搜索";

    pub const CMD_SHOW_SOCIAL: &str = "cat ./social_links.json";
    pub const CMD_SHOW_MEDIA_HUB: &str = "ls -l ./media-hub/";
    pub const BTN_MEDIA_VIDEO: &str = "视频";
    pub const BTN_MEDIA_AUDIO: &str = "音频";
    pub const BTN_MEDIA_IMAGE: &str = "图片";
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
    pub const MODE_MUSIC: &str = "Music";
    pub const MUSIC_SEARCHING: &str = "正在搜索音乐...";
    pub const MUSIC_MISS_TEMPLATE: &str = "未找到与「{}」相关的音乐";
    pub const MUSIC_FOUND_TEMPLATE: &str = "找到 {} 首相关音乐";
    pub const MUSIC_TRY_SEMANTIC: &str = "试试语义搜索";
    pub const MUSIC_TRY_HYBRID: &str = "试试混合搜索";
    pub const MUSIC_TRY_HINT: &str = "关键词没找到？语义搜索能理解歌曲含义，混合搜索兼顾精确与语义";
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
    pub const IMAGE_SCROLL_LOADING: &str = "正在加载更多图片...";
    pub const IMAGE_SCROLL_HINT: &str = "加载更多";
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
    pub const OPEN_INTERACTIVE_BUTTON_ZH: &str = "打开交互原版";
    pub const OPEN_INTERACTIVE_BUTTON_EN: &str = "Open Interactive";
    pub const INTERACTIVE_ALERT_BADGE: &str = "Interactive First";
    pub const INTERACTIVE_ALERT_TITLE_ZH: &str = "这篇内容请优先切换到交互界面";
    pub const INTERACTIVE_ALERT_TITLE_EN: &str = "Open The Interactive View First";
    pub const INTERACTIVE_ALERT_DESC_ZH: &str =
        "正文更适合检索与引用；核心图示、参数拖拽和 Bloom Filter 演示都在交互界面里。";
    pub const INTERACTIVE_ALERT_DESC_EN: &str = "Use the text version for search and quoting. The \
                                                 key graphs, sliders, and bloom-filter demos live \
                                                 in the interactive view.";
    pub const INTERACTIVE_ALERT_NOTE_ZH: &str =
        "交互页内支持直接切换中文 / English，阅读体验会完整很多。";
    pub const INTERACTIVE_ALERT_NOTE_EN: &str =
        "The interactive page lets you switch between 中文 and English directly.";
    pub const INTERACTIVE_ALERT_OPEN_ZH: &str = "立即进入交互界面";
    pub const INTERACTIVE_ALERT_OPEN_EN: &str = "Open Interactive Now";
    pub const INTERACTIVE_ALERT_STAY_ZH: &str = "暂时留在本文页";
    pub const INTERACTIVE_ALERT_STAY_EN: &str = "Stay On This Page";
    pub const INTERACTIVE_ALERT_MODAL_ARIA: &str = "交互模式提醒";
    pub const INTERACTIVE_ALERT_CLOSE_ARIA: &str = "关闭交互提示";
    pub const OPEN_RAW_MARKDOWN_BUTTON_ZH: &str = "查看原始 Markdown";
    pub const OPEN_RAW_MARKDOWN_BUTTON_EN: &str = "View Raw Markdown";
    pub const CLOSE_BRIEF_ARIA: &str = "关闭快速导读";
    pub const CLOSE_BRIEF_BUTTON: &str = "关闭";

    pub const WORD_COUNT_TEMPLATE: &str = "{} 字";
    pub const READ_TIME_TEMPLATE: &str = "约 {} 分钟";
    pub const VIEW_COUNT_TEMPLATE: &str = "{} 次浏览";
    pub const VIEW_COUNT_LOADING: &str = "浏览量统计中...";

    pub const NOT_FOUND_TITLE: &str = "文章未找到";
    pub const NOT_FOUND_DESC: &str = "抱歉，没有找到对应的文章，请返回列表重试。";

    pub const BACK_TOOLTIP: &str = "返回";
    pub const TREND_TOOLTIP: &str = "查看浏览趋势";
    pub const TREND_TITLE: &str = "浏览趋势";
    pub const TREND_SUBTITLE: &str = "按天或按小时查看浏览变化";
    pub const TREND_TAB_DAY: &str = "按天";
    pub const TREND_TAB_HOUR: &str = "按小时";
    pub const TREND_SELECT_DAY: &str = "日期";
    pub const TREND_LOADING: &str = "趋势加载中...";
    pub const TREND_EMPTY: &str = "暂无趋势数据";
    pub const TREND_TOTAL_TEMPLATE: &str = "总浏览：{}";
    pub const TREND_CLOSE_ARIA: &str = "关闭趋势面板";
    pub const CLOSE_IMAGE_ARIA: &str = "关闭图片";
    pub const LIGHTBOX_ZOOM_IN_ARIA: &str = "放大图片";
    pub const LIGHTBOX_ZOOM_OUT_ARIA: &str = "缩小图片";
    pub const LIGHTBOX_ZOOM_RESET_ARIA: &str = "重置图片缩放";
    pub const DEFAULT_IMAGE_ALT: &str = "文章图片";
    pub const IMAGE_PREVIEW_FAILED: &str = "图片加载失败，可尝试在新标签打开：{}";
    pub const SOURCE_LINK_TEXT: &str = "原文来源";
}

pub mod interactive_article_page {
    pub const BADGE: &str = "Interactive Mirror";
    pub const TITLE_NOTE: &str = "站内镜像保留原页面交互，并支持在界面内切换中文 / English。";
    pub const BACK_TO_ARTICLE: &str = "返回文章";
    pub const OPEN_SOURCE: &str = "打开来源";
    pub const LOADING: &str = "正在加载交互页...";
    pub const REDIRECT_NOTE: &str =
        "正在为你打开交互镜像；如果浏览器没有自动跳转，可以直接点下面的按钮。";
    pub const OPEN_INTERACTIVE: &str = "进入交互镜像";
    pub const NOT_AVAILABLE_TITLE: &str = "交互页不可用";
    pub const NOT_AVAILABLE_DESC: &str = "当前文章没有关联交互镜像，或镜像尚未准备完成。";
    pub const LOAD_ERROR_PREFIX: &str = "加载失败";
}

pub mod article_raw_page {
    pub const RAW_BADGE: &str = "Raw Markdown";
    pub const TITLE_TEMPLATE: &str = "{} · {}";
    pub const BACK_BUTTON: &str = "返回文章";
    pub const COPY_BUTTON: &str = "复制";
    pub const COPIED_BUTTON: &str = "已复制";
    pub const LOADING: &str = "正在加载原始 Markdown...";
    pub const ERROR_PREFIX: &str = "加载失败";
    pub const EMPTY: &str = "原始内容为空";
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


pub mod coming_soon_page {
    pub const TERMINAL_TITLE_TEMPLATE: &str = "{}.sh";
    pub const CMD_INIT_TEMPLATE: &str = "./scripts/init-{}.sh --status";
    pub const STATUS_LABEL: &str = "STATUS: ";
    pub const STATUS_COMING_SOON: &str = "COMING SOON";
    pub const DESC_VIDEO: &str =
        "视频中心正在开发中，即将支持在线播放与语义搜索，快速定位你想看的内容。";
    pub const DESC_AUDIO: &str = "音频中心正在开发中，即将支持播客/音乐在线播放与智能检索。";
    pub const DESC_DEFAULT: &str = "该功能模块正在开发中，敬请期待。";
    pub const CMD_AVAILABLE_ROUTES: &str = "ls -l ./available_routes/";
    pub const BTN_HOME: &str = "返回首页";
    pub const BTN_IMAGE_LIBRARY: &str = "图片库";
}

pub mod image_library_page {
    pub const TITLE: &str = "图片库";
    pub const SUBTITLE: &str = "浏览、检索并管理你的本地图片集合";
    pub const MODE_RANDOM: &str = "随机 10 张";
    pub const MODE_ALL: &str = "全部图片";
    pub const BTN_REFRESH_RANDOM: &str = "换一组";
    pub const BTN_CLEAR_SEARCH: &str = "清空搜索";
    pub const SEARCH_PLACEHOLDER: &str = "搜索图片描述、关键词或文件名...";
    pub const LABEL_SEARCH_RESULTS: &str = "搜索结果";
    pub const LABEL_RANDOM_HINT: &str = "默认随机展示 10 张图片，可点击“换一组”刷新";
    pub const LABEL_TOTAL_IMAGES: &str = "图片总数";
    pub const LOAD_ERROR_PREFIX: &str = "加载失败";
    pub const EMPTY_LIBRARY: &str = "暂无图片，请先入库图片资源";
    pub const EMPTY_SEARCH: &str = "未找到匹配的图片";
    pub const BTN_AUDIO_LIBRARY: &str = "音频库";
    pub const BTN_VIDEO_LIBRARY: &str = "视频库";
}

pub mod music_wish {
    pub const SECTION_TITLE: &str = "许愿点歌";
    pub const SECTION_SUBTITLE: &str = "想听什么歌？留下你的心愿，我们会尽力帮你找到";
    pub const SONG_NAME_LABEL: &str = "歌名";
    pub const SONG_NAME_PLACEHOLDER: &str = "输入歌曲名称（必填）";
    pub const ARTIST_LABEL: &str = "歌手";
    pub const ARTIST_PLACEHOLDER: &str = "歌手名（可选）";
    pub const MESSAGE_LABEL: &str = "留言";
    pub const MESSAGE_PLACEHOLDER: &str = "说说你为什么想听这首歌...（必填）";
    pub const NICKNAME_LABEL: &str = "昵称（可选）";
    pub const NICKNAME_PLACEHOLDER: &str = "你的昵称（可选，不填会自动生成）";
    pub const EMAIL_LABEL: &str = "邮箱（可选）";
    pub const EMAIL_PLACEHOLDER: &str = "填写邮箱可实时接收完成进度";
    pub const EMAIL_HELP_TEXT: &str = "可选，但建议填写；任务完成后会自动通知你";
    pub const SUBMIT_BTN: &str = "提交心愿";
    pub const SUBMITTING: &str = "提交中...";
    pub const STATUS_PENDING: &str = "等待审核";
    pub const STATUS_APPROVED: &str = "已通过";
    pub const STATUS_RUNNING: &str = "搜索中";
    pub const STATUS_DONE: &str = "已入库";
    pub const STATUS_FAILED: &str = "搜索失败";
    pub const LISTEN_NOW: &str = "去听歌 →";
    pub const EMPTY_LIST: &str = "还没有人许愿，来做第一个吧！";
    pub const SUBMIT_SUCCESS: &str = "心愿已提交，等待审核中";
    pub const REFRESH_BTN: &str = "刷新状态";
    pub const REFRESHING: &str = "刷新中...";
    pub const FILL_FORM: &str = "填入表单";
}

pub mod article_request {
    pub const SECTION_TITLE: &str = "文章入库请求";
    pub const SECTION_SUBTITLE: &str = "发现好文？提交链接，审核通过后自动入库";
    pub const URL_LABEL: &str = "文章链接";
    pub const URL_PLACEHOLDER: &str = "输入文章 URL（必填，http/https）";
    pub const TITLE_HINT_LABEL: &str = "标题提示";
    pub const TITLE_HINT_PLACEHOLDER: &str = "文章标题（可选）";
    pub const MESSAGE_LABEL: &str = "推荐理由";
    pub const MESSAGE_PLACEHOLDER: &str = "说说你为什么推荐这篇文章...（必填）";
    pub const NICKNAME_LABEL: &str = "昵称（可选）";
    pub const NICKNAME_PLACEHOLDER: &str = "你的昵称（可选，不填会自动生成）";
    pub const EMAIL_LABEL: &str = "邮箱（可选）";
    pub const EMAIL_PLACEHOLDER: &str = "填写邮箱可实时接收完成进度";
    pub const EMAIL_HELP_TEXT: &str = "可选，但建议填写；任务完成后会自动通知你";
    pub const SUBMIT_BTN: &str = "提交请求";
    pub const SUBMITTING: &str = "提交中...";
    pub const STATUS_PENDING: &str = "等待审核";
    pub const STATUS_APPROVED: &str = "已通过";
    pub const STATUS_RUNNING: &str = "入库中";
    pub const STATUS_DONE: &str = "已入库";
    pub const STATUS_DONE_NO_ARTICLE: &str = "已处理（未入库）";
    pub const STATUS_FAILED: &str = "入库失败";
    pub const VIEW_ARTICLE: &str = "查看文章 →";
    pub const EMPTY_LIST: &str = "还没有人提交请求，来做第一个吧！";
    pub const SUBMIT_SUCCESS: &str = "请求已提交，等待审核中";
    pub const REFRESH_BTN: &str = "刷新状态";
    pub const REFRESHING: &str = "刷新中...";
    pub const NAV_BTN: &str = "文章入库";
    pub const AI_REPLY_TOGGLE: &str = "查看 AI 回复";
    pub const FOLLOW_UP_BTN: &str = "继续对话 →";
    pub const FOLLOW_UP_INDICATOR: &str = "追问模式 — 基于已完成请求";
    pub const FOLLOW_UP_BADGE: &str = "追问";
    pub const FOLLOW_UP_REF_PREFIX: &str = "基于 #";
    pub const CANCEL_FOLLOW_UP: &str = "取消追问";
    pub const DETAIL_MODAL_TITLE: &str = "请求详情";
    pub const DETAIL_MODAL_CLOSE: &str = "关闭";
    pub const VIEW_DETAIL_BTN: &str = "查看详情";
    pub const LABEL_URL: &str = "文章链接";
    pub const LABEL_TITLE: &str = "标题";
    pub const LABEL_MESSAGE: &str = "推荐理由";
    pub const LABEL_AI_REPLY: &str = "AI 回复";
    pub const LABEL_REGION: &str = "来源";
    pub const NO_ARTICLE_NOTICE: &str =
        "这次请求已处理完成，但没有产出入库文章。请查看下方 AI 回复了解原因。";
}

pub mod mock {
    pub const ARTICLE_TITLE_TEMPLATE: &str = "示例文章 {} - {} 技术与思考";
    pub const ARTICLE_SUMMARY_TEMPLATE: &str = "这是一篇关于 {} 的示例文章，涵盖实践要点与思考。";
}
