use wasm_bindgen::JsCast;
use wasm_bindgen_futures::spawn_local;
use web_sys::{ClipboardEvent, File, HtmlInputElement, HtmlSelectElement, HtmlTextAreaElement};
use yew::prelude::*;
use yew_router::prelude::*;

use crate::{
    api::{
        edit_public_gpt2api_images, generate_public_gpt2api_images, verify_public_gpt2api_key,
        AdminGpt2ApiRsImageGenerationRequest, Gpt2ApiPublicKeyInfo,
    },
    pages::gpt2api_public_shared::{
        build_conversation_title, clear_auth_key, clear_image_conversations, create_client_id,
        delete_image_conversation, file_to_data_url, format_conversation_time,
        list_image_conversations, load_auth_key, nav_shell, now_ms, save_image_conversation,
        StoredGpt2ApiImage, StoredGpt2ApiImageConversation, StoredGpt2ApiReferenceImage,
    },
    router::Route,
};

const IMAGE_MODEL_OPTIONS: &[&str] = &["gpt-image-1", "gpt-image-2"];

#[derive(Clone, PartialEq, Eq)]
struct LightboxImage {
    id: String,
    src: String,
}

fn replace_image_conversation(
    items: &[StoredGpt2ApiImageConversation],
    conversation: StoredGpt2ApiImageConversation,
) -> Vec<StoredGpt2ApiImageConversation> {
    let mut next = items
        .iter()
        .filter(|item| item.id != conversation.id)
        .cloned()
        .collect::<Vec<_>>();
    next.push(conversation);
    next.sort_by(|left, right| right.created_at_ms.cmp(&left.created_at_ms));
    next
}

fn recover_image_history(
    items: Vec<StoredGpt2ApiImageConversation>,
) -> (Vec<StoredGpt2ApiImageConversation>, bool) {
    let mut changed = false;
    let mut next = Vec::with_capacity(items.len());
    for mut conversation in items {
        let was_generating =
            conversation.status == "generating" || conversation.status == "running";
        let mut images_changed = false;
        let images = conversation
            .images
            .into_iter()
            .map(|mut image| {
                if image.status == "loading" {
                    images_changed = true;
                    image.status = "error".to_string();
                    image.error = Some("页面已刷新，生成已中断".to_string());
                }
                image
            })
            .collect::<Vec<_>>();
        if was_generating || images_changed {
            changed = true;
            conversation.status = "error".to_string();
            if conversation.error.is_none() {
                conversation.error =
                    Some(if images.iter().any(|image| image.status == "success") {
                        "生成已中断".to_string()
                    } else {
                        "页面已刷新，生成已中断".to_string()
                    });
            }
        }
        conversation.images = images;
        next.push(conversation);
    }
    next.sort_by(|left, right| right.created_at_ms.cmp(&left.created_at_ms));
    (next, changed)
}

async fn build_reference_previews(
    files: &[File],
) -> Result<Vec<StoredGpt2ApiReferenceImage>, String> {
    let mut previews = Vec::with_capacity(files.len());
    for file in files {
        previews.push(StoredGpt2ApiReferenceImage {
            name: file.name(),
            mime_type: file.type_(),
            data_url: file_to_data_url(file).await?,
        });
    }
    Ok(previews)
}

fn clear_file_input(input_ref: &NodeRef) {
    if let Some(input) = input_ref.cast::<HtmlInputElement>() {
        input.set_value("");
    }
}

