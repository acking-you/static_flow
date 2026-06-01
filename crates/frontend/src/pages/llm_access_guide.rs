use gloo_timers::callback::Timeout;
use wasm_bindgen::prelude::*;
use yew::prelude::*;
use yew_router::prelude::Link;

use crate::{
    api::{
        fetch_llm_gateway_access, fetch_llm_gateway_model_catalog_json, LlmGatewayAccessResponse,
    },
    pages::llm_access_shared::{
        chat_curl_example, chat_python_example, codex_auth_json, codex_login_command,
        codex_model_catalog_download_command, codex_provider_config, example_key_name,
        example_key_secret, preferred_model_slug_from_catalog_json, resolved_base_url,
        resolved_model_catalog_url, REMOTE_COMPACT_ARTICLE_ID,
    },
    router::Route,
};

#[wasm_bindgen(inline_js = r#"
export function copy_text(text) {
    if (navigator.clipboard) {
        navigator.clipboard.writeText(text).catch(function(){});
    }
}
"#)]
extern "C" {
    fn copy_text(text: &str);
}

const CODEX_FEATURE_SNAPSHOT_VERSION: &str = "v0.124.0";
const CODEX_FEATURE_SNAPSHOT_DATE: &str = "2026-04-25";

#[derive(Clone, Copy, PartialEq, Eq)]
struct CodexFeatureGuideItem {
    key: &'static str,
    label: &'static str,
    stage: &'static str,
    default_enabled: bool,
    recommended_enabled: bool,
    summary: &'static str,
    config_note: &'static str,
}

