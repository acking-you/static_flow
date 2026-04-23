use std::{cell::RefCell, rc::Rc};

use wasm_bindgen_futures::spawn_local;
use web_sys::{HtmlSelectElement, HtmlTextAreaElement};
use yew::prelude::*;
use yew_router::prelude::*;

use crate::{
    api::{verify_public_gpt2api_key, Gpt2ApiPublicKeyInfo, API_BASE},
    pages::gpt2api_public_shared::{
        build_conversation_title, clear_auth_key, clear_chat_conversations, create_client_id,
        delete_chat_conversation, format_conversation_time, list_chat_conversations, load_auth_key,
        nav_shell, now_ms, save_chat_conversation, stream_chat_completion,
        StoredGpt2ApiChatConversation,
    },
    router::Route,
    utils::markdown_to_html,
};

const CHAT_MODEL_OPTIONS: &[&str] = &["auto", "gpt-5", "gpt-5-mini"];

fn extract_delta_text(payload: &str) -> Option<String> {
    if payload.trim() == "[DONE]" {
        return None;
    }
    let value: serde_json::Value = serde_json::from_str(payload).ok()?;
    value
        .pointer("/choices/0/delta/content")
        .and_then(serde_json::Value::as_str)
        .map(ToString::to_string)
}

fn replace_chat_conversation(
    items: &[StoredGpt2ApiChatConversation],
    conversation: StoredGpt2ApiChatConversation,
) -> Vec<StoredGpt2ApiChatConversation> {
    let mut next = items
        .iter()
        .filter(|item| item.id != conversation.id)
        .cloned()
        .collect::<Vec<_>>();
    next.push(conversation);
    next.sort_by(|left, right| right.created_at_ms.cmp(&left.created_at_ms));
    next
}

fn recover_chat_history(
    items: Vec<StoredGpt2ApiChatConversation>,
) -> (Vec<StoredGpt2ApiChatConversation>, bool) {
    let mut changed = false;
    let mut next = Vec::with_capacity(items.len());
    for mut conversation in items {
        if conversation.status == "streaming" {
            changed = true;
            conversation.status = "error".to_string();
            if conversation.error.is_none() {
                conversation.error = Some(if conversation.answer.trim().is_empty() {
                    "页面已刷新，对话已中断".to_string()
                } else {
                    "页面已刷新，流式输出已中断".to_string()
                });
            }
        }
        next.push(conversation);
    }
    next.sort_by(|left, right| right.created_at_ms.cmp(&left.created_at_ms));
    (next, changed)
}