fn image_lightbox(
    open: bool,
    images: &[LightboxImage],
    current_index: usize,
    on_close: Callback<MouseEvent>,
    on_prev: Callback<MouseEvent>,
    on_next: Callback<MouseEvent>,
) -> Html {
    if !open || images.is_empty() {
        return html! {};
    }
    let safe_index = current_index.min(images.len().saturating_sub(1));
    let current = images[safe_index].clone();
    let stop_bubble = Callback::from(|event: MouseEvent| event.stop_propagation());
    html! {
        <div
            class={classes!("fixed", "inset-0", "z-[120]", "flex", "items-center", "justify-center", "bg-black/80", "p-4")}
            onclick={on_close.clone()}
        >
            <div
                class={classes!("relative", "flex", "max-h-[92vh]", "w-full", "max-w-[1240px]", "items-center", "justify-center", "gap-4")}
                onclick={stop_bubble}
            >
                if images.len() > 1 {
                    <button
                        type="button"
                        class={classes!("inline-flex", "size-11", "items-center", "justify-center", "rounded-full", "bg-white/12", "text-2xl", "text-white", "transition", "hover:bg-white/20")}
                        onclick={on_prev}
                    >
                        { "‹" }
                    </button>
                }
                <img
                    src={current.src}
                    class={classes!("max-h-[90vh]", "max-w-[min(100%,1080px)]", "rounded-[24px]", "object-contain", "shadow-[0_28px_90px_rgba(0,0,0,0.45)]")}
                    alt="generated preview"
                />
                if images.len() > 1 {
                    <button
                        type="button"
                        class={classes!("inline-flex", "size-11", "items-center", "justify-center", "rounded-full", "bg-white/12", "text-2xl", "text-white", "transition", "hover:bg-white/20")}
                        onclick={on_next}
                    >
                        { "›" }
                    </button>
                }
                <button
                    type="button"
                    class={classes!("absolute", "right-2", "top-2", "inline-flex", "size-10", "items-center", "justify-center", "rounded-full", "bg-white/12", "text-white", "transition", "hover:bg-white/20")}
                    onclick={on_close}
                >
                    { "×" }
                </button>
            </div>
        </div>
    }
}