const CODEX_FEATURE_CATALOG: &[CodexFeatureGuideItem] = &[
    CodexFeatureGuideItem {
        key: "memories",
        label: "Memories",
        stage: "experimental",
        default_enabled: false,
        recommended_enabled: true,
        summary: "从历史会话提炼可复用记忆，并在新会话里按需读取项目经验、偏好和踩坑记录。",
        config_note: "需要新会话生效；生成 memory 依赖本地 state DB 和后台整理任务。",
    },
    CodexFeatureGuideItem {
        key: "js_repl",
        label: "JavaScript REPL",
        stage: "experimental",
        default_enabled: false,
        recommended_enabled: false,
        summary: "给调试网页、脚本和小段 JS 逻辑时提供持久 Node REPL。",
        config_note: "需要 Node >= 22.22.0；只在明确需要 JS 交互调试时开启。",
    },
    CodexFeatureGuideItem {
        key: "prevent_idle_sleep",
        label: "Prevent sleep",
        stage: "experimental",
        default_enabled: false,
        recommended_enabled: false,
        summary: "长任务运行时尽量防止电脑休眠，适合编译、迁移、批处理等长会话。",
        config_note: "桌面/笔记本本地运行更有价值；服务器环境通常不需要。",
    },
    CodexFeatureGuideItem {
        key: "multi_agent",
        label: "Multi-agent",
        stage: "stable",
        default_enabled: true,
        recommended_enabled: true,
        summary: "允许 Codex 在合适场景拆出子任务并行处理，减少主线程等待。",
        config_note: "复杂代码分析或并行验证时有用；简单任务不会强制使用。",
    },
    CodexFeatureGuideItem {
        key: "apps",
        label: "Apps",
        stage: "stable",
        default_enabled: true,
        recommended_enabled: true,
        summary: "启用 Codex app 相关能力，是新版 TUI/应用化体验的基础开关之一。",
        config_note: "保持默认开启即可。",
    },
    CodexFeatureGuideItem {
        key: "plugins",
        label: "Plugins",
        stage: "stable",
        default_enabled: true,
        recommended_enabled: true,
        summary: "允许加载本地插件，让团队工作流、工具和技能可以被复用。",
        config_note: "使用自定义插件或 marketplace 时保留开启。",
    },
    CodexFeatureGuideItem {
        key: "tool_search",
        label: "Tool search",
        stage: "stable",
        default_enabled: true,
        recommended_enabled: true,
        summary: "把工具发现从一次性全量注入转为按需搜索，降低上下文负担。",
        config_note: "工具较多、MCP 较多时建议保持开启。",
    },
    CodexFeatureGuideItem {
        key: "tool_suggest",
        label: "Tool suggest",
        stage: "stable",
        default_enabled: true,
        recommended_enabled: true,
        summary: "让 Codex 能根据上下文提示可能有用的工具入口。",
        config_note: "和 tool_search 搭配使用。",
    },
    CodexFeatureGuideItem {
        key: "in_app_browser",
        label: "In-app browser",
        stage: "stable",
        default_enabled: true,
        recommended_enabled: true,
        summary: "启用应用内浏览器相关能力，方便在 Codex 流程内查看网页内容。",
        config_note: "需要网页检查、账号登录或可视化确认时更有价值。",
    },
    CodexFeatureGuideItem {
        key: "browser_use",
        label: "Browser use",
        stage: "stable",
        default_enabled: true,
        recommended_enabled: true,
        summary: "启用浏览器自动化/网页交互能力，用于真实页面检查和操作。",
        config_note: "前端验收、网页调试和资料核对建议开启。",
    },
    CodexFeatureGuideItem {
        key: "computer_use",
        label: "Computer use",
        stage: "stable",
        default_enabled: true,
        recommended_enabled: true,
        summary: "启用计算机操作类能力，为更完整的本地/图形界面任务做准备。",
        config_note: "本地桌面自动化场景建议开启。",
    },
    CodexFeatureGuideItem {
        key: "image_generation",
        label: "Image generation",
        stage: "stable",
        default_enabled: true,
        recommended_enabled: true,
        summary: "允许 Codex 使用图片生成/编辑能力，适合内容、设计和素材工作流。",
        config_note: "如果不希望请求图像生成模型，可以关闭。",
    },
    CodexFeatureGuideItem {
        key: "enable_request_compression",
        label: "Request compression",
        stage: "stable",
        default_enabled: true,
        recommended_enabled: true,
        summary: "对发往 Codex backend 的请求体启用 zstd 压缩，降低大上下文传输成本。",
        config_note: "StaticFlow gateway 已支持 zstd 解压，建议保持开启。",
    },
    CodexFeatureGuideItem {
        key: "tool_call_mcp_elicitation",
        label: "MCP elicitation",
        stage: "stable",
        default_enabled: true,
        recommended_enabled: true,
        summary: "允许 MCP 工具在调用过程中发起补充确认或参数征询。",
        config_note: "MCP 工作流较多时保留开启。",
    },
    CodexFeatureGuideItem {
        key: "personality",
        label: "Personality",
        stage: "stable",
        default_enabled: true,
        recommended_enabled: true,
        summary: "启用新版人格/行为配置能力，让不同使用模式更一致。",
        config_note: "保持默认开启即可。",
    },
    CodexFeatureGuideItem {
        key: "fast_mode",
        label: "Fast mode",
        stage: "stable",
        default_enabled: true,
        recommended_enabled: true,
        summary: "启用 Codex 的快速模式入口，适合低风险、低延迟任务。",
        config_note: "需要更保守推理时可按会话选择更高 reasoning，而不是关闭此功能。",
    },
];

fn selected_feature_keys() -> Vec<String> {
    CODEX_FEATURE_CATALOG
        .iter()
        .filter(|feature| feature.recommended_enabled)
        .map(|feature| feature.key.to_string())
        .collect()
}

fn feature_selected(selected_keys: &[&str], key: &str) -> bool {
    selected_keys.contains(&key)
}

fn codex_feature_config_snippet(selected_keys: &[&str]) -> String {
    let mut config = format!(
        "# Codex feature snapshot: {CODEX_FEATURE_SNAPSHOT_VERSION}, reviewed \
         {CODEX_FEATURE_SNAPSHOT_DATE}\n[features]\n"
    );
    for feature in CODEX_FEATURE_CATALOG {
        let enabled = feature_selected(selected_keys, feature.key);
        config.push_str(&format!("{} = {}\n", feature.key, enabled));
    }
    if feature_selected(selected_keys, "memories") {
        config.push_str("\n[memories]\nuse_memories = true\ngenerate_memories = true\n");
    }
    config
}