#[function_component(Gpt2ApiChatPage)]
pub fn gpt2api_chat_page() -> Html {
    let navigator = use_navigator();
    let auth_key = use_state(String::new);
    let key_info = use_state(|| None::<Gpt2ApiPublicKeyInfo>);
    let auth_ready = use_state(|| false);
    let loading_key = use_state(|| false);
    let load_error = use_state(|| None::<String>);

    let conversations = use_state(Vec::<StoredGpt2ApiChatConversation>::new);
    let selected_conversation_id = use_state(|| None::<String>);
    let prompt = use_state(String::new);
    let model = use_state(|| CHAT_MODEL_OPTIONS[0].to_string());
    let is_loading_history = use_state(|| true);
    let streaming_ids = use_state(Vec::<String>::new);

    {
        let auth_key = auth_key.clone();
        let auth_ready = auth_ready.clone();
        let conversations = conversations.clone();
        let selected_conversation_id = selected_conversation_id.clone();
        let navigator = navigator.clone();
        let load_error = load_error.clone();
        let key_info = key_info.clone();
        let loading_key = loading_key.clone();
        let is_loading_history = is_loading_history.clone();
        use_effect_with((), move |_| {
            spawn_local(async move {
                match load_auth_key().await {
                    Ok(stored) if !stored.trim().is_empty() => {
                        auth_key.set(stored.clone());
                        loading_key.set(true);
                        match verify_public_gpt2api_key(&stored).await {
                            Ok(verified) => key_info.set(Some(verified.key)),
                            Err(err) => load_error.set(Some(err)),
                        }
                        loading_key.set(false);
                    },
                    Ok(_) => {
                        if let Some(navigator) = navigator.clone() {
                            navigator.replace(&Route::Gpt2ApiLogin);
                        }
                    },
                    Err(err) => load_error.set(Some(err)),
                }

                match list_chat_conversations().await {
                    Ok(items) => {
                        let (normalized, changed) = recover_chat_history(items);
                        if changed {
                            for item in &normalized {
                                let _ = save_chat_conversation(item).await;
                            }
                        }
                        if let Some(first) = normalized.first() {
                            selected_conversation_id.set(Some(first.id.clone()));
                        }
                        conversations.set(normalized);
                    },
                    Err(err) => load_error.set(Some(err)),
                }
                is_loading_history.set(false);
                auth_ready.set(true);
            });
            || ()
        });
    }

    let on_logout = {
        let navigator = navigator.clone();
        Callback::from(move |_| {
            let navigator = navigator.clone();
            spawn_local(async move {
                let _ = clear_auth_key().await;
                if let Some(navigator) = navigator {
                    navigator.replace(&Route::Gpt2ApiLogin);
                }
            });
        })
    };

    let on_create_draft = {
        let selected_conversation_id = selected_conversation_id.clone();
        let prompt = prompt.clone();
        Callback::from(move |_| {
            selected_conversation_id.set(None);
            prompt.set(String::new());
        })
    };

    let on_clear_history = {
        let conversations = conversations.clone();
        let selected_conversation_id = selected_conversation_id.clone();
        let load_error = load_error.clone();
        Callback::from(move |_| {
            let conversations = conversations.clone();
            let selected_conversation_id = selected_conversation_id.clone();
            let load_error = load_error.clone();
            spawn_local(async move {
                match clear_chat_conversations().await {
                    Ok(_) => {
                        conversations.set(Vec::new());
                        selected_conversation_id.set(None);
                        load_error.set(None);
                    },
                    Err(err) => load_error.set(Some(err)),
                }
            });
        })
    };

    let on_delete_conversation = {
        let conversations = conversations.clone();
        let selected_conversation_id = selected_conversation_id.clone();
        let load_error = load_error.clone();
        Callback::from(move |id: String| {
            let conversations = conversations.clone();
            let selected_conversation_id = selected_conversation_id.clone();
            let load_error = load_error.clone();
            spawn_local(async move {
                let next = (*conversations)
                    .iter()
                    .filter(|item| item.id != id)
                    .cloned()
                    .collect::<Vec<_>>();
                match delete_chat_conversation(&id).await {
                    Ok(_) => {
                        if (*selected_conversation_id).as_ref() == Some(&id) {
                            selected_conversation_id.set(next.first().map(|item| item.id.clone()));
                        }
                        conversations.set(next);
                    },
                    Err(err) => load_error.set(Some(err)),
                }
            });
        })
    };

    let on_submit = {
        let auth_key = auth_key.clone();
        let prompt = prompt.clone();
        let model = model.clone();
        let conversations = conversations.clone();
        let selected_conversation_id = selected_conversation_id.clone();
        let streaming_ids = streaming_ids.clone();
        let load_error = load_error.clone();
        let key_info = key_info.clone();
        Callback::from(move |_| {
            let auth_key_value = (*auth_key).trim().to_string();
            if auth_key_value.is_empty() {
                load_error.set(Some("请先登录公开 Key".to_string()));
                return;
            }
            let prompt_value = (*prompt).trim().to_string();
            if prompt_value.is_empty() {
                load_error.set(Some("请输入聊天内容".to_string()));
                return;
            }

            let conversation_id = create_client_id();
            let draft = StoredGpt2ApiChatConversation {
                id: conversation_id.clone(),
                title: build_conversation_title(&prompt_value, 12),
                created_at_ms: now_ms(),
                prompt: prompt_value.clone(),
                model: (*model).clone(),
                status: "streaming".to_string(),
                answer: String::new(),
                error: None,
            };

            let next = replace_chat_conversation(&conversations, draft.clone());
            conversations.set(next);
            selected_conversation_id.set(Some(conversation_id.clone()));
            prompt.set(String::new());
            load_error.set(None);

            let mut next_streaming = (*streaming_ids).clone();
            if !next_streaming.contains(&conversation_id) {
                next_streaming.push(conversation_id.clone());
            }
            streaming_ids.set(next_streaming);

            let conversations = conversations.clone();
            let load_error = load_error.clone();
            let streaming_ids = streaming_ids.clone();
            let key_info = key_info.clone();
            let model_value = (*model).clone();
            spawn_local(async move {
                let _ = save_chat_conversation(&draft).await;

                let buffer = Rc::new(RefCell::new(String::new()));
                let buffer_for_stream = buffer.clone();
                let conversations_for_stream = conversations.clone();
                let stream_conversation_id = conversation_id.clone();
                let url = format!("{}/gpt2api/chat/completions", API_BASE.trim_end_matches('/'));
                let body = serde_json::json!({
                    "model": model_value,
                    "stream": true,
                    "messages": [{ "role": "user", "content": prompt_value }],
                })
                .to_string();

                let stream_result =
                    stream_chat_completion(&url, &auth_key_value, &body, move |payload| {
                        if payload.trim() == "[DONE]" {
                            return;
                        }
                        let Some(delta) = extract_delta_text(&payload) else {
                            return;
                        };
                        buffer_for_stream.borrow_mut().push_str(&delta);
                        let answer = buffer_for_stream.borrow().clone();
                        let mut current = (*conversations_for_stream).clone();
                        if let Some(item) = current
                            .iter_mut()
                            .find(|item| item.id == stream_conversation_id)
                        {
                            item.answer = answer;
                        }
                        conversations_for_stream.set(current);
                    })
                    .await;

                let final_answer = buffer.borrow().clone();
                let updated = match stream_result {
                    Ok(_) => StoredGpt2ApiChatConversation {
                        status: "success".to_string(),
                        answer: final_answer,
                        ..draft.clone()
                    },
                    Err(err) => {
                        load_error.set(Some(err.clone()));
                        StoredGpt2ApiChatConversation {
                            status: "error".to_string(),
                            answer: final_answer,
                            error: Some(err),
                            ..draft.clone()
                        }
                    },
                };

                let next = replace_chat_conversation(&conversations, updated.clone());
                conversations.set(next);
                let _ = save_chat_conversation(&updated).await;

                let next_streaming = (*streaming_ids)
                    .iter()
                    .filter(|item| **item != conversation_id)
                    .cloned()
                    .collect::<Vec<_>>();
                streaming_ids.set(next_streaming);

                if let Ok(verified) = verify_public_gpt2api_key(&auth_key_value).await {
                    key_info.set(Some(verified.key));
                }
            });
        })
    };

    let selected_conversation = (*selected_conversation_id)
        .as_ref()
        .and_then(|id| conversations.iter().find(|item| &item.id == id))
        .cloned();
    let is_selected_streaming = selected_conversation
        .as_ref()
        .is_some_and(|item| streaming_ids.contains(&item.id));
    let has_any_streaming = !streaming_ids.is_empty();

    html! {
        <>
            { nav_shell("chat", on_logout) }
            <section class={classes!("mx-auto", "grid", "h-[calc(100vh-5rem)]", "min-h-0", "w-full", "max-w-[1380px]", "grid-cols-1", "gap-3", "px-3", "pb-6", "lg:grid-cols-[240px_minmax(0,1fr)]")}>
                <aside class={classes!("min-h-0", "border-r", "border-stone-200/70", "pr-3")}>
                    <div class={classes!("flex", "h-full", "min-h-0", "flex-col", "gap-3", "py-2")}>
                        <div class={classes!("flex", "items-center", "gap-2")}>
                            <button
                                type="button"
                                class={classes!("h-10", "flex-1", "rounded-xl", "bg-stone-950", "px-4", "text-sm", "font-medium", "text-white", "transition", "hover:bg-stone-800")}
                                onclick={on_create_draft}
                            >
                                { "新建对话" }
                            </button>
                            <button
                                type="button"
                                class={classes!("h-10", "rounded-xl", "border", "border-stone-200", "bg-white/85", "px-3", "text-sm", "text-stone-600", "transition", "hover:bg-white", "disabled:cursor-not-allowed", "disabled:opacity-50")}
                                onclick={on_clear_history}
                                disabled={conversations.is_empty()}
                            >
                                { "清空" }
                            </button>
                        </div>
                        <div class={classes!("min-h-0", "flex-1", "space-y-2", "overflow-y-auto", "pr-1")}>
                            if *is_loading_history {
                                <div class={classes!("px-2", "py-3", "text-sm", "text-stone-500")}>{ "正在读取会话记录" }</div>
                            } else if conversations.is_empty() {
                                <div class={classes!("px-2", "py-3", "text-sm", "leading-6", "text-stone-500")}>
                                    { "还没有聊天记录，发出第一条消息后会在这里显示。" }
                                </div>
                            } else {
                                { for conversations.iter().map(|conversation| {
                                    let conversation_id = conversation.id.clone();
                                    let select_id = selected_conversation_id.clone();
                                    let delete_id = conversation.id.clone();
                                    let on_delete_conversation = on_delete_conversation.clone();
                                    let streaming = streaming_ids.contains(&conversation.id);
                                    html! {
                                        <div
                                            class={classes!(
                                                "group",
                                                "relative",
                                                "w-full",
                                                "border-l-2",
                                                "px-3",
                                                "py-3",
                                                "text-left",
                                                "transition",
                                                if Some(conversation.id.clone()) == *selected_conversation_id {
                                                    "border-stone-900 bg-black/[0.03] text-stone-950"
                                                } else {
                                                    "border-transparent text-stone-700 hover:border-stone-300 hover:bg-white/40"
                                                }
                                            )}
                                        >
                                            <button
                                                type="button"
                                                class={classes!("block", "w-full", "pr-8", "text-left")}
                                                onclick={Callback::from(move |_| select_id.set(Some(conversation_id.clone())))}
                                            >
                                                <div class={classes!("flex", "items-center", "gap-1.5", "truncate", "text-sm", "font-semibold")}>
                                                    if streaming {
                                                        <span class={classes!("size-2", "rounded-full", "bg-amber-500")}></span>
                                                    }
                                                    <span class={classes!("truncate")}>{ conversation.title.clone() }</span>
                                                </div>
                                                <div class={classes!("mt-1", "text-xs", if Some(conversation.id.clone()) == *selected_conversation_id { "text-stone-500" } else { "text-stone-400" })}>
                                                    { format_conversation_time(conversation.created_at_ms) }
                                                </div>
                                            </button>
                                            <button
                                                type="button"
                                                class={classes!("absolute", "right-2", "top-3", "inline-flex", "size-7", "items-center", "justify-center", "rounded-md", "text-stone-400", "opacity-0", "transition", "hover:bg-stone-100", "hover:text-rose-500", "group-hover:opacity-100")}
                                                onclick={Callback::from(move |_| on_delete_conversation.emit(delete_id.clone()))}
                                            >
                                                { "×" }
                                            </button>
                                        </div>
                                    }
                                }) }
                            }
                        </div>
                    </div>
                </aside>

                <div class={classes!("flex", "min-h-0", "flex-col", "gap-4")}>
                    <div class={classes!("min-h-0", "flex-1", "overflow-y-auto", "px-2", "py-3", "sm:px-4", "sm:py-4")}>
                        if let Some(conversation) = selected_conversation.clone() {
                            <div class={classes!("mx-auto", "flex", "w-full", "max-w-[980px]", "flex-col", "gap-6")}>
                                <div class={classes!("flex", "justify-end")}>
                                    <div class={classes!("w-full", "max-w-[min(820px,92%)]", "px-1", "pt-1")}>
                                        <div class={classes!("ml-auto", "w-fit", "max-w-[min(40rem,100%)]", "whitespace-pre-wrap", "rounded-[24px]", "bg-stone-100", "px-5", "py-4", "text-[15px]", "leading-7", "text-stone-700")}>
                                            { conversation.prompt.clone() }
                                        </div>
                                    </div>
                                </div>

                                <div class={classes!("flex", "justify-start")}>
                                    <div class={classes!("w-full", "p-1")}>
                                        <div class={classes!("mb-4", "flex", "flex-wrap", "items-center", "gap-2", "text-xs", "text-stone-500")}>
                                            <span class={classes!("rounded-full", "bg-stone-100", "px-3", "py-1")}>{ conversation.model.clone() }</span>
                                            <span class={classes!("rounded-full", "bg-stone-100", "px-3", "py-1")}>{ format_conversation_time(conversation.created_at_ms) }</span>
                                            if is_selected_streaming {
                                                <span class={classes!("rounded-full", "bg-amber-50", "px-3", "py-1", "text-amber-700")}>{ "处理中" }</span>
                                            }
                                        </div>
                                        <article class={classes!("rounded-[26px]", "border", "border-stone-200", "bg-white", "px-5", "py-5", "shadow-sm")}>
                                            if conversation.answer.trim().is_empty() {
                                                <div class={classes!("min-h-[220px]", "text-sm", "leading-7", "text-stone-500")}>
                                                    { if is_selected_streaming { "等待上游返回..." } else { "当前对话还没有返回内容。" } }
                                                </div>
                                            } else {
                                                <div
                                                    class={classes!("article-content", "min-h-[220px]", "text-sm", "leading-7", "text-stone-800")}
                                                >
                                                    { Html::from_html_unchecked(AttrValue::from(markdown_to_html(&conversation.answer))) }
                                                </div>
                                            }
                                            if let Some(error_message) = conversation.error.clone() {
                                                <div class={classes!("mt-4", "rounded-[18px]", "bg-rose-50/80", "px-4", "py-3", "text-sm", "leading-6", "text-rose-600")}>
                                                    { error_message }
                                                </div>
                                            }
                                        </article>
                                    </div>
                                </div>
                            </div>
                        } else {
                            <div class={classes!("flex", "h-full", "min-h-[420px]", "items-center", "justify-center", "text-center")}>
                                <div class={classes!("w-full", "max-w-4xl")}>
                                    <h1
                                        class={classes!("text-3xl", "font-semibold", "tracking-tight", "text-stone-950", "md:text-5xl")}
                                        style={"font-family: \"Palatino Linotype\",\"Book Antiqua\",\"URW Palladio L\",\"Times New Roman\",serif;"}
                                    >
                                        { "Single-turn chat workspace" }
                                    </h1>
                                    <p
                                        class={classes!("mt-4", "text-[15px]", "italic", "tracking-[0.01em]", "text-stone-500")}
                                        style={"font-family: \"Palatino Linotype\",\"Book Antiqua\",\"URW Palladio L\",\"Times New Roman\",serif;"}
                                    >
                                        { "Pick a model, send a prompt, and let the stream fall straight into the page." }
                                    </p>
                                </div>
                            </div>
                        }
                    </div>

                    <div class={classes!("shrink-0", "flex", "justify-center")}>
                        <div class={classes!("w-full", "max-w-[980px]")}>
                            <div class={classes!("overflow-hidden", "rounded-[32px]", "border", "border-stone-200", "bg-white")}>
                                <div class={classes!("relative", "cursor-text")}>
                                    <textarea
                                        value={(*prompt).clone()}
                                        class={classes!("min-h-[148px]", "w-full", "resize-none", "border-0", "bg-transparent", "px-6", "pb-20", "pt-6", "text-[15px]", "leading-7", "text-stone-900", "outline-none", "placeholder:text-stone-400")}
                                        placeholder="输入你要发送的内容"
                                        oninput={{
                                            let prompt = prompt.clone();
                                            Callback::from(move |event: InputEvent| {
                                                prompt.set(event.target_unchecked_into::<HtmlTextAreaElement>().value());
                                            })
                                        }}
                                        onkeydown={{
                                            let on_submit = on_submit.clone();
                                            Callback::from(move |event: KeyboardEvent| {
                                                if event.key() == "Enter" && !event.shift_key() {
                                                    event.prevent_default();
                                                    on_submit.emit(());
                                                }
                                            })
                                        }}
                                    />
                                    <div class={classes!("absolute", "inset-x-0", "bottom-0", "bg-gradient-to-t", "from-white", "via-white/95", "to-transparent", "px-4", "pb-4", "pt-6", "sm:px-6")}>
                                        <div class={classes!("flex", "items-end", "justify-between", "gap-3")}>
                                            <div class={classes!("flex", "min-w-0", "flex-1", "flex-wrap", "items-center", "gap-3")}>
                                                if let Some(info) = (*key_info).clone() {
                                                    <div class={classes!("rounded-full", "bg-stone-100", "px-3", "py-2", "text-xs", "font-medium", "text-stone-600")}>
                                                        { format!("剩余额度 {}", info.quota_total_calls.saturating_sub(info.quota_used_calls)) }
                                                    </div>
                                                }
                                                if has_any_streaming {
                                                    <div class={classes!("rounded-full", "bg-amber-50", "px-3", "py-2", "text-xs", "font-medium", "text-amber-700")}>
                                                        { format!("{} 个会话进行中", streaming_ids.len()) }
                                                    </div>
                                                }
                                                <select
                                                    class={classes!("h-10", "w-[164px]", "rounded-full", "border", "border-stone-200", "bg-white", "px-4", "text-sm", "font-medium", "text-stone-700", "outline-none")}
                                                    value={(*model).clone()}
                                                    onchange={{
                                                        let model = model.clone();
                                                        Callback::from(move |event: Event| {
                                                            model.set(event.target_unchecked_into::<HtmlSelectElement>().value());
                                                        })
                                                    }}
                                                >
                                                    { for CHAT_MODEL_OPTIONS.iter().map(|model_name| html! {
                                                        <option value={model_name.to_string()}>{ model_name.to_string() }</option>
                                                    }) }
                                                </select>
                                            </div>
                                            <button
                                                type="button"
                                                class={classes!("inline-flex", "size-11", "shrink-0", "items-center", "justify-center", "rounded-full", "bg-stone-950", "text-white", "transition", "hover:bg-stone-800", "disabled:cursor-not-allowed", "disabled:bg-stone-300")}
                                                onclick={{
                                                    let on_submit = on_submit.clone();
                                                    Callback::from(move |_| on_submit.emit(()))
                                                }}
                                                disabled={prompt.trim().is_empty() || !*auth_ready || *loading_key || has_any_streaming}
                                            >
                                                { "↑" }
                                            </button>
                                        </div>
                                    </div>
                                </div>
                            </div>
                            if let Some(message) = (*load_error).clone() {
                                <div class={classes!("mt-3", "rounded-[18px]", "bg-rose-50/80", "px-4", "py-3", "text-sm", "leading-6", "text-rose-600")}>
                                    { message }
                                </div>
                            }
                        </div>
                    </div>
                </div>
            </section>
        </>
    }
}