#[function_component(Gpt2ApiImagePage)]
pub fn gpt2api_image_page() -> Html {
    let navigator = use_navigator();
    let auth_key = use_state(String::new);
    let auth_ready = use_state(|| false);
    let key_info = use_state(|| None::<Gpt2ApiPublicKeyInfo>);
    let loading_key = use_state(|| false);
    let load_error = use_state(|| None::<String>);

    let conversations = use_state(Vec::<StoredGpt2ApiImageConversation>::new);
    let selected_conversation_id = use_state(|| None::<String>);
    let prompt = use_state(String::new);
    let image_mode = use_state(|| "generate".to_string());
    let image_model = use_state(|| IMAGE_MODEL_OPTIONS[0].to_string());
    let image_count = use_state(|| "1".to_string());
    let reference_files = use_state(Vec::<File>::new);
    let reference_images = use_state(Vec::<StoredGpt2ApiReferenceImage>::new);
    let is_loading_history = use_state(|| true);
    let generating_ids = use_state(Vec::<String>::new);

    let file_input_ref = use_node_ref();
    let lightbox_images = use_state(Vec::<LightboxImage>::new);
    let lightbox_index = use_state(|| 0usize);
    let lightbox_open = use_state(|| false);

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

                match list_image_conversations().await {
                    Ok(items) => {
                        let (normalized, changed) = recover_image_history(items);
                        if changed {
                            for item in &normalized {
                                let _ = save_image_conversation(item).await;
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
        let image_count = image_count.clone();
        let reference_files = reference_files.clone();
        let reference_images = reference_images.clone();
        let file_input_ref = file_input_ref.clone();
        Callback::from(move |_| {
            selected_conversation_id.set(None);
            prompt.set(String::new());
            image_count.set("1".to_string());
            reference_files.set(Vec::new());
            reference_images.set(Vec::new());
            clear_file_input(&file_input_ref);
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
                match clear_image_conversations().await {
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
                match delete_image_conversation(&id).await {
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

    let append_reference_images = {
        let reference_files = reference_files.clone();
        let reference_images = reference_images.clone();
        let load_error = load_error.clone();
        let file_input_ref = file_input_ref.clone();
        Callback::from(move |files: Vec<File>| {
            let reference_files = reference_files.clone();
            let reference_images = reference_images.clone();
            let load_error = load_error.clone();
            let file_input_ref = file_input_ref.clone();
            spawn_local(async move {
                if files.is_empty() {
                    return;
                }
                match build_reference_previews(&files).await {
                    Ok(previews) => {
                        let mut next_files = (*reference_files).clone();
                        next_files.extend(files);
                        reference_files.set(next_files);

                        let mut next_previews = (*reference_images).clone();
                        next_previews.extend(previews);
                        reference_images.set(next_previews);
                        clear_file_input(&file_input_ref);
                    },
                    Err(err) => load_error.set(Some(err)),
                }
            });
        })
    };

    let on_reference_input_change = {
        let append_reference_images = append_reference_images.clone();
        let reference_files = reference_files.clone();
        let reference_images = reference_images.clone();
        let file_input_ref = file_input_ref.clone();
        Callback::from(move |event: Event| {
            let input = event.target_unchecked_into::<HtmlInputElement>();
            let mut files = Vec::new();
            if let Some(file_list) = input.files() {
                for index in 0..file_list.length() {
                    if let Some(file) = file_list.get(index) {
                        files.push(file);
                    }
                }
            }
            if files.is_empty() {
                reference_files.set(Vec::new());
                reference_images.set(Vec::new());
                clear_file_input(&file_input_ref);
                return;
            }
            append_reference_images.emit(files);
        })
    };

    let on_remove_reference_image = {
        let reference_files = reference_files.clone();
        let reference_images = reference_images.clone();
        let file_input_ref = file_input_ref.clone();
        Callback::from(move |index: usize| {
            let next_files = (*reference_files)
                .iter()
                .enumerate()
                .filter(|(current, _)| *current != index)
                .map(|(_, file)| file.clone())
                .collect::<Vec<_>>();
            let next_images = (*reference_images)
                .iter()
                .enumerate()
                .filter(|(current, _)| *current != index)
                .map(|(_, image)| image.clone())
                .collect::<Vec<_>>();
            if next_files.is_empty() {
                clear_file_input(&file_input_ref);
            }
            reference_files.set(next_files);
            reference_images.set(next_images);
        })
    };

    let on_prompt_paste = {
        let image_mode = image_mode.clone();
        let append_reference_images = append_reference_images.clone();
        Callback::from(move |event: Event| {
            if *image_mode != "edit" {
                return;
            }
            let event = event.unchecked_into::<ClipboardEvent>();
            let Some(data_transfer): Option<web_sys::DataTransfer> = event.clipboard_data() else {
                return;
            };
            let Some(files): Option<web_sys::FileList> = data_transfer.files() else {
                return;
            };
            let mut images = Vec::new();
            for index in 0..files.length() {
                if let Some(file) = files.get(index) {
                    if file.type_().starts_with("image/") {
                        images.push(file);
                    }
                }
            }
            if images.is_empty() {
                return;
            }
            event.prevent_default();
            append_reference_images.emit(images);
        })
    };

    let on_submit = {
        let auth_key = auth_key.clone();
        let key_info = key_info.clone();
        let prompt = prompt.clone();
        let image_mode = image_mode.clone();
        let image_model = image_model.clone();
        let image_count = image_count.clone();
        let reference_files = reference_files.clone();
        let reference_images = reference_images.clone();
        let conversations = conversations.clone();
        let selected_conversation_id = selected_conversation_id.clone();
        let generating_ids = generating_ids.clone();
        let load_error = load_error.clone();
        let file_input_ref = file_input_ref.clone();
        Callback::from(move |_| {
            let auth_key_value = (*auth_key).trim().to_string();
            if auth_key_value.is_empty() {
                load_error.set(Some("请先登录公开 Key".to_string()));
                return;
            }
            let prompt_value = (*prompt).trim().to_string();
            if prompt_value.is_empty() {
                load_error.set(Some("请输入提示词".to_string()));
                return;
            }
            if *image_mode == "edit" && reference_files.is_empty() {
                load_error.set(Some("请先上传参考图".to_string()));
                return;
            }

            let parsed_count = (*image_count)
                .trim()
                .parse::<usize>()
                .unwrap_or(1)
                .clamp(1, 10);
            let conversation_id = create_client_id();
            let image_mode_value = (*image_mode).clone();
            let image_model_value = (*image_model).clone();
            let image_files = (*reference_files).clone();
            let draft = StoredGpt2ApiImageConversation {
                id: conversation_id.clone(),
                title: build_conversation_title(&prompt_value, 5),
                created_at_ms: now_ms(),
                prompt: prompt_value.clone(),
                model: image_model_value.clone(),
                mode: image_mode_value.clone(),
                count: parsed_count,
                status: "generating".to_string(),
                error: None,
                reference_images: if image_mode_value == "edit" {
                    (*reference_images).clone()
                } else {
                    Vec::new()
                },
                images: (0..parsed_count)
                    .map(|index| StoredGpt2ApiImage {
                        id: format!("{conversation_id}-{index}"),
                        b64_json: None,
                        status: "loading".to_string(),
                        error: None,
                    })
                    .collect(),
            };

            let next = replace_image_conversation(&conversations, draft.clone());
            conversations.set(next);
            selected_conversation_id.set(Some(conversation_id.clone()));
            prompt.set(String::new());
            image_count.set("1".to_string());
            reference_files.set(Vec::new());
            reference_images.set(Vec::new());
            clear_file_input(&file_input_ref);
            load_error.set(None);

            let mut next_generating = (*generating_ids).clone();
            if !next_generating.contains(&conversation_id) {
                next_generating.push(conversation_id.clone());
            }
            generating_ids.set(next_generating);

            let conversations = conversations.clone();
            let generating_ids = generating_ids.clone();
            let load_error = load_error.clone();
            let key_info = key_info.clone();
            spawn_local(async move {
                let _ = save_image_conversation(&draft).await;

                let result = if image_mode_value == "edit" {
                    edit_public_gpt2api_images(
                        &auth_key_value,
                        &prompt_value,
                        &image_model_value,
                        parsed_count,
                        &image_files,
                    )
                    .await
                } else {
                    generate_public_gpt2api_images(
                        &auth_key_value,
                        &AdminGpt2ApiRsImageGenerationRequest {
                            prompt: prompt_value.clone(),
                            model: image_model_value.clone(),
                            n: parsed_count,
                            response_format: "b64_json".to_string(),
                        },
                    )
                    .await
                };

                let updated = match result {
                    Ok(response) => {
                        let mut success_count = 0usize;
                        let mut next_images = Vec::with_capacity(parsed_count);
                        for index in 0..parsed_count {
                            let image_id = format!("{conversation_id}-{index}");
                            let payload = response.data.get(index).cloned();
                            match payload.and_then(|item| item.b64_json) {
                                Some(b64_json) if !b64_json.is_empty() => {
                                    success_count += 1;
                                    next_images.push(StoredGpt2ApiImage {
                                        id: image_id,
                                        b64_json: Some(b64_json),
                                        status: "success".to_string(),
                                        error: None,
                                    });
                                },
                                _ => next_images.push(StoredGpt2ApiImage {
                                    id: image_id,
                                    b64_json: None,
                                    status: "error".to_string(),
                                    error: Some(format!("第 {} 张没有返回图片数据", index + 1)),
                                }),
                            }
                        }
                        let failed_count = parsed_count.saturating_sub(success_count);
                        StoredGpt2ApiImageConversation {
                            status: if failed_count == 0 {
                                "success".to_string()
                            } else {
                                "error".to_string()
                            },
                            error: if failed_count == 0 {
                                None
                            } else {
                                Some(format!("其中 {} 张生成失败", failed_count))
                            },
                            images: next_images,
                            ..draft.clone()
                        }
                    },
                    Err(err) => {
                        load_error.set(Some(err.clone()));
                        StoredGpt2ApiImageConversation {
                            status: "error".to_string(),
                            error: Some(err.clone()),
                            images: draft
                                .images
                                .iter()
                                .map(|image| StoredGpt2ApiImage {
                                    id: image.id.clone(),
                                    b64_json: None,
                                    status: "error".to_string(),
                                    error: Some(err.clone()),
                                })
                                .collect(),
                            ..draft.clone()
                        }
                    },
                };

                let next = replace_image_conversation(&conversations, updated.clone());
                conversations.set(next);
                let _ = save_image_conversation(&updated).await;

                let next_generating = (*generating_ids)
                    .iter()
                    .filter(|item| **item != conversation_id)
                    .cloned()
                    .collect::<Vec<_>>();
                generating_ids.set(next_generating);

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
    let is_selected_generating = selected_conversation
        .as_ref()
        .is_some_and(|item| generating_ids.contains(&item.id));
    let has_any_generating = !generating_ids.is_empty();

    let selected_lightbox_images = selected_conversation
        .as_ref()
        .map(|conversation| {
            conversation
                .images
                .iter()
                .filter_map(|image| {
                    image.b64_json.as_ref().map(|b64_json| LightboxImage {
                        id: image.id.clone(),
                        src: format!("data:image/png;base64,{b64_json}"),
                    })
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    let open_generated_lightbox = {
        let lightbox_images = lightbox_images.clone();
        let lightbox_index = lightbox_index.clone();
        let lightbox_open = lightbox_open.clone();
        Callback::from(move |image_id: String| {
            let images = selected_lightbox_images.clone();
            if let Some(index) = images.iter().position(|image| image.id == image_id) {
                lightbox_images.set(images);
                lightbox_index.set(index);
                lightbox_open.set(true);
            }
        })
    };

    let open_reference_lightbox = {
        let lightbox_images = lightbox_images.clone();
        let lightbox_index = lightbox_index.clone();
        let lightbox_open = lightbox_open.clone();
        let current_reference_images = (*reference_images).clone();
        Callback::from(move |index: usize| {
            let images = current_reference_images
                .iter()
                .enumerate()
                .map(|(current, image)| LightboxImage {
                    id: format!("reference-{current}"),
                    src: image.data_url.clone(),
                })
                .collect::<Vec<_>>();
            if images.is_empty() {
                return;
            }
            lightbox_images.set(images);
            lightbox_index.set(index.min(current_reference_images.len().saturating_sub(1)));
            lightbox_open.set(true);
        })
    };

    let close_lightbox = {
        let lightbox_open = lightbox_open.clone();
        Callback::from(move |_| lightbox_open.set(false))
    };
    let prev_lightbox = {
        let lightbox_index = lightbox_index.clone();
        let lightbox_images = lightbox_images.clone();
        Callback::from(move |_| {
            if lightbox_images.is_empty() {
                return;
            }
            let len = lightbox_images.len();
            let next = if *lightbox_index == 0 { len - 1 } else { *lightbox_index - 1 };
            lightbox_index.set(next);
        })
    };
    let next_lightbox = {
        let lightbox_index = lightbox_index.clone();
        let lightbox_images = lightbox_images.clone();
        Callback::from(move |_| {
            if lightbox_images.is_empty() {
                return;
            }
            let len = lightbox_images.len();
            lightbox_index.set((*lightbox_index + 1) % len);
        })
    };

    html! {
        <>
            { nav_shell("image", on_logout) }
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
                                    { "还没有图片记录，输入提示词后会在这里显示。" }
                                </div>
                            } else {
                                { for conversations.iter().map(|conversation| {
                                    let conversation_id = conversation.id.clone();
                                    let select_id = selected_conversation_id.clone();
                                    let delete_id = conversation.id.clone();
                                    let on_delete_conversation = on_delete_conversation.clone();
                                    let generating = generating_ids.contains(&conversation.id);
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
                                                    if generating {
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
                            <div class={classes!("mx-auto", "flex", "w-full", "max-w-[980px]", "flex-col", "gap-4")}>
                                <div class={classes!("flex", "justify-end")}>
                                    <div class={classes!("w-full", "max-w-[min(820px,92%)]", "px-1", "pt-1")}>
                                        <div class={classes!("ml-auto", "flex", "max-w-full", "flex-col", "items-end", "gap-2.5", "text-right")}>
                                            <div class={classes!("w-fit", "max-w-[min(32rem,100%)]", "whitespace-pre-wrap", "break-words", "text-[15px]", "leading-6", "text-stone-700", "sm:leading-7")}>
                                                { conversation.prompt.clone() }
                                            </div>
                                            if !conversation.reference_images.is_empty() {
                                                <div
                                                    class={classes!("grid", "w-fit", "auto-rows-fr", "gap-3")}
                                                    style={format!("grid-template-columns: repeat({}, minmax(0, 1fr));", conversation.reference_images.len().min(3))}
                                                >
                                                    { for conversation.reference_images.iter().enumerate().map(|(index, image)| {
                                                        let lightbox_images = lightbox_images.clone();
                                                        let lightbox_index = lightbox_index.clone();
                                                        let lightbox_open = lightbox_open.clone();
                                                        let reference_lightbox_images = conversation
                                                            .reference_images
                                                            .iter()
                                                            .enumerate()
                                                            .map(|(current, image)| LightboxImage {
                                                                id: format!("selected-reference-{current}"),
                                                                src: image.data_url.clone(),
                                                            })
                                                            .collect::<Vec<_>>();
                                                        html! {
                                                            <button
                                                                type="button"
                                                                class={classes!("group", "relative", "aspect-square", "min-h-[112px]", "overflow-hidden", "rounded-[18px]", "border", "border-stone-200/80", "bg-stone-100/60", "text-left", "transition", "hover:border-stone-300", "sm:min-h-[136px]")}
                                                                onclick={Callback::from(move |_| {
                                                                    lightbox_images.set(reference_lightbox_images.clone());
                                                                    lightbox_index.set(index);
                                                                    lightbox_open.set(true);
                                                                })}
                                                            >
                                                                <img
                                                                    src={image.data_url.clone()}
                                                                    alt={image.name.clone()}
                                                                    class={classes!("absolute", "inset-0", "h-full", "w-full", "object-cover", "transition", "duration-200", "group-hover:scale-[1.02]")}
                                                                />
                                                            </button>
                                                        }
                                                    }) }
                                                </div>
                                            }
                                        </div>
                                    </div>
                                </div>

                                <div class={classes!("flex", "justify-start")}>
                                    <div class={classes!("w-full", "p-1")}>
                                        <div class={classes!("mb-4", "flex", "flex-wrap", "items-center", "gap-2", "text-xs", "text-stone-500")}>
                                            <span class={classes!("rounded-full", "bg-stone-100", "px-3", "py-1")}>
                                                { if conversation.mode == "edit" { "编辑图" } else { "文生图" } }
                                            </span>
                                            <span class={classes!("rounded-full", "bg-stone-100", "px-3", "py-1")}>{ conversation.model.clone() }</span>
                                            <span class={classes!("rounded-full", "bg-stone-100", "px-3", "py-1")}>{ format!("{} 张", conversation.count) }</span>
                                            <span class={classes!("rounded-full", "bg-stone-100", "px-3", "py-1")}>{ format_conversation_time(conversation.created_at_ms) }</span>
                                            if is_selected_generating {
                                                <span class={classes!("rounded-full", "bg-amber-50", "px-3", "py-1", "text-amber-700")}>{ "处理中" }</span>
                                            }
                                        </div>

                                        if conversation.status == "error" && conversation.images.is_empty() {
                                            <div class={classes!("border-l-2", "border-rose-300", "bg-rose-50/70", "px-4", "py-4", "text-sm", "leading-6", "text-rose-600")}>
                                                { conversation.error.clone().unwrap_or_else(|| "生成失败".to_string()) }
                                            </div>
                                        }

                                        if !conversation.images.is_empty() {
                                            <div class={classes!("columns-1", "gap-4", "space-y-4", "sm:columns-2", "xl:columns-3")}>
                                                { for conversation.images.iter().enumerate().map(|(index, image)| {
                                                    if image.status == "success" {
                                                        let image_id = image.id.clone();
                                                        let open_generated_lightbox = open_generated_lightbox.clone();
                                                        let src = format!("data:image/png;base64,{}", image.b64_json.clone().unwrap_or_default());
                                                        html! {
                                                            <div class={classes!("break-inside-avoid", "overflow-hidden", "rounded-[22px]")}>
                                                                <button
                                                                    type="button"
                                                                    class={classes!("group", "block", "w-full", "cursor-zoom-in")}
                                                                    onclick={Callback::from(move |_| open_generated_lightbox.emit(image_id.clone()))}
                                                                >
                                                                    <img
                                                                        src={src}
                                                                        alt={format!("Generated result {}", index + 1)}
                                                                        class={classes!("block", "h-auto", "w-full", "transition", "duration-200", "group-hover:brightness-90")}
                                                                    />
                                                                </button>
                                                            </div>
                                                        }
                                                    } else if image.status == "error" {
                                                        html! {
                                                            <div class={classes!("break-inside-avoid", "overflow-hidden", "rounded-[22px]", "bg-rose-50", "px-6", "py-8", "text-center", "text-sm", "leading-6", "text-rose-600")}>
                                                                { image.error.clone().unwrap_or_else(|| "生成失败".to_string()) }
                                                            </div>
                                                        }
                                                    } else {
                                                        html! {
                                                            <div class={classes!("break-inside-avoid", "overflow-hidden", "rounded-[22px]", "bg-stone-100/80", "px-6", "py-8", "text-center", "text-sm", "text-stone-500")}>
                                                                { "正在生成图片..." }
                                                            </div>
                                                        }
                                                    }
                                                }) }
                                            </div>
                                        }

                                        if let Some(error_message) = conversation.error.clone() {
                                            if conversation.images.iter().any(|image| image.status == "success") {
                                                <div class={classes!("mt-4", "border-l-2", "border-amber-300", "bg-amber-50/70", "px-4", "py-3", "text-sm", "leading-6", "text-amber-700")}>
                                                    { error_message }
                                                </div>
                                            }
                                        }
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
                                        { "Turn ideas into images" }
                                    </h1>
                                    <p
                                        class={classes!("mt-4", "text-[15px]", "italic", "tracking-[0.01em]", "text-stone-500")}
                                        style={"font-family: \"Palatino Linotype\",\"Book Antiqua\",\"URW Palladio L\",\"Times New Roman\",serif;"}
                                    >
                                        { "Describe a scene, a mood, or a character, and let the next image start here." }
                                    </p>
                                </div>
                            </div>
                        }
                    </div>

                    <div class={classes!("shrink-0", "flex", "justify-center")}>
                        <div class={classes!("w-full", "max-w-[980px]")}>
                            if *image_mode == "edit" {
                                <input
                                    ref={file_input_ref.clone()}
                                    type="file"
                                    accept="image/*"
                                    multiple=true
                                    class={classes!("hidden")}
                                    onchange={on_reference_input_change}
                                />
                            }

                            if *image_mode == "edit" && !reference_images.is_empty() {
                                <div class={classes!("mb-3", "flex", "flex-wrap", "gap-2", "px-1")}>
                                    { for reference_images.iter().enumerate().map(|(index, image)| {
                                        let on_remove_reference_image = on_remove_reference_image.clone();
                                        let open_reference_lightbox = open_reference_lightbox.clone();
                                        html! {
                                            <div class={classes!("relative", "size-16")}>
                                                <button
                                                    type="button"
                                                    class={classes!("group", "size-16", "overflow-hidden", "rounded-2xl", "border", "border-stone-200", "bg-stone-50", "transition", "hover:border-stone-300")}
                                                    onclick={Callback::from(move |_| open_reference_lightbox.emit(index))}
                                                >
                                                    <img
                                                        src={image.data_url.clone()}
                                                        alt={image.name.clone()}
                                                        class={classes!("h-full", "w-full", "object-cover")}
                                                    />
                                                </button>
                                                <button
                                                    type="button"
                                                    class={classes!("absolute", "-right-1", "-top-1", "inline-flex", "size-5", "items-center", "justify-center", "rounded-full", "border", "border-stone-200", "bg-white", "text-stone-500", "transition", "hover:border-stone-300", "hover:text-stone-800")}
                                                    onclick={Callback::from(move |_| on_remove_reference_image.emit(index))}
                                                >
                                                    { "×" }
                                                </button>
                                            </div>
                                        }
                                    }) }
                                </div>
                            }

                            <div class={classes!("overflow-hidden", "rounded-[32px]", "border", "border-stone-200", "bg-white")}>
                                <div class={classes!("relative", "cursor-text")}>
                                    <textarea
                                        value={(*prompt).clone()}
                                        class={classes!("min-h-[148px]", "w-full", "resize-none", "border-0", "bg-transparent", "px-6", "pb-20", "pt-6", "text-[15px]", "leading-7", "text-stone-900", "outline-none", "placeholder:text-stone-400")}
                                        placeholder={if *image_mode == "edit" { "描述你希望如何修改这张参考图，可直接粘贴图片" } else { "输入你想要生成的画面" }}
                                        oninput={{
                                            let prompt = prompt.clone();
                                            Callback::from(move |event: InputEvent| {
                                                prompt.set(event.target_unchecked_into::<HtmlTextAreaElement>().value());
                                            })
                                        }}
                                        onpaste={on_prompt_paste}
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
                                                if *image_mode == "edit" {
                                                    <button
                                                        type="button"
                                                        class={classes!("h-10", "rounded-full", "border", "border-stone-200", "bg-white", "px-4", "text-sm", "font-medium", "text-stone-700", "transition", "hover:bg-stone-50")}
                                                        onclick={{
                                                            let file_input_ref = file_input_ref.clone();
                                                            Callback::from(move |_| {
                                                                if let Some(input) = file_input_ref.cast::<HtmlInputElement>() {
                                                                    input.click();
                                                                }
                                                            })
                                                        }}
                                                    >
                                                        { if reference_images.is_empty() { "上传参考图" } else { "继续添加参考图" } }
                                                    </button>
                                                }
                                                if let Some(info) = (*key_info).clone() {
                                                    <div class={classes!("rounded-full", "bg-stone-100", "px-3", "py-2", "text-xs", "font-medium", "text-stone-600")}>
                                                        { format!("剩余额度 {}", info.quota_total_calls.saturating_sub(info.quota_used_calls)) }
                                                    </div>
                                                }
                                                if has_any_generating {
                                                    <div class={classes!("rounded-full", "bg-amber-50", "px-3", "py-2", "text-xs", "font-medium", "text-amber-700")}>
                                                        { format!("{} 个任务进行中", generating_ids.len()) }
                                                    </div>
                                                }
                                                <select
                                                    class={classes!("h-10", "w-[164px]", "rounded-full", "border", "border-stone-200", "bg-white", "px-4", "text-sm", "font-medium", "text-stone-700", "outline-none")}
                                                    value={(*image_model).clone()}
                                                    onchange={{
                                                        let image_model = image_model.clone();
                                                        Callback::from(move |event: Event| {
                                                            image_model.set(event.target_unchecked_into::<HtmlSelectElement>().value());
                                                        })
                                                    }}
                                                >
                                                    { for IMAGE_MODEL_OPTIONS.iter().map(|model| html! {
                                                        <option value={model.to_string()}>{ model.to_string() }</option>
                                                    }) }
                                                </select>
                                                <div class={classes!("flex", "items-center", "gap-2", "rounded-full", "border", "border-stone-200", "bg-white", "px-3", "py-1")}>
                                                    <span class={classes!("text-sm", "font-medium", "text-stone-700")}>{ "张数" }</span>
                                                    <input
                                                        type="number"
                                                        min="1"
                                                        max="10"
                                                        step="1"
                                                        value={(*image_count).clone()}
                                                        class={classes!("h-8", "w-[64px]", "border-0", "bg-transparent", "px-0", "text-center", "text-sm", "font-medium", "text-stone-700", "outline-none")}
                                                        oninput={{
                                                            let image_count = image_count.clone();
                                                            Callback::from(move |event: InputEvent| {
                                                                image_count.set(event.target_unchecked_into::<HtmlInputElement>().value());
                                                            })
                                                        }}
                                                    />
                                                </div>
                                                <div class={classes!("flex", "items-center", "gap-2")}>
                                                    <button
                                                        type="button"
                                                        class={classes!("rounded-full", "px-4", "py-2", "text-sm", "font-medium", "transition", if *image_mode == "generate" { "bg-stone-950 text-white" } else { "bg-stone-100 text-stone-600 hover:bg-stone-200" })}
                                                        onclick={{
                                                            let image_mode = image_mode.clone();
                                                            Callback::from(move |_| image_mode.set("generate".to_string()))
                                                        }}
                                                    >
                                                        { "文生图" }
                                                    </button>
                                                    <button
                                                        type="button"
                                                        class={classes!("rounded-full", "px-4", "py-2", "text-sm", "font-medium", "transition", if *image_mode == "edit" { "bg-stone-950 text-white" } else { "bg-stone-100 text-stone-600 hover:bg-stone-200" })}
                                                        onclick={{
                                                            let image_mode = image_mode.clone();
                                                            Callback::from(move |_| image_mode.set("edit".to_string()))
                                                        }}
                                                    >
                                                        { "编辑图" }
                                                    </button>
                                                </div>
                                            </div>
                                            <button
                                                type="button"
                                                class={classes!("inline-flex", "size-11", "shrink-0", "items-center", "justify-center", "rounded-full", "bg-stone-950", "text-white", "transition", "hover:bg-stone-800", "disabled:cursor-not-allowed", "disabled:bg-stone-300")}
                                                onclick={{
                                                    let on_submit = on_submit.clone();
                                                    Callback::from(move |_| on_submit.emit(()))
                                                }}
                                                disabled={prompt.trim().is_empty() || (*image_mode == "edit" && reference_images.is_empty()) || !*auth_ready || *loading_key}
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

            { image_lightbox(
                *lightbox_open,
                &lightbox_images,
                *lightbox_index,
                close_lightbox,
                prev_lightbox,
                next_lightbox,
            ) }
        </>
    }
}