fn codex_feature_cli_commands(selected_keys: &[&str]) -> String {
    let mut commands = String::from("# Enable selected Codex features\n");
    for feature in CODEX_FEATURE_CATALOG {
        if feature_selected(selected_keys, feature.key) {
            commands.push_str(&format!("codex features enable {}\n", feature.key));
        }
    }
    commands.push_str("codex features list\n");
    commands
}

fn codex_feature_toggle_label(expanded: bool) -> &'static str {
    if expanded {
        "收起清单"
    } else {
        "展开清单"
    }
}

#[derive(Properties, PartialEq)]
struct GuideCodePanelProps {
    eyebrow: AttrValue,
    title: AttrValue,
    button_label: AttrValue,
    copy_label: AttrValue,
    code: String,
    on_copy: Callback<(String, String)>,
}

#[cfg(test)]
mod tests {
    use super::{
        codex_feature_cli_commands, codex_feature_config_snippet, codex_feature_toggle_label,
    };

    #[test]
    fn codex_feature_config_marks_selected_features_and_memory_settings() {
        let config = codex_feature_config_snippet(&["memories", "tool_search"]);

        assert!(config.contains("[features]"));
        assert!(config.contains("memories = true"));
        assert!(config.contains("tool_search = true"));
        assert!(config.contains("js_repl = false"));
        assert!(config.contains("[memories]"));
        assert!(config.contains("use_memories = true"));
        assert!(config.contains("generate_memories = true"));
    }

    #[test]
    fn codex_feature_cli_commands_follow_catalog_order_and_ignore_unknown_keys() {
        let commands = codex_feature_cli_commands(&["unknown_feature", "tool_search", "memories"]);

        assert!(commands.contains("codex features enable memories"));
        assert!(commands.contains("codex features enable tool_search"));
        assert!(!commands.contains("unknown_feature"));
        assert!(
            commands
                .find("codex features enable memories")
                .expect("memory command should be present")
                < commands
                    .find("codex features enable tool_search")
                    .expect("tool search command should be present")
        );
    }

    #[test]
    fn codex_feature_toggle_label_matches_expanded_state() {
        assert_eq!(codex_feature_toggle_label(false), "展开清单");
        assert_eq!(codex_feature_toggle_label(true), "收起清单");
    }
}

#[function_component(GuideCodePanel)]
fn guide_code_panel(props: &GuideCodePanelProps) -> Html {
    html! {
        <section class={classes!("rounded-xl", "border", "border-[var(--border)]", "bg-[var(--surface)]", "p-4")}>
            <div class={classes!("flex", "items-center", "justify-between", "gap-3", "flex-wrap")}>
                <div>
                    <span class={classes!("text-xs", "uppercase", "tracking-widest", "text-[var(--muted)]")}>{ props.eyebrow.clone() }</span>
                    <h4 class={classes!("m-0", "mt-1", "text-sm", "font-bold", "text-[var(--text)]")}>{ props.title.clone() }</h4>
                </div>
                <button
                    class={classes!("btn-terminal", "btn-terminal-primary", "!text-xs")}
                    onclick={{
                        let label = props.copy_label.to_string();
                        let code = props.code.clone();
                        let on_copy = props.on_copy.clone();
                        Callback::from(move |_| on_copy.emit((label.clone(), code.clone())))
                    }}
                >
                    { props.button_label.clone() }
                </button>
            </div>
            <pre class={classes!("mt-3", "overflow-x-auto", "rounded-lg", "bg-slate-950", "p-3", "text-xs", "leading-6", "text-emerald-200")}>
                { props.code.clone() }
            </pre>
        </section>
    }
}

#[function_component(LlmAccessGuidePage)]
pub fn llm_access_guide_page() -> Html {
    let access = use_state(|| None::<LlmGatewayAccessResponse>);
    let model_catalog_json = use_state(|| None::<String>);
    let model_catalog_error = use_state(|| None::<String>);
    let loading = use_state(|| true);
    let error = use_state(|| None::<String>);
    let toast = use_state(|| None::<(String, bool)>);
    let toast_timeout = use_mut_ref(|| None::<Timeout>);
    let selected_features = use_state(selected_feature_keys);
    let feature_section_expanded = use_state(|| false);

    {
        let access = access.clone();
        let model_catalog_json = model_catalog_json.clone();
        let model_catalog_error = model_catalog_error.clone();
        let loading = loading.clone();
        let error = error.clone();
        use_effect_with((), move |_| {
            wasm_bindgen_futures::spawn_local(async move {
                match fetch_llm_gateway_access().await {
                    Ok(data) => {
                        match fetch_llm_gateway_model_catalog_json(Some(&data.model_catalog_path))
                            .await
                        {
                            Ok(raw) => {
                                model_catalog_json.set(Some(raw));
                                model_catalog_error.set(None);
                            },
                            Err(err) => {
                                model_catalog_json.set(None);
                                model_catalog_error.set(Some(err));
                            },
                        }
                        access.set(Some(data));
                        error.set(None);
                    },
                    Err(err) => {
                        access.set(None);
                        model_catalog_json.set(None);
                        model_catalog_error.set(None);
                        error.set(Some(err));
                    },
                }
                loading.set(false);
            });
            || ()
        });
    }

    let on_copy = {
        let toast = toast.clone();
        let toast_timeout = toast_timeout.clone();
        Callback::from(move |(label, value): (String, String)| {
            copy_text(&value);
            toast.set(Some((format!("已复制{}", label), false)));
            toast_timeout.borrow_mut().take();
            let toast = toast.clone();
            let clear_handle = toast_timeout.clone();
            let timeout = Timeout::new(1800, move || {
                toast.set(None);
                clear_handle.borrow_mut().take();
            });
            *toast_timeout.borrow_mut() = Some(timeout);
        })
    };

    let content = if *loading {
        html! {
            <div class={classes!("mt-10", "rounded-xl", "border", "border-dashed", "border-[var(--border)]", "px-5", "py-12", "text-center", "text-[var(--muted)]")}>
                { "正在读取接入信息" }
            </div>
        }
    } else if let Some(err) = (*error).clone() {
        html! {
            <div class={classes!("mt-10", "rounded-xl", "border", "border-red-400/35", "bg-red-500/8", "px-5", "py-5", "text-sm", "text-red-700", "dark:text-red-200")}>
                { err }
            </div>
        }
    } else if let Some(access) = (*access).clone() {
        let base_url = resolved_base_url(&access);
        let model_catalog_url = resolved_model_catalog_url(&access);
        let example_key = example_key_secret(&access);
        let example_key_name = example_key_name(&access);
        let default_model = (*model_catalog_json)
            .as_deref()
            .and_then(preferred_model_slug_from_catalog_json)
            .unwrap_or_else(|| "gpt-5.5".to_string());
        let provider_config = codex_provider_config(&base_url, &default_model);
        let model_catalog_download_command =
            codex_model_catalog_download_command(&model_catalog_url);
        let login_command = codex_login_command();
        let auth_json = codex_auth_json(&example_key);
        let curl_example = chat_curl_example(&base_url, &example_key, &default_model);
        let python_example = chat_python_example(&base_url, &example_key, &default_model);
        let selected_feature_values = (*selected_features).clone();
        let selected_feature_refs = selected_feature_values
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>();
        let feature_config = codex_feature_config_snippet(&selected_feature_refs);
        let feature_commands = codex_feature_cli_commands(&selected_feature_refs);
        let selected_feature_count = selected_feature_values.len();
        let selected_feature_preview = selected_feature_values
            .iter()
            .take(6)
            .cloned()
            .collect::<Vec<_>>();
        let hidden_selected_feature_count =
            selected_feature_count.saturating_sub(selected_feature_preview.len());
        let feature_section_is_expanded = *feature_section_expanded;
        let feature_toggle_label = codex_feature_toggle_label(feature_section_is_expanded);
        let on_toggle_feature_section = {
            let feature_section_expanded = feature_section_expanded.clone();
            Callback::from(move |_| feature_section_expanded.set(!*feature_section_expanded))
        };

        html! {
            <>
                // Page header
                <section class={classes!("mt-8", "rounded-xl", "border", "border-[var(--border)]", "bg-[var(--surface)]", "p-5")}>
                    <div class={classes!("flex", "items-start", "justify-between", "gap-4", "flex-wrap")}>
                        <div>
                            <h1 class={classes!("m-0", "text-2xl", "font-bold", "text-[var(--text)]")}>
                                { "接入 Codex / 养龙虾🦞" }
                            </h1>
                            <p class={classes!("mt-2", "m-0", "text-sm", "text-[var(--muted)]")}>
                                { format!("示例 Key: {} · Codex feature snapshot {}", example_key_name, CODEX_FEATURE_SNAPSHOT_VERSION) }
                            </p>
                        </div>
                        <div class={classes!("flex", "items-center", "gap-2")}>
                            <Link<Route> to={Route::LlmAccess} classes={classes!("btn-terminal")}>
                                <i class="fas fa-arrow-left"></i>
                                { "Key 大厅" }
                            </Link<Route>>
                            <button
                                class={classes!("btn-terminal", "btn-terminal-primary")}
                                onclick={{
                                    let on_copy = on_copy.clone();
                                    let base_url = base_url.clone();
                                    Callback::from(move |_| on_copy.emit(("Base URL".to_string(), base_url.clone())))
                                }}
                            >
                                <i class="fas fa-copy"></i>
                                { "复制 URL" }
                            </button>
                        </div>
                    </div>
                </section>

                // Notice bar
                <div class={classes!("mt-4", "llm-access-notice")}>
                    { "保住 remote compact 是接 Codex 的前提 — " }
                    <Link<Route>
                        to={Route::ArticleDetail { id: REMOTE_COMPACT_ARTICLE_ID.to_string() }}
                        classes={classes!("underline", "text-[var(--primary)]")}
                    >
                        { "深潜文章" }
                    </Link<Route>>
                </div>

                // Step 01: model_catalog.json
                <section class={classes!("mt-6", "rounded-xl", "border", "border-[var(--border)]", "bg-[var(--surface)]", "p-5")}>
                    <div class={classes!("flex", "items-center", "gap-2")}>
                        <span class={classes!("text-xs", "font-semibold", "uppercase", "tracking-widest", "text-[var(--primary)]")}>{ "Step 01" }</span>
                        <h2 class={classes!("m-0", "text-lg", "font-bold", "text-[var(--text)]")}>{ "写入 model_catalog.json" }</h2>
                    </div>
                    <p class={classes!("mt-3", "mb-0", "text-sm", "text-[var(--muted)]")}>
                        { "先执行下面这条命令，它会把后端当前可用模型直接写到 ~/.codex/model_catalog.json。" }
                    </p>
                    <div class={classes!("mt-4")}>
                        <GuideCodePanel
                            eyebrow={"推荐"}
                            title={"一键下载命令"}
                            button_label={"复制"}
                            copy_label={"model_catalog 下载命令"}
                            code={model_catalog_download_command.clone()}
                            on_copy={on_copy.clone()}
                        />
                    </div>
                    if let Some(err) = (*model_catalog_error).clone() {
                        <p class={classes!("mt-3", "mb-0", "text-sm", "text-red-600", "dark:text-red-300")}>
                            { format!("model_catalog.json 拉取失败：{err}") }
                        </p>
                    }
                </section>

                // Step 02: Provider config
                <section class={classes!("mt-4", "rounded-xl", "border", "border-[var(--border)]", "bg-[var(--surface)]", "p-5")}>
                    <div class={classes!("flex", "items-center", "gap-2")}>
                        <span class={classes!("text-xs", "font-semibold", "uppercase", "tracking-widest", "text-[var(--primary)]")}>{ "Step 02" }</span>
                        <h2 class={classes!("m-0", "text-lg", "font-bold", "text-[var(--text)]")}>{ "配置 Provider" }</h2>
                    </div>
                    <p class={classes!("mt-3", "mb-0", "text-sm", "text-[var(--muted)]")}>
                        { format!("当前推荐默认模型：{}", default_model) }
                    </p>
                    <div class={classes!("mt-4")}>
                        <GuideCodePanel
                            eyebrow={"~/.codex/config.toml"}
                            title={"Provider 配置"}
                            button_label={"复制"}
                            copy_label={"provider 配置"}
                            code={provider_config.clone()}
                            on_copy={on_copy.clone()}
                        />
                    </div>
                </section>

                // Step 03: Codex features
                <section class={classes!("mt-4", "rounded-xl", "border", "border-[var(--border)]", "bg-[var(--surface)]", "p-5")}>
                    <div class={classes!("flex", "items-start", "justify-between", "gap-4", "flex-wrap")}>
                        <div>
                            <div class={classes!("flex", "items-center", "gap-2")}>
                                <span class={classes!("text-xs", "font-semibold", "uppercase", "tracking-widest", "text-[var(--primary)]")}>{ "Step 03" }</span>
                                <h2 class={classes!("m-0", "text-lg", "font-bold", "text-[var(--text)]")}>{ "选择 Codex Feature" }</h2>
                            </div>
                            <p class={classes!("mt-3", "mb-0", "max-w-3xl", "text-sm", "leading-6", "text-[var(--muted)]")}>
                                { format!("清单基于 Codex {} 的 features registry，最近核对于 {}。选择后复制 config.toml 片段；页面不会直接写你的本机配置。", CODEX_FEATURE_SNAPSHOT_VERSION, CODEX_FEATURE_SNAPSHOT_DATE) }
                            </p>
                        </div>
                        <div class={classes!("flex", "items-center", "gap-2", "flex-wrap")}>
                            <span class={classes!("rounded-full", "border", "border-[var(--border)]", "px-3", "py-1.5", "font-mono", "text-xs", "text-[var(--muted)]")}>
                                { format!("selected {selected_feature_count}/{}", CODEX_FEATURE_CATALOG.len()) }
                            </span>
                            <button
                                type="button"
                                class={classes!("btn-terminal", "btn-terminal-primary")}
                                aria-expanded={feature_section_is_expanded.to_string()}
                                onclick={on_toggle_feature_section}
                            >
                                <i class={classes!("fas", if feature_section_is_expanded { "fa-chevron-up" } else { "fa-chevron-down" })}></i>
                                { feature_toggle_label }
                            </button>
                        </div>
                    </div>

                    <div class={classes!("mt-4", "rounded-lg", "border", "border-dashed", "border-[var(--border)]", "bg-[var(--surface-alt)]", "p-3")}>
                        <div class={classes!("flex", "items-center", "justify-between", "gap-3", "flex-wrap")}>
                            <span class={classes!("text-xs", "font-semibold", "uppercase", "tracking-widest", "text-[var(--muted)]")}>
                                { "当前推荐组合" }
                            </span>
                            if !feature_section_is_expanded {
                                <span class={classes!("text-xs", "text-[var(--muted)]")}>{ "展开后可逐项勾选并复制配置。" }</span>
                            }
                        </div>
                        <div class={classes!("mt-3", "flex", "items-center", "gap-2", "flex-wrap")}>
                            { for selected_feature_preview.iter().map(|key| html! {
                                <span class={classes!("rounded-full", "border", "border-emerald-400/45", "bg-emerald-500/8", "px-2.5", "py-1", "font-mono", "text-[11px]", "text-[var(--text)]")}>
                                    { key }
                                </span>
                            }) }
                            if hidden_selected_feature_count > 0 {
                                <span class={classes!("rounded-full", "border", "border-[var(--border)]", "px-2.5", "py-1", "font-mono", "text-[11px]", "text-[var(--muted)]")}>
                                    { format!("+{hidden_selected_feature_count}") }
                                </span>
                            }
                        </div>
                    </div>

                    if feature_section_is_expanded {
                        <>
                            <div class={classes!("mt-5", "grid", "gap-3", "md:grid-cols-2")}>
                                { for CODEX_FEATURE_CATALOG.iter().map(|feature| {
                                    let enabled = selected_feature_values.iter().any(|key| key == feature.key);
                                    let feature_key = feature.key.to_string();
                                    let selected_features = selected_features.clone();
                                    html! {
                                        <label class={classes!(
                                            "flex", "h-full", "cursor-pointer", "items-start", "gap-3",
                                            "rounded-lg", "border", "p-4", "transition-colors", "duration-150",
                                            if enabled {
                                                classes!("border-emerald-400/60", "bg-emerald-500/8")
                                            } else {
                                                classes!("border-[var(--border)]", "bg-[var(--surface-alt)]", "hover:border-[var(--primary)]/45")
                                            }
                                        )}>
                                            <input
                                                type="checkbox"
                                                class={classes!("mt-1", "h-4", "w-4", "shrink-0", "accent-emerald-500")}
                                                checked={enabled}
                                                onchange={{
                                                    Callback::from(move |_| {
                                                        let mut next = (*selected_features).clone();
                                                        if next.iter().any(|key| key == &feature_key) {
                                                            next.retain(|key| key != &feature_key);
                                                        } else {
                                                            next.push(feature_key.clone());
                                                        }
                                                        selected_features.set(next);
                                                    })
                                                }}
                                            />
                                            <span class={classes!("min-w-0", "flex-1")}>
                                                <span class={classes!("flex", "items-center", "justify-between", "gap-2", "flex-wrap")}>
                                                    <span class={classes!("font-mono", "text-sm", "font-bold", "text-[var(--text)]")}>{ feature.key }</span>
                                                    <span class={classes!("flex", "items-center", "gap-1.5", "text-[10px]", "uppercase", "tracking-wide", "text-[var(--muted)]")}>
                                                        <span class={classes!("rounded-full", "border", "border-[var(--border)]", "px-2", "py-0.5")}>{ feature.stage }</span>
                                                        <span class={classes!("rounded-full", "border", "border-[var(--border)]", "px-2", "py-0.5")}>
                                                            { if feature.default_enabled { "default on" } else { "default off" } }
                                                        </span>
                                                    </span>
                                                </span>
                                                <span class={classes!("mt-1", "block", "text-sm", "font-semibold", "text-[var(--text)]")}>{ feature.label }</span>
                                                <span class={classes!("mt-2", "block", "text-xs", "leading-5", "text-[var(--muted)]")}>{ feature.summary }</span>
                                                <span class={classes!("mt-2", "block", "text-[11px]", "leading-5", "text-[var(--muted)]")}>{ feature.config_note }</span>
                                            </span>
                                        </label>
                                    }
                                }) }
                            </div>

                            <div class={classes!("mt-5", "grid", "gap-3", "xl:grid-cols-2")}>
                                <GuideCodePanel
                                    eyebrow={"~/.codex/config.toml"}
                                    title={"Feature 配置片段"}
                                    button_label={"复制"}
                                    copy_label={"Codex feature 配置"}
                                    code={feature_config.clone()}
                                    on_copy={on_copy.clone()}
                                />
                                <GuideCodePanel
                                    eyebrow={"CLI"}
                                    title={"启用所选 Feature"}
                                    button_label={"复制"}
                                    copy_label={"Codex feature 命令"}
                                    code={feature_commands.clone()}
                                    on_copy={on_copy.clone()}
                                />
                            </div>
                        </>
                    }
                </section>

                // Step 04: Auth
                <section class={classes!("mt-4", "rounded-xl", "border", "border-[var(--border)]", "bg-[var(--surface)]", "p-5")}>
                    <div class={classes!("flex", "items-center", "gap-2")}>
                        <span class={classes!("text-xs", "font-semibold", "uppercase", "tracking-widest", "text-[var(--primary)]")}>{ "Step 04" }</span>
                        <h2 class={classes!("m-0", "text-lg", "font-bold", "text-[var(--text)]")}>{ "写入 Key" }</h2>
                    </div>
                    <div class={classes!("mt-4", "grid", "gap-3", "xl:grid-cols-2")}>
                        <GuideCodePanel
                            eyebrow={"推荐"}
                            title={"codex login --with-api-key"}
                            button_label={"复制"}
                            copy_label={"登录命令"}
                            code={login_command.clone()}
                            on_copy={on_copy.clone()}
                        />
                        <GuideCodePanel
                            eyebrow={"备用"}
                            title={"手写 auth.json"}
                            button_label={"复制"}
                            copy_label={"auth.json"}
                            code={auth_json.clone()}
                            on_copy={on_copy.clone()}
                        />
                    </div>
                </section>

                // Step 05: Usage
                <section class={classes!("mt-4", "rounded-xl", "border", "border-[var(--border)]", "bg-[var(--surface)]", "p-5")}>
                    <div class={classes!("flex", "items-center", "gap-2")}>
                        <span class={classes!("text-xs", "font-semibold", "uppercase", "tracking-widest", "text-[var(--primary)]")}>{ "Step 05" }</span>
                        <h2 class={classes!("m-0", "text-lg", "font-bold", "text-[var(--text)]")}>{ "开始使用" }</h2>
                    </div>
                    <div class={classes!("mt-4", "grid", "gap-3", "xl:grid-cols-2")}>
                        <GuideCodePanel
                            eyebrow={"curl"}
                            title={"最小请求示例"}
                            button_label={"复制"}
                            copy_label={"curl 示例"}
                            code={curl_example.clone()}
                            on_copy={on_copy.clone()}
                        />
                        <GuideCodePanel
                            eyebrow={"Python"}
                            title={"OpenAI SDK 风格"}
                            button_label={"复制"}
                            copy_label={"Python 示例"}
                            code={python_example.clone()}
                            on_copy={on_copy.clone()}
                        />
                    </div>
                </section>

                // Back to keys
                <section class={classes!("mt-4", "flex", "items-center", "justify-between", "gap-4", "rounded-xl", "border", "border-[var(--border)]", "bg-[var(--surface)]", "p-5", "flex-wrap")}>
                    <h2 class={classes!("m-0", "text-lg", "font-bold", "text-[var(--text)]")}>
                        { "配好了，回去复制 Key" }
                    </h2>
                    <Link<Route> to={Route::LlmAccess} classes={classes!("btn-terminal", "btn-terminal-primary")}>
                        <i class="fas fa-key"></i>
                        { "Key 大厅" }
                    </Link<Route>>
                </section>
            </>
        }
    } else {
        Html::default()
    };

    html! {
        <main class={classes!("relative", "min-h-screen", "bg-[var(--bg)]")}>
            <div class={classes!("relative", "mx-auto", "max-w-5xl", "px-4", "pb-16", "pt-8", "lg:px-6")}>
                { content }
            </div>

            if let Some((message, is_error)) = (*toast).clone() {
                <div class={classes!(
                    "fixed", "bottom-5", "right-5", "z-[90]",
                    "rounded-full", "border", "px-4", "py-3",
                    "text-sm", "font-semibold",
                    "shadow-[0_8px_24px_rgba(0,0,0,0.15)]",
                    if is_error {
                        classes!("border-red-400/35", "bg-red-500/92", "text-white")
                    } else {
                        classes!("border-emerald-400/35", "bg-emerald-500/92", "text-white")
                    }
                )}>
                    { message }
                </div>
            }
        </main>
    }
}
