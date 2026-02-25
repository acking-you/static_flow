use std::collections::HashSet;

use js_sys::Date;
use web_sys::{HtmlInputElement, HtmlTextAreaElement};
use yew::prelude::*;
use yew_router::prelude::Link;

use crate::{
    api::{
        admin_approve_and_run_comment_task, admin_approve_and_run_music_wish,
        admin_approve_comment_task, admin_cleanup_api_behavior, admin_cleanup_comments,
        admin_delete_comment_task, admin_delete_music_wish, admin_reject_comment_task,
        admin_reject_music_wish, admin_retry_comment_task, admin_retry_music_wish,
        delete_admin_published_comment, fetch_admin_api_behavior_config,
        fetch_admin_api_behavior_events, fetch_admin_api_behavior_overview,
        fetch_admin_comment_audit_logs, fetch_admin_comment_runtime_config,
        fetch_admin_comment_task, fetch_admin_comment_task_ai_output,
        fetch_admin_comment_tasks_grouped, fetch_admin_music_wishes,
        fetch_admin_published_comments, fetch_admin_view_analytics_config,
        patch_admin_comment_task, patch_admin_published_comment, update_admin_api_behavior_config,
        update_admin_comment_runtime_config, update_admin_view_analytics_config,
        AdminApiBehaviorCleanupRequest, AdminApiBehaviorEvent, AdminApiBehaviorEventsQuery,
        AdminApiBehaviorOverviewResponse, AdminCleanupRequest, AdminCommentAuditLog,
        AdminCommentTask, AdminCommentTaskAiOutputResponse, AdminCommentTaskGroup,
        AdminPatchCommentTaskRequest, AdminPatchPublishedCommentRequest, AdminTaskActionRequest,
        ApiBehaviorBucket, ApiBehaviorConfig, ArticleComment, ArticleViewPoint,
        CommentRuntimeConfig, MusicWishItem, ViewAnalyticsConfig,
    },
    components::{
        loading_spinner::{LoadingSpinner, SpinnerSize},
        pagination::Pagination,
        view_trend_chart::ViewTrendChart,
    },
    router::Route,
};

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum AdminTab {
    Tasks,
    Published,
    Audit,
    Behavior,
    MusicWishes,
}

fn format_ms(ts_ms: i64) -> String {
    let d = Date::new(&wasm_bindgen::JsValue::from_f64(ts_ms as f64));
    format!(
        "{:04}-{:02}-{:02} {:02}:{:02}:{:02}",
        d.get_full_year(),
        d.get_month() + 1, // JS months are 0-indexed
        d.get_date(),
        d.get_hours(),
        d.get_minutes(),
        d.get_seconds(),
    )
}

fn status_badge_class(status: &str) -> Classes {
    let base = classes!(
        "inline-flex",
        "items-center",
        "rounded-full",
        "px-2",
        "py-0.5",
        "text-xs",
        "font-semibold",
        "uppercase",
        "tracking-[0.06em]"
    );
    match status {
        "pending" => classes!(base, "bg-amber-500/15", "text-amber-700", "dark:text-amber-200"),
        "approved" => classes!(base, "bg-sky-500/15", "text-sky-700", "dark:text-sky-200"),
        "running" => classes!(base, "bg-indigo-500/15", "text-indigo-700", "dark:text-indigo-200"),
        "done" => classes!(base, "bg-emerald-500/15", "text-emerald-700", "dark:text-emerald-200"),
        "failed" => classes!(base, "bg-red-500/15", "text-red-700", "dark:text-red-200"),
        "rejected" => classes!(base, "bg-slate-500/15", "text-slate-700", "dark:text-slate-200"),
        _ => classes!(base, "bg-[var(--surface-alt)]", "text-[var(--muted)]"),
    }
}

fn to_view_points(buckets: &[ApiBehaviorBucket]) -> Vec<ArticleViewPoint> {
    buckets
        .iter()
        .map(|item| ArticleViewPoint {
            key: item.key.clone(),
            views: item.count,
        })
        .collect()
}

#[function_component(AdminPage)]
pub fn admin_page() -> Html {
    let load_error = use_state(|| None::<String>);
    let view_config = use_state(|| None::<ViewAnalyticsConfig>);
    let comment_config = use_state(|| None::<CommentRuntimeConfig>);
    let behavior_config = use_state(|| None::<ApiBehaviorConfig>);
    let behavior_overview = use_state(|| None::<AdminApiBehaviorOverviewResponse>);
    let behavior_events = use_state(Vec::<AdminApiBehaviorEvent>::new);
    let behavior_days = use_state(|| "30".to_string());
    let behavior_date = use_state(String::new);
    let behavior_has_more = use_state(|| false);
    let behavior_total = use_state(|| 0_usize);
    let behavior_offset = use_state(|| 0_usize);
    let behavior_path_filter = use_state(String::new);
    let behavior_page_filter = use_state(String::new);
    let behavior_device_filter = use_state(String::new);
    let behavior_status_filter = use_state(String::new);

    let task_groups = use_state(Vec::<AdminCommentTaskGroup>::new);
    let grouped_status_counts = use_state(std::collections::HashMap::<String, usize>::new);
    let status_filter = use_state(String::new);
    let selected_task_id = use_state(|| None::<String>);
    let selected_task = use_state(|| None::<AdminCommentTask>);
    let selected_task_ai_output = use_state(|| None::<AdminCommentTaskAiOutputResponse>);
    let task_action_inflight = use_state(HashSet::<String>::new);

    let published_comments = use_state(Vec::<ArticleComment>::new);
    let selected_published_id = use_state(|| None::<String>);
    let selected_published = use_state(|| None::<ArticleComment>);

    let audit_logs = use_state(Vec::<AdminCommentAuditLog>::new);
    let audit_task_filter = use_state(String::new);
    let audit_action_filter = use_state(String::new);

    let active_tab = use_state(|| None::<AdminTab>);
    let cleanup_days = use_state(|| "30".to_string());
    let loading = use_state(|| false);

    let music_wishes = use_state(Vec::<MusicWishItem>::new);
    let music_wish_action_inflight = use_state(HashSet::<String>::new);
    let saving = use_state(|| false);
    let loaded_tabs = use_state(HashSet::<AdminTab>::new);
    let tab_loading = use_state(HashSet::<AdminTab>::new);
    // Request sequence guards to avoid stale async responses overriding newer pages.
    let refresh_all_seq = use_mut_ref(|| 0_u64);
    let refresh_audit_seq = use_mut_ref(|| 0_u64);
    let refresh_behavior_seq = use_mut_ref(|| 0_u64);
    let refresh_music_wishes_seq = use_mut_ref(|| 0_u64);

    // Per-tab pagination state
    const PAGE_SIZE: usize = 50;
    let tasks_page = use_state(|| 1_usize);
    let published_page = use_state(|| 1_usize);
    let audit_page = use_state(|| 1_usize);
    let behavior_page = use_state(|| 1_usize);
    let music_wish_page = use_state(|| 1_usize);
    // Per-tab total counts for pagination
    let tasks_total = use_state(|| 0_usize);
    let published_total = use_state(|| 0_usize);
    let audit_total = use_state(|| 0_usize);
    let music_wish_total = use_state(|| 0_usize);
    let tasks_has_more = use_state(|| false);
    let published_has_more = use_state(|| false);
    let audit_has_more = use_state(|| false);
    let music_wish_has_more = use_state(|| false);
    // Split behavior loading flags
    let behavior_overview_loading = use_state(|| false);
    let behavior_events_loading = use_state(|| false);

    let refresh_music_wishes = {
        let music_wishes = music_wishes.clone();
        let load_error = load_error.clone();
        let tab_loading = tab_loading.clone();
        let music_wish_page = music_wish_page.clone();
        let music_wish_total = music_wish_total.clone();
        let music_wish_has_more = music_wish_has_more.clone();
        let refresh_music_wishes_seq = refresh_music_wishes_seq.clone();
        Callback::from(move |requested_page: Option<usize>| {
            let music_wishes = music_wishes.clone();
            let load_error = load_error.clone();
            let tab_loading = tab_loading.clone();
            let page = requested_page.unwrap_or(*music_wish_page).max(1);
            let music_wish_total = music_wish_total.clone();
            let music_wish_has_more = music_wish_has_more.clone();
            let refresh_music_wishes_seq = refresh_music_wishes_seq.clone();
            let request_id = {
                let mut seq = refresh_music_wishes_seq.borrow_mut();
                *seq += 1;
                *seq
            };
            {
                let mut s = (*tab_loading).clone();
                s.insert(AdminTab::MusicWishes);
                tab_loading.set(s);
            }
            wasm_bindgen_futures::spawn_local(async move {
                let offset = (page - 1) * PAGE_SIZE;
                match fetch_admin_music_wishes(None, Some(PAGE_SIZE), Some(offset)).await {
                    Ok(resp) => {
                        if *refresh_music_wishes_seq.borrow() != request_id {
                            return;
                        }
                        music_wishes.set(resp.wishes);
                        music_wish_total.set(resp.total);
                        music_wish_has_more.set(resp.has_more);
                        load_error.set(None);
                    },
                    Err(err) => {
                        if *refresh_music_wishes_seq.borrow() != request_id {
                            return;
                        }
                        load_error.set(Some(format!("Failed to load music wishes: {}", err)));
                    },
                }
                if *refresh_music_wishes_seq.borrow() != request_id {
                    return;
                }
                let mut s = (*tab_loading).clone();
                s.remove(&AdminTab::MusicWishes);
                tab_loading.set(s);
            });
        })
    };

    let refresh_audit = {
        let load_error = load_error.clone();
        let audit_logs = audit_logs.clone();
        let audit_task_filter = audit_task_filter.clone();
        let audit_action_filter = audit_action_filter.clone();
        let tab_loading = tab_loading.clone();
        let audit_page = audit_page.clone();
        let audit_total = audit_total.clone();
        let audit_has_more = audit_has_more.clone();
        let refresh_audit_seq = refresh_audit_seq.clone();
        Callback::from(move |requested_page: Option<usize>| {
            let load_error = load_error.clone();
            let audit_logs = audit_logs.clone();
            let task_filter = (*audit_task_filter).trim().to_string();
            let action_filter = (*audit_action_filter).trim().to_string();
            let tab_loading = tab_loading.clone();
            let page = requested_page.unwrap_or(*audit_page).max(1);
            let audit_total = audit_total.clone();
            let audit_has_more = audit_has_more.clone();
            let refresh_audit_seq = refresh_audit_seq.clone();
            let request_id = {
                let mut seq = refresh_audit_seq.borrow_mut();
                *seq += 1;
                *seq
            };
            {
                let mut s = (*tab_loading).clone();
                s.insert(AdminTab::Audit);
                tab_loading.set(s);
            }
            wasm_bindgen_futures::spawn_local(async move {
                let offset = (page - 1) * PAGE_SIZE;
                match fetch_admin_comment_audit_logs(
                    if task_filter.is_empty() { None } else { Some(task_filter.as_str()) },
                    if action_filter.is_empty() { None } else { Some(action_filter.as_str()) },
                    Some(PAGE_SIZE),
                    Some(offset),
                )
                .await
                {
                    Ok(resp) => {
                        if *refresh_audit_seq.borrow() != request_id {
                            return;
                        }
                        audit_logs.set(resp.logs);
                        audit_total.set(resp.total);
                        audit_has_more.set(resp.has_more);
                        load_error.set(None);
                    },
                    Err(err) => {
                        if *refresh_audit_seq.borrow() != request_id {
                            return;
                        }
                        load_error.set(Some(format!("Failed to load audit logs: {}", err)));
                    },
                }
                if *refresh_audit_seq.borrow() != request_id {
                    return;
                }
                let mut s = (*tab_loading).clone();
                s.remove(&AdminTab::Audit);
                tab_loading.set(s);
            });
        })
    };

    let on_refresh_audit_click = {
        let audit_page = audit_page.clone();
        let refresh_audit = refresh_audit.clone();
        Callback::from(move |_| {
            audit_page.set(1);
            refresh_audit.emit(Some(1));
        })
    };

    let refresh_behavior = {
        let load_error = load_error.clone();
        let behavior_config = behavior_config.clone();
        let behavior_overview = behavior_overview.clone();
        let behavior_events = behavior_events.clone();
        let behavior_days = behavior_days.clone();
        let behavior_date = behavior_date.clone();
        let behavior_has_more = behavior_has_more.clone();
        let behavior_total = behavior_total.clone();
        let behavior_offset = behavior_offset.clone();
        let behavior_path_filter = behavior_path_filter.clone();
        let behavior_page_filter = behavior_page_filter.clone();
        let behavior_device_filter = behavior_device_filter.clone();
        let behavior_status_filter = behavior_status_filter.clone();
        let tab_loading = tab_loading.clone();
        let behavior_page = behavior_page.clone();
        let behavior_overview_loading = behavior_overview_loading.clone();
        let behavior_events_loading = behavior_events_loading.clone();
        let refresh_behavior_seq = refresh_behavior_seq.clone();

        Callback::from(move |requested_page: Option<usize>| {
            let load_error = load_error.clone();
            let behavior_config = behavior_config.clone();
            let behavior_overview = behavior_overview.clone();
            let behavior_events = behavior_events.clone();
            let behavior_has_more = behavior_has_more.clone();
            let behavior_total = behavior_total.clone();
            let behavior_offset = behavior_offset.clone();
            let date_val = (*behavior_date).trim().to_string();
            let days = (*behavior_days)
                .trim()
                .parse::<usize>()
                .ok()
                .filter(|value| *value > 0);
            let path_filter = (*behavior_path_filter).trim().to_string();
            let page_filter = (*behavior_page_filter).trim().to_string();
            let device_filter = (*behavior_device_filter).trim().to_string();
            let status_filter = (*behavior_status_filter).trim().parse::<i32>().ok();
            let tab_loading = tab_loading.clone();
            let page = requested_page.unwrap_or(*behavior_page).max(1);
            let behavior_overview_loading = behavior_overview_loading.clone();
            let behavior_events_loading = behavior_events_loading.clone();
            let refresh_behavior_seq = refresh_behavior_seq.clone();
            let request_id = {
                let mut seq = refresh_behavior_seq.borrow_mut();
                *seq += 1;
                *seq
            };

            let (query_days, query_date) =
                if date_val.is_empty() { (days, None) } else { (None, Some(date_val)) };

            {
                let mut s = (*tab_loading).clone();
                s.insert(AdminTab::Behavior);
                tab_loading.set(s);
            }

            // Spawn overview + config fetch
            {
                let behavior_config = behavior_config.clone();
                let behavior_overview = behavior_overview.clone();
                let load_error = load_error.clone();
                let behavior_overview_loading = behavior_overview_loading.clone();
                let query_days = query_days;
                let refresh_behavior_seq = refresh_behavior_seq.clone();
                behavior_overview_loading.set(true);
                wasm_bindgen_futures::spawn_local(async move {
                    let config_result = fetch_admin_api_behavior_config().await;
                    let overview_result =
                        fetch_admin_api_behavior_overview(query_days, Some(20)).await;
                    match (config_result, overview_result) {
                        (Ok(config), Ok(overview)) => {
                            if *refresh_behavior_seq.borrow() != request_id {
                                return;
                            }
                            behavior_config.set(Some(config));
                            behavior_overview.set(Some(overview));
                        },
                        (cfg_err, over_err) => {
                            if *refresh_behavior_seq.borrow() != request_id {
                                return;
                            }
                            load_error.set(Some(format!(
                                "Behavior overview unavailable. config={:?}, overview={:?}",
                                cfg_err.err(),
                                over_err.err()
                            )));
                        },
                    }
                    if *refresh_behavior_seq.borrow() != request_id {
                        return;
                    }
                    behavior_overview_loading.set(false);
                    // Remove tab loading if events also done
                    // (events spawn handles its own removal)
                });
            }

            // Spawn events fetch
            {
                let behavior_events = behavior_events.clone();
                let behavior_has_more = behavior_has_more.clone();
                let behavior_total = behavior_total.clone();
                let behavior_offset = behavior_offset.clone();
                let load_error = load_error.clone();
                let tab_loading = tab_loading.clone();
                let behavior_events_loading = behavior_events_loading.clone();
                let query_date = query_date.clone();
                let refresh_behavior_seq = refresh_behavior_seq.clone();
                behavior_events_loading.set(true);
                wasm_bindgen_futures::spawn_local(async move {
                    let offset = (page - 1) * PAGE_SIZE;
                    let events_result =
                        fetch_admin_api_behavior_events(&AdminApiBehaviorEventsQuery {
                            days: query_days,
                            limit: Some(PAGE_SIZE),
                            offset: Some(offset),
                            path_contains: if path_filter.is_empty() {
                                None
                            } else {
                                Some(path_filter)
                            },
                            page_contains: if page_filter.is_empty() {
                                None
                            } else {
                                Some(page_filter)
                            },
                            device_type: if device_filter.is_empty() {
                                None
                            } else {
                                Some(device_filter)
                            },
                            method: None,
                            status_code: status_filter,
                            ip: None,
                            date: query_date,
                        })
                        .await;

                    match events_result {
                        Ok(events) => {
                            if *refresh_behavior_seq.borrow() != request_id {
                                return;
                            }
                            behavior_has_more.set(events.has_more);
                            behavior_total.set(events.total);
                            behavior_offset.set(events.offset);
                            behavior_events.set(events.events);
                        },
                        Err(err) => {
                            if *refresh_behavior_seq.borrow() != request_id {
                                return;
                            }
                            load_error.set(Some(format!("Behavior events unavailable: {:?}", err)));
                        },
                    }
                    if *refresh_behavior_seq.borrow() != request_id {
                        return;
                    }
                    behavior_events_loading.set(false);
                    let mut s = (*tab_loading).clone();
                    s.remove(&AdminTab::Behavior);
                    tab_loading.set(s);
                });
            }
        })
    };

    let refresh_all = {
        let load_error = load_error.clone();
        let view_config = view_config.clone();
        let comment_config = comment_config.clone();
        let task_groups = task_groups.clone();
        let grouped_status_counts = grouped_status_counts.clone();
        let published_comments = published_comments.clone();
        let selected_task_id = selected_task_id.clone();
        let selected_task = selected_task.clone();
        let selected_task_ai_output = selected_task_ai_output.clone();
        let selected_published_id = selected_published_id.clone();
        let selected_published = selected_published.clone();
        let loading = loading.clone();
        let status_filter = status_filter.clone();
        let tab_loading = tab_loading.clone();
        let tasks_page = tasks_page.clone();
        let published_page = published_page.clone();
        let tasks_total = tasks_total.clone();
        let published_total = published_total.clone();
        let tasks_has_more = tasks_has_more.clone();
        let published_has_more = published_has_more.clone();
        let refresh_all_seq = refresh_all_seq.clone();

        Callback::from(move |requested_pages: (Option<usize>, Option<usize>)| {
            let load_error = load_error.clone();
            let view_config = view_config.clone();
            let comment_config = comment_config.clone();
            let task_groups = task_groups.clone();
            let grouped_status_counts = grouped_status_counts.clone();
            let published_comments = published_comments.clone();
            let selected_task_id = selected_task_id.clone();
            let selected_task = selected_task.clone();
            let selected_task_ai_output = selected_task_ai_output.clone();
            let selected_published_id = selected_published_id.clone();
            let selected_published = selected_published.clone();
            let loading = loading.clone();
            let tab_loading = tab_loading.clone();
            let tasks_total = tasks_total.clone();
            let published_total = published_total.clone();
            let tasks_has_more = tasks_has_more.clone();
            let published_has_more = published_has_more.clone();
            let refresh_all_seq = refresh_all_seq.clone();
            let request_id = {
                let mut seq = refresh_all_seq.borrow_mut();
                *seq += 1;
                *seq
            };

            let status = (*status_filter).trim().to_string();
            let t_page = requested_pages.0.unwrap_or(*tasks_page).max(1);
            let p_page = requested_pages.1.unwrap_or(*published_page).max(1);
            loading.set(true);
            {
                let mut s = (*tab_loading).clone();
                s.insert(AdminTab::Tasks);
                s.insert(AdminTab::Published);
                tab_loading.set(s);
            }
            wasm_bindgen_futures::spawn_local(async move {
                let t_offset = (t_page - 1) * PAGE_SIZE;
                let p_offset = (p_page - 1) * PAGE_SIZE;
                let view_result = fetch_admin_view_analytics_config().await;
                let comment_result = fetch_admin_comment_runtime_config().await;
                let grouped_result = fetch_admin_comment_tasks_grouped(
                    if status.is_empty() { None } else { Some(status.as_str()) },
                    Some(PAGE_SIZE),
                    Some(t_offset),
                )
                .await;
                let published_result =
                    fetch_admin_published_comments(None, None, Some(PAGE_SIZE), Some(p_offset))
                        .await;

                if *refresh_all_seq.borrow() != request_id {
                    return;
                }

                match (view_result, comment_result, grouped_result, published_result) {
                    (Ok(view), Ok(comment), Ok(grouped), Ok(published)) => {
                        if *refresh_all_seq.borrow() != request_id {
                            return;
                        }
                        view_config.set(Some(view));
                        comment_config.set(Some(comment));
                        grouped_status_counts.set(grouped.status_counts);
                        tasks_total.set(grouped.total_articles);
                        tasks_has_more.set(grouped.has_more);
                        task_groups.set(grouped.groups.clone());
                        published_total.set(published.total);
                        published_has_more.set(published.has_more);
                        published_comments.set(published.comments.clone());

                        if let Some(task_id) = (*selected_task_id).clone() {
                            let mut found = None;
                            for group in grouped.groups {
                                if let Some(task) =
                                    group.tasks.into_iter().find(|task| task.task_id == task_id)
                                {
                                    found = Some(task);
                                    break;
                                }
                            }
                            selected_task.set(found);
                            match fetch_admin_comment_task_ai_output(&task_id, None, Some(1200))
                                .await
                            {
                                Ok(output) => {
                                    if *refresh_all_seq.borrow() != request_id {
                                        return;
                                    }
                                    selected_task_ai_output.set(Some(output));
                                },
                                Err(err) => {
                                    if *refresh_all_seq.borrow() != request_id {
                                        return;
                                    }
                                    selected_task_ai_output.set(None);
                                    load_error.set(Some(format!(
                                        "Failed to load task AI output: {}",
                                        err
                                    )));
                                },
                            }
                        } else {
                            if *refresh_all_seq.borrow() != request_id {
                                return;
                            }
                            selected_task_ai_output.set(None);
                        }

                        if let Some(comment_id) = (*selected_published_id).clone() {
                            if *refresh_all_seq.borrow() != request_id {
                                return;
                            }
                            let found = published
                                .comments
                                .into_iter()
                                .find(|comment| comment.comment_id == comment_id);
                            selected_published.set(found);
                        }

                        if *refresh_all_seq.borrow() != request_id {
                            return;
                        }
                        load_error.set(None);
                    },
                    (view_err, comment_err, grouped_err, published_err) => {
                        if *refresh_all_seq.borrow() != request_id {
                            return;
                        }
                        load_error.set(Some(format!(
                            "Admin API unavailable. view={:?}, comment={:?}, grouped={:?}, \
                             published={:?}",
                            view_err.err(),
                            comment_err.err(),
                            grouped_err.err(),
                            published_err.err()
                        )));
                    },
                }
                if *refresh_all_seq.borrow() != request_id {
                    return;
                }
                loading.set(false);
                let mut s = (*tab_loading).clone();
                s.remove(&AdminTab::Tasks);
                s.remove(&AdminTab::Published);
                tab_loading.set(s);
            });
        })
    };

    {
        let active_tab = active_tab.clone();
        let loaded_tabs = loaded_tabs.clone();
        let refresh_all = refresh_all.clone();
        let refresh_audit = refresh_audit.clone();
        let refresh_behavior = refresh_behavior.clone();
        let refresh_music_wishes = refresh_music_wishes.clone();
        use_effect_with(*active_tab, move |tab| {
            if let Some(tab) = tab {
                if !loaded_tabs.contains(tab) {
                    match *tab {
                        AdminTab::Tasks | AdminTab::Published => refresh_all.emit((None, None)),
                        AdminTab::Audit => refresh_audit.emit(None),
                        AdminTab::Behavior => refresh_behavior.emit(None),
                        AdminTab::MusicWishes => refresh_music_wishes.emit(None),
                    }
                    let mut set = (*loaded_tabs).clone();
                    set.insert(*tab);
                    loaded_tabs.set(set);
                }
            }
            || ()
        });
    }

    let on_filter_change = {
        let status_filter = status_filter.clone();
        Callback::from(move |event: InputEvent| {
            if let Some(target) = event.target_dyn_into::<HtmlInputElement>() {
                status_filter.set(target.value());
            }
        })
    };

    let on_tasks_apply = {
        let tasks_page = tasks_page.clone();
        let refresh_all = refresh_all.clone();
        Callback::from(move |_| {
            tasks_page.set(1);
            refresh_all.emit((Some(1), None));
        })
    };

    let on_reload_click = {
        let active_tab = active_tab.clone();
        let refresh_all = refresh_all.clone();
        let refresh_audit = refresh_audit.clone();
        let refresh_behavior = refresh_behavior.clone();
        let refresh_music_wishes = refresh_music_wishes.clone();
        Callback::from(move |_| {
            if let Some(tab) = *active_tab {
                match tab {
                    AdminTab::Tasks | AdminTab::Published => refresh_all.emit((None, None)),
                    AdminTab::Audit => refresh_audit.emit(None),
                    AdminTab::Behavior => refresh_behavior.emit(None),
                    AdminTab::MusicWishes => refresh_music_wishes.emit(None),
                }
            }
        })
    };

    let on_save_configs = {
        let view_config = view_config.clone();
        let comment_config = comment_config.clone();
        let behavior_config = behavior_config.clone();
        let load_error = load_error.clone();
        let saving = saving.clone();
        let refresh_all = refresh_all.clone();
        let refresh_behavior = refresh_behavior.clone();
        Callback::from(move |_| {
            let Some(view_config_value) = (*view_config).clone() else {
                return;
            };
            let Some(comment_config_value) = (*comment_config).clone() else {
                return;
            };
            let Some(behavior_config_value) = (*behavior_config).clone() else {
                return;
            };

            let load_error = load_error.clone();
            let saving = saving.clone();
            let refresh_all = refresh_all.clone();
            let refresh_behavior = refresh_behavior.clone();
            saving.set(true);
            wasm_bindgen_futures::spawn_local(async move {
                let view_result = update_admin_view_analytics_config(&view_config_value).await;
                let comment_result =
                    update_admin_comment_runtime_config(&comment_config_value).await;
                let behavior_result =
                    update_admin_api_behavior_config(&behavior_config_value).await;
                match (view_result, comment_result, behavior_result) {
                    (Ok(_), Ok(_), Ok(_)) => {
                        load_error.set(None);
                        refresh_all.emit((None, None));
                        refresh_behavior.emit(None);
                    },
                    (view_err, comment_err, behavior_err) => {
                        load_error.set(Some(format!(
                            "Save failed. view={:?}, comment={:?}, behavior={:?}",
                            view_err.err(),
                            comment_err.err(),
                            behavior_err.err()
                        )));
                    },
                }
                saving.set(false);
            });
        })
    };

    let on_select_task = {
        let selected_task_id = selected_task_id.clone();
        let selected_task = selected_task.clone();
        let selected_task_ai_output = selected_task_ai_output.clone();
        let load_error = load_error.clone();
        Callback::from(move |task_id: String| {
            selected_task_id.set(Some(task_id.clone()));
            let selected_task = selected_task.clone();
            let selected_task_ai_output = selected_task_ai_output.clone();
            let load_error = load_error.clone();
            wasm_bindgen_futures::spawn_local(async move {
                let task_result = fetch_admin_comment_task(&task_id).await;
                let ai_result =
                    fetch_admin_comment_task_ai_output(&task_id, None, Some(1200)).await;
                match (task_result, ai_result) {
                    (Ok(task), Ok(ai_output)) => {
                        selected_task.set(Some(task));
                        selected_task_ai_output.set(Some(ai_output));
                    },
                    (Err(err), _) => {
                        selected_task.set(None);
                        selected_task_ai_output.set(None);
                        load_error.set(Some(format!("Failed to load task detail: {}", err)));
                    },
                    (Ok(task), Err(err)) => {
                        selected_task.set(Some(task));
                        selected_task_ai_output.set(None);
                        load_error.set(Some(format!("Failed to load task AI output: {}", err)));
                    },
                }
            });
        })
    };

    let on_select_task_ai_run = {
        let selected_task_id = selected_task_id.clone();
        let selected_task_ai_output = selected_task_ai_output.clone();
        let load_error = load_error.clone();
        Callback::from(move |run_id: String| {
            let Some(task_id) = (*selected_task_id).clone() else {
                return;
            };
            let selected_task_ai_output = selected_task_ai_output.clone();
            let load_error = load_error.clone();
            wasm_bindgen_futures::spawn_local(async move {
                match fetch_admin_comment_task_ai_output(&task_id, Some(&run_id), Some(1200)).await
                {
                    Ok(output) => selected_task_ai_output.set(Some(output)),
                    Err(err) => {
                        load_error.set(Some(format!("Failed to load task AI output: {}", err)));
                    },
                }
            });
        })
    };

    let on_selected_task_comment_change = {
        let selected_task = selected_task.clone();
        Callback::from(move |event: InputEvent| {
            if let Some(target) = event.target_dyn_into::<HtmlTextAreaElement>() {
                let mut next = (*selected_task).clone();
                if let Some(task) = next.as_mut() {
                    task.comment_text = target.value();
                }
                selected_task.set(next);
            }
        })
    };

    let on_selected_task_note_change = {
        let selected_task = selected_task.clone();
        Callback::from(move |event: InputEvent| {
            if let Some(target) = event.target_dyn_into::<HtmlTextAreaElement>() {
                let mut next = (*selected_task).clone();
                if let Some(task) = next.as_mut() {
                    task.admin_note = Some(target.value());
                }
                selected_task.set(next);
            }
        })
    };

    let on_save_task = {
        let selected_task = selected_task.clone();
        let load_error = load_error.clone();
        let refresh_all = refresh_all.clone();
        Callback::from(move |_| {
            let Some(task) = (*selected_task).clone() else {
                return;
            };
            let request = AdminPatchCommentTaskRequest {
                comment_text: Some(task.comment_text.clone()),
                selected_text: task.selected_text.clone(),
                anchor_block_id: task.anchor_block_id.clone(),
                anchor_context_before: task.anchor_context_before.clone(),
                anchor_context_after: task.anchor_context_after.clone(),
                admin_note: task.admin_note.clone(),
                operator: Some("admin-ui".to_string()),
            };
            let load_error = load_error.clone();
            let refresh_all = refresh_all.clone();
            let selected_task = selected_task.clone();
            wasm_bindgen_futures::spawn_local(async move {
                match patch_admin_comment_task(&task.task_id, &request).await {
                    Ok(updated) => {
                        selected_task.set(Some(updated));
                        refresh_all.emit((None, None));
                    },
                    Err(err) => load_error.set(Some(format!("Patch task failed: {}", err))),
                }
            });
        })
    };

    let run_task_action = {
        let load_error = load_error.clone();
        let refresh_all = refresh_all.clone();
        let selected_task = selected_task.clone();
        let selected_task_ai_output = selected_task_ai_output.clone();
        let task_action_inflight = task_action_inflight.clone();
        Callback::from(move |(task_id, action): (String, String)| {
            if task_action_inflight.contains(&task_id) {
                return;
            }
            {
                let mut next = (*task_action_inflight).clone();
                next.insert(task_id.clone());
                task_action_inflight.set(next);
            }
            let load_error = load_error.clone();
            let refresh_all = refresh_all.clone();
            let selected_task = selected_task.clone();
            let selected_task_ai_output = selected_task_ai_output.clone();
            let task_action_inflight = task_action_inflight.clone();
            let request = AdminTaskActionRequest {
                operator: Some("admin-ui".to_string()),
                admin_note: None,
            };
            wasm_bindgen_futures::spawn_local(async move {
                let result = match action.as_str() {
                    "approve" => admin_approve_comment_task(&task_id, &request)
                        .await
                        .map(|_| ()),
                    "approve_run" => admin_approve_and_run_comment_task(&task_id, &request)
                        .await
                        .map(|_| ()),
                    "retry" => admin_retry_comment_task(&task_id, &request)
                        .await
                        .map(|_| ()),
                    "reject" => admin_reject_comment_task(&task_id, &request)
                        .await
                        .map(|_| ()),
                    "delete" => admin_delete_comment_task(&task_id, &request)
                        .await
                        .map(|_| ()),
                    _ => Ok(()),
                };
                match result {
                    Ok(()) => {
                        if selected_task
                            .as_ref()
                            .as_ref()
                            .map(|item| item.task_id.as_str())
                            == Some(task_id.as_str())
                        {
                            selected_task.set(None);
                            selected_task_ai_output.set(None);
                        }
                        refresh_all.emit((None, None));
                    },
                    Err(err) => load_error.set(Some(format!("Task action failed: {}", err))),
                }
                let mut next = (*task_action_inflight).clone();
                next.remove(&task_id);
                task_action_inflight.set(next);
            });
        })
    };

    let on_select_published = {
        let selected_published_id = selected_published_id.clone();
        let selected_published = selected_published.clone();
        Callback::from(move |comment: ArticleComment| {
            selected_published_id.set(Some(comment.comment_id.clone()));
            selected_published.set(Some(comment));
        })
    };

    let on_selected_published_comment_change = {
        let selected_published = selected_published.clone();
        Callback::from(move |event: InputEvent| {
            if let Some(target) = event.target_dyn_into::<HtmlTextAreaElement>() {
                let mut next = (*selected_published).clone();
                if let Some(comment) = next.as_mut() {
                    comment.comment_text = target.value();
                }
                selected_published.set(next);
            }
        })
    };

    let on_selected_published_ai_change = {
        let selected_published = selected_published.clone();
        Callback::from(move |event: InputEvent| {
            if let Some(target) = event.target_dyn_into::<HtmlTextAreaElement>() {
                let mut next = (*selected_published).clone();
                if let Some(comment) = next.as_mut() {
                    comment.ai_reply_markdown = Some(target.value());
                }
                selected_published.set(next);
            }
        })
    };

    let on_save_published = {
        let selected_published = selected_published.clone();
        let load_error = load_error.clone();
        let refresh_all = refresh_all.clone();
        Callback::from(move |_| {
            let Some(comment) = (*selected_published).clone() else {
                return;
            };
            let request = AdminPatchPublishedCommentRequest {
                ai_reply_markdown: comment.ai_reply_markdown.clone(),
                comment_text: Some(comment.comment_text.clone()),
                operator: Some("admin-ui".to_string()),
            };
            let load_error = load_error.clone();
            let refresh_all = refresh_all.clone();
            let selected_published = selected_published.clone();
            wasm_bindgen_futures::spawn_local(async move {
                match patch_admin_published_comment(&comment.comment_id, &request).await {
                    Ok(updated) => {
                        selected_published.set(Some(updated));
                        refresh_all.emit((None, None));
                    },
                    Err(err) => load_error.set(Some(format!("Patch published failed: {}", err))),
                }
            });
        })
    };

    let delete_published_action = {
        let load_error = load_error.clone();
        let refresh_all = refresh_all.clone();
        let selected_published = selected_published.clone();
        Callback::from(move |comment_id: String| {
            let load_error = load_error.clone();
            let refresh_all = refresh_all.clone();
            let selected_published = selected_published.clone();
            let request = AdminTaskActionRequest {
                operator: Some("admin-ui".to_string()),
                admin_note: None,
            };
            wasm_bindgen_futures::spawn_local(async move {
                match delete_admin_published_comment(&comment_id, &request).await {
                    Ok(_) => {
                        if selected_published
                            .as_ref()
                            .as_ref()
                            .map(|item| item.comment_id.as_str())
                            == Some(comment_id.as_str())
                        {
                            selected_published.set(None);
                        }
                        refresh_all.emit((None, None));
                    },
                    Err(err) => {
                        load_error.set(Some(format!("Delete published failed: {}", err)));
                    },
                }
            });
        })
    };

    let on_cleanup = {
        let cleanup_days = cleanup_days.clone();
        let load_error = load_error.clone();
        let refresh_all = refresh_all.clone();
        Callback::from(move |_| {
            let days = cleanup_days.parse::<i64>().ok();
            let request = AdminCleanupRequest {
                status: Some("failed".to_string()),
                retention_days: days,
            };
            let load_error = load_error.clone();
            let refresh_all = refresh_all.clone();
            wasm_bindgen_futures::spawn_local(async move {
                match admin_cleanup_comments(&request).await {
                    Ok(_) => refresh_all.emit((None, None)),
                    Err(err) => load_error.set(Some(format!("Cleanup failed: {}", err))),
                }
            });
        })
    };

    let on_cleanup_days_change = {
        let cleanup_days = cleanup_days.clone();
        Callback::from(move |event: InputEvent| {
            if let Some(target) = event.target_dyn_into::<HtmlInputElement>() {
                cleanup_days.set(target.value());
            }
        })
    };

    let on_audit_task_filter_change = {
        let audit_task_filter = audit_task_filter.clone();
        Callback::from(move |event: InputEvent| {
            if let Some(target) = event.target_dyn_into::<HtmlInputElement>() {
                audit_task_filter.set(target.value());
            }
        })
    };

    let on_audit_action_filter_change = {
        let audit_action_filter = audit_action_filter.clone();
        Callback::from(move |event: InputEvent| {
            if let Some(target) = event.target_dyn_into::<HtmlInputElement>() {
                audit_action_filter.set(target.value());
            }
        })
    };

    let on_behavior_days_change = {
        let behavior_days = behavior_days.clone();
        Callback::from(move |event: InputEvent| {
            if let Some(target) = event.target_dyn_into::<HtmlInputElement>() {
                behavior_days.set(target.value());
            }
        })
    };

    let on_behavior_date_change = {
        let behavior_date = behavior_date.clone();
        Callback::from(move |event: InputEvent| {
            if let Some(target) = event.target_dyn_into::<HtmlInputElement>() {
                behavior_date.set(target.value());
            }
        })
    };

    let on_behavior_path_filter_change = {
        let behavior_path_filter = behavior_path_filter.clone();
        Callback::from(move |event: InputEvent| {
            if let Some(target) = event.target_dyn_into::<HtmlInputElement>() {
                behavior_path_filter.set(target.value());
            }
        })
    };

    let on_behavior_page_filter_change = {
        let behavior_page_filter = behavior_page_filter.clone();
        Callback::from(move |event: InputEvent| {
            if let Some(target) = event.target_dyn_into::<HtmlInputElement>() {
                behavior_page_filter.set(target.value());
            }
        })
    };

    let on_behavior_device_filter_change = {
        let behavior_device_filter = behavior_device_filter.clone();
        Callback::from(move |event: InputEvent| {
            if let Some(target) = event.target_dyn_into::<HtmlInputElement>() {
                behavior_device_filter.set(target.value());
            }
        })
    };

    let on_behavior_status_filter_change = {
        let behavior_status_filter = behavior_status_filter.clone();
        Callback::from(move |event: InputEvent| {
            if let Some(target) = event.target_dyn_into::<HtmlInputElement>() {
                behavior_status_filter.set(target.value());
            }
        })
    };

    let on_behavior_apply = {
        let behavior_page = behavior_page.clone();
        let behavior_events_loading = behavior_events_loading.clone();
        let refresh_behavior = refresh_behavior.clone();
        Callback::from(move |_| {
            behavior_page.set(1);
            behavior_events_loading.set(true);
            refresh_behavior.emit(Some(1));
        })
    };

    let on_behavior_cleanup = {
        let behavior_config = behavior_config.clone();
        let behavior_page = behavior_page.clone();
        let behavior_events_loading = behavior_events_loading.clone();
        let refresh_behavior = refresh_behavior.clone();
        let load_error = load_error.clone();
        Callback::from(move |_| {
            let Some(config) = (*behavior_config).clone() else {
                return;
            };
            let request = AdminApiBehaviorCleanupRequest {
                retention_days: Some(config.retention_days),
            };
            let refresh_behavior = refresh_behavior.clone();
            let load_error = load_error.clone();
            let behavior_page = behavior_page.clone();
            let behavior_events_loading = behavior_events_loading.clone();
            wasm_bindgen_futures::spawn_local(async move {
                match admin_cleanup_api_behavior(&request).await {
                    Ok(_) => {
                        behavior_page.set(1);
                        behavior_events_loading.set(true);
                        refresh_behavior.emit(Some(1));
                    },
                    Err(err) => {
                        load_error.set(Some(format!("Behavior cleanup failed: {}", err)));
                    },
                }
            });
        })
    };

    let tab_tasks = {
        let active_tab = active_tab.clone();
        Callback::from(move |_| active_tab.set(Some(AdminTab::Tasks)))
    };
    let tab_published = {
        let active_tab = active_tab.clone();
        Callback::from(move |_| active_tab.set(Some(AdminTab::Published)))
    };
    let tab_audit = {
        let active_tab = active_tab.clone();
        Callback::from(move |_| active_tab.set(Some(AdminTab::Audit)))
    };
    let tab_behavior = {
        let active_tab = active_tab.clone();
        Callback::from(move |_| active_tab.set(Some(AdminTab::Behavior)))
    };
    let tab_music_wishes = {
        let active_tab = active_tab.clone();
        Callback::from(move |_| active_tab.set(Some(AdminTab::MusicWishes)))
    };

    let grouped_total_tasks: usize = task_groups.iter().map(|group| group.total).sum();

    // Pagination callbacks
    let on_tasks_page_change = {
        let page = tasks_page.clone();
        let refresh = refresh_all.clone();
        Callback::from(move |p: usize| {
            page.set(p);
            refresh.emit((Some(p), None));
        })
    };
    let on_published_page_change = {
        let page = published_page.clone();
        let refresh = refresh_all.clone();
        Callback::from(move |p: usize| {
            page.set(p);
            refresh.emit((None, Some(p)));
        })
    };
    let on_audit_page_change = {
        let page = audit_page.clone();
        let refresh = refresh_audit.clone();
        Callback::from(move |p: usize| {
            page.set(p);
            refresh.emit(Some(p));
        })
    };
    let on_behavior_page_change = {
        let page = behavior_page.clone();
        let refresh = refresh_behavior.clone();
        let behavior_events_loading = behavior_events_loading.clone();
        Callback::from(move |p: usize| {
            page.set(p);
            behavior_events_loading.set(true);
            refresh.emit(Some(p));
        })
    };
    let on_music_wish_page_change = {
        let page = music_wish_page.clone();
        let refresh = refresh_music_wishes.clone();
        Callback::from(move |p: usize| {
            page.set(p);
            refresh.emit(Some(p));
        })
    };

    // Compute total pages
    let tasks_total_pages = ((*tasks_total).max(1) + PAGE_SIZE - 1) / PAGE_SIZE;
    let published_total_pages = ((*published_total).max(1) + PAGE_SIZE - 1) / PAGE_SIZE;
    let audit_total_pages = ((*audit_total).max(1) + PAGE_SIZE - 1) / PAGE_SIZE;
    let behavior_total_pages = ((*behavior_total).max(1) + PAGE_SIZE - 1) / PAGE_SIZE;
    let music_wish_total_pages = ((*music_wish_total).max(1) + PAGE_SIZE - 1) / PAGE_SIZE;

    html! {
        <main class={classes!("container", "py-8")}>
            <section class={classes!(
                "bg-[var(--surface)]",
                "border",
                "border-[var(--border)]",
                "rounded-[var(--radius)]",
                "shadow-[var(--shadow)]",
                "p-5",
                "mb-5"
            )}>
                <div class={classes!("flex", "items-center", "justify-between", "gap-3", "flex-wrap")}>
                    <div>
                        <h1 class={classes!("m-0", "text-xl", "font-semibold")}>{ "Admin Console" }</h1>
                        <p class={classes!("m-0", "text-sm", "text-[var(--muted)]")}>
                            { "Manage runtime config, comments workflows, and API behavior analytics." }
                        </p>
                    </div>
                    <button class={classes!("btn-fluent-secondary")} onclick={on_reload_click.clone()}>
                        <i class={classes!("fas", "fa-rotate-right", "mr-2")} aria-hidden="true"></i>
                        { if *loading { "Loading..." } else { "Refresh" } }
                    </button>
                </div>
                if let Some(err) = (*load_error).clone() {
                    <div class={classes!(
                        "mt-3",
                        "rounded-[var(--radius)]",
                        "border",
                        "border-red-400/40",
                        "bg-red-500/10",
                        "px-3",
                        "py-2",
                        "text-sm",
                        "text-red-700",
                        "dark:text-red-200"
                    )}>
                        { err }
                    </div>
                }
            </section>

            <section class={classes!(
                "bg-[var(--surface)]",
                "border",
                "border-[var(--border)]",
                "rounded-[var(--radius)]",
                "shadow-[var(--shadow)]",
                "p-5",
                "mb-5"
            )}>
                <h2 class={classes!("m-0", "mb-4", "text-lg", "font-semibold")}>{ "Runtime Config" }</h2>
                <div class={classes!("grid", "gap-4", "md:grid-cols-2", "xl:grid-cols-3")}>
                    <div class={classes!("rounded-[var(--radius)]", "border", "border-[var(--border)]", "p-3")}>
                        <h3 class={classes!("m-0", "mb-2", "text-sm", "uppercase", "tracking-[0.08em]", "text-[var(--muted)]")}>
                            { "View Analytics" }
                        </h3>
                        if let Some(cfg) = (*view_config).clone() {
                            <label class={classes!("block", "text-sm", "mb-2")}>
                                { "dedupe_window_seconds" }
                                <input
                                    type="number"
                                    value={cfg.dedupe_window_seconds.to_string()}
                                    class={classes!("mt-1", "w-full", "rounded-lg", "border", "border-[var(--border)]", "px-3", "py-2")}
                                    oninput={{
                                        let view_config = view_config.clone();
                                        Callback::from(move |event: InputEvent| {
                                            if let Some(target) = event.target_dyn_into::<HtmlInputElement>() {
                                                if let Ok(v) = target.value().parse::<u64>() {
                                                    let mut next = (*view_config).clone();
                                                    if let Some(cfg) = next.as_mut() {
                                                        cfg.dedupe_window_seconds = v;
                                                    }
                                                    view_config.set(next);
                                                }
                                            }
                                        })
                                    }}
                                />
                            </label>
                            <label class={classes!("block", "text-sm", "mb-2")}>
                                { "trend_default_days" }
                                <input
                                    type="number"
                                    value={cfg.trend_default_days.to_string()}
                                    class={classes!("mt-1", "w-full", "rounded-lg", "border", "border-[var(--border)]", "px-3", "py-2")}
                                    oninput={{
                                        let view_config = view_config.clone();
                                        Callback::from(move |event: InputEvent| {
                                            if let Some(target) = event.target_dyn_into::<HtmlInputElement>() {
                                                if let Ok(v) = target.value().parse::<usize>() {
                                                    let mut next = (*view_config).clone();
                                                    if let Some(cfg) = next.as_mut() {
                                                        cfg.trend_default_days = v;
                                                    }
                                                    view_config.set(next);
                                                }
                                            }
                                        })
                                    }}
                                />
                            </label>
                            <label class={classes!("block", "text-sm")}>
                                { "trend_max_days" }
                                <input
                                    type="number"
                                    value={cfg.trend_max_days.to_string()}
                                    class={classes!("mt-1", "w-full", "rounded-lg", "border", "border-[var(--border)]", "px-3", "py-2")}
                                    oninput={{
                                        let view_config = view_config.clone();
                                        Callback::from(move |event: InputEvent| {
                                            if let Some(target) = event.target_dyn_into::<HtmlInputElement>() {
                                                if let Ok(v) = target.value().parse::<usize>() {
                                                    let mut next = (*view_config).clone();
                                                    if let Some(cfg) = next.as_mut() {
                                                        cfg.trend_max_days = v;
                                                    }
                                                    view_config.set(next);
                                                }
                                            }
                                        })
                                    }}
                                />
                            </label>
                        } else {
                            <p class={classes!("text-sm", "text-[var(--muted)]", "m-0")}>{ "Unavailable" }</p>
                        }
                    </div>

                    <div class={classes!("rounded-[var(--radius)]", "border", "border-[var(--border)]", "p-3")}>
                        <h3 class={classes!("m-0", "mb-2", "text-sm", "uppercase", "tracking-[0.08em]", "text-[var(--muted)]")}>
                            { "Comment Runtime" }
                        </h3>
                        if let Some(cfg) = (*comment_config).clone() {
                            <label class={classes!("block", "text-sm", "mb-2")}>
                                { "submit_rate_limit_seconds" }
                                <input
                                    type="number"
                                    value={cfg.submit_rate_limit_seconds.to_string()}
                                    class={classes!("mt-1", "w-full", "rounded-lg", "border", "border-[var(--border)]", "px-3", "py-2")}
                                    oninput={{
                                        let comment_config = comment_config.clone();
                                        Callback::from(move |event: InputEvent| {
                                            if let Some(target) = event.target_dyn_into::<HtmlInputElement>() {
                                                if let Ok(v) = target.value().parse::<u64>() {
                                                    let mut next = (*comment_config).clone();
                                                    if let Some(cfg) = next.as_mut() {
                                                        cfg.submit_rate_limit_seconds = v;
                                                    }
                                                    comment_config.set(next);
                                                }
                                            }
                                        })
                                    }}
                                />
                            </label>
                            <label class={classes!("block", "text-sm", "mb-2")}>
                                { "list_default_limit" }
                                <input
                                    type="number"
                                    value={cfg.list_default_limit.to_string()}
                                    class={classes!("mt-1", "w-full", "rounded-lg", "border", "border-[var(--border)]", "px-3", "py-2")}
                                    oninput={{
                                        let comment_config = comment_config.clone();
                                        Callback::from(move |event: InputEvent| {
                                            if let Some(target) = event.target_dyn_into::<HtmlInputElement>() {
                                                if let Ok(v) = target.value().parse::<usize>() {
                                                    let mut next = (*comment_config).clone();
                                                    if let Some(cfg) = next.as_mut() {
                                                        cfg.list_default_limit = v;
                                                    }
                                                    comment_config.set(next);
                                                }
                                            }
                                        })
                                    }}
                                />
                            </label>
                            <label class={classes!("block", "text-sm")}>
                                { "cleanup_retention_days" }
                                <input
                                    type="number"
                                    value={cfg.cleanup_retention_days.to_string()}
                                    class={classes!("mt-1", "w-full", "rounded-lg", "border", "border-[var(--border)]", "px-3", "py-2")}
                                    oninput={{
                                        let comment_config = comment_config.clone();
                                        Callback::from(move |event: InputEvent| {
                                            if let Some(target) = event.target_dyn_into::<HtmlInputElement>() {
                                                if let Ok(v) = target.value().parse::<i64>() {
                                                    let mut next = (*comment_config).clone();
                                                    if let Some(cfg) = next.as_mut() {
                                                        cfg.cleanup_retention_days = v;
                                                    }
                                                    comment_config.set(next);
                                                }
                                            }
                                        })
                                    }}
                                />
                            </label>
                        } else {
                            <p class={classes!("text-sm", "text-[var(--muted)]", "m-0")}>{ "Unavailable" }</p>
                        }
                    </div>

                    <div class={classes!("rounded-[var(--radius)]", "border", "border-[var(--border)]", "p-3")}>
                        <h3 class={classes!("m-0", "mb-2", "text-sm", "uppercase", "tracking-[0.08em]", "text-[var(--muted)]")}>
                            { "API Behavior" }
                        </h3>
                        if let Some(cfg) = (*behavior_config).clone() {
                            <label class={classes!("block", "text-sm", "mb-2")}>
                                { "retention_days" }
                                <input
                                    type="number"
                                    value={cfg.retention_days.to_string()}
                                    class={classes!("mt-1", "w-full", "rounded-lg", "border", "border-[var(--border)]", "px-3", "py-2")}
                                    oninput={{
                                        let behavior_config = behavior_config.clone();
                                        Callback::from(move |event: InputEvent| {
                                            if let Some(target) = event.target_dyn_into::<HtmlInputElement>() {
                                                if let Ok(v) = target.value().parse::<i64>() {
                                                    let mut next = (*behavior_config).clone();
                                                    if let Some(cfg) = next.as_mut() {
                                                        cfg.retention_days = v;
                                                    }
                                                    behavior_config.set(next);
                                                }
                                            }
                                        })
                                    }}
                                />
                            </label>
                            <label class={classes!("block", "text-sm", "mb-2")}>
                                { "default_days" }
                                <input
                                    type="number"
                                    value={cfg.default_days.to_string()}
                                    class={classes!("mt-1", "w-full", "rounded-lg", "border", "border-[var(--border)]", "px-3", "py-2")}
                                    oninput={{
                                        let behavior_config = behavior_config.clone();
                                        Callback::from(move |event: InputEvent| {
                                            if let Some(target) = event.target_dyn_into::<HtmlInputElement>() {
                                                if let Ok(v) = target.value().parse::<usize>() {
                                                    let mut next = (*behavior_config).clone();
                                                    if let Some(cfg) = next.as_mut() {
                                                        cfg.default_days = v;
                                                    }
                                                    behavior_config.set(next);
                                                }
                                            }
                                        })
                                    }}
                                />
                            </label>
                            <label class={classes!("block", "text-sm")}>
                                { "max_days" }
                                <input
                                    type="number"
                                    value={cfg.max_days.to_string()}
                                    class={classes!("mt-1", "w-full", "rounded-lg", "border", "border-[var(--border)]", "px-3", "py-2")}
                                    oninput={{
                                        let behavior_config = behavior_config.clone();
                                        Callback::from(move |event: InputEvent| {
                                            if let Some(target) = event.target_dyn_into::<HtmlInputElement>() {
                                                if let Ok(v) = target.value().parse::<usize>() {
                                                    let mut next = (*behavior_config).clone();
                                                    if let Some(cfg) = next.as_mut() {
                                                        cfg.max_days = v;
                                                    }
                                                    behavior_config.set(next);
                                                }
                                            }
                                        })
                                    }}
                                />
                            </label>
                        } else {
                            <p class={classes!("text-sm", "text-[var(--muted)]", "m-0")}>{ "Unavailable" }</p>
                        }
                    </div>
                </div>
                <div class={classes!("mt-4")}>
                    <button class={classes!("btn-fluent-primary")} onclick={on_save_configs} disabled={*saving}>
                        <i class={classes!("fas", "fa-floppy-disk", "mr-2")} aria-hidden="true"></i>
                        { if *saving { "Saving..." } else { "Save Config" } }
                    </button>
                </div>
            </section>

            <section class={classes!(
                "bg-[var(--surface)]",
                "border",
                "border-[var(--border)]",
                "rounded-[var(--radius)]",
                "shadow-[var(--shadow)]",
                "p-5",
                "mb-5"
            )}>
                <div class={classes!("flex", "items-center", "gap-2", "mb-4", "flex-wrap")}>
                    <button class={if *active_tab == Some(AdminTab::Tasks) { classes!("btn-fluent-primary") } else { classes!("btn-fluent-secondary") }} onclick={tab_tasks}>{ "Tasks (Grouped)" }</button>
                    <button class={if *active_tab == Some(AdminTab::Published) { classes!("btn-fluent-primary") } else { classes!("btn-fluent-secondary") }} onclick={tab_published}>{ "Published" }</button>
                    <button class={if *active_tab == Some(AdminTab::Audit) { classes!("btn-fluent-primary") } else { classes!("btn-fluent-secondary") }} onclick={tab_audit}>{ "Audit Logs" }</button>
                    <button class={if *active_tab == Some(AdminTab::Behavior) { classes!("btn-fluent-primary") } else { classes!("btn-fluent-secondary") }} onclick={tab_behavior}>{ "API Behavior" }</button>
                    <button class={if *active_tab == Some(AdminTab::MusicWishes) { classes!("btn-fluent-primary") } else { classes!("btn-fluent-secondary") }} onclick={tab_music_wishes}>{ "Music Wishes" }</button>
                </div>

                if active_tab.is_none() {
                    <div class={classes!("flex", "flex-col", "items-center", "justify-center", "py-16", "text-[var(--muted)]")}>
                        <i class={classes!("fas", "fa-hand-pointer", "text-3xl", "mb-3", "opacity-40")} aria-hidden="true"></i>
                        <p class={classes!("m-0", "text-sm")}>{ "Select a tab to get started" }</p>
                    </div>
                } else if *active_tab == Some(AdminTab::Tasks) {
                    <div class="animate-[fadeIn_0.3s_ease]">
                    if tab_loading.contains(&AdminTab::Tasks) && task_groups.is_empty() {
                        <div class={classes!("flex", "justify-center", "py-8")}>
                            <LoadingSpinner size={SpinnerSize::Small} />
                        </div>
                    } else {
                    <>
                        <div class={classes!("flex", "items-center", "justify-between", "gap-3", "flex-wrap", "mb-4")}>
                            <h2 class={classes!("m-0", "text-lg", "font-semibold")}>
                                { format!("Task Groups: {} articles / {} tasks", task_groups.len(), grouped_total_tasks) }
                            </h2>
                            <div class={classes!("flex", "items-center", "gap-2", "flex-wrap")}>
                                <input
                                    type="text"
                                    value={(*status_filter).clone()}
                                    oninput={on_filter_change}
                                    placeholder="status filter: pending/approved/failed"
                                    class={classes!("rounded-lg", "border", "border-[var(--border)]", "px-3", "py-2", "text-sm", "w-[280px]")}
                                />
                                <button class={classes!("btn-fluent-secondary")} onclick={on_tasks_apply}>{ "Apply" }</button>
                            </div>
                        </div>
                        if tab_loading.contains(&AdminTab::Tasks) {
                            <div class={classes!("mb-3", "inline-flex", "items-center", "gap-2", "text-xs", "text-[var(--muted)]")}>
                                <LoadingSpinner size={SpinnerSize::Small} />
                                <span>{ "Loading tasks..." }</span>
                            </div>
                        }

                        <div class={classes!("mb-4", "text-sm", "text-[var(--muted)]", "flex", "gap-2", "flex-wrap")}>
                            { for grouped_status_counts.iter().map(|(status, count)| html! {
                                <span class={status_badge_class(status)}>{ format!("{}: {}", status, count) }</span>
                            }) }
                        </div>

                        <div class={classes!("grid", "gap-4")}>
                            { for (*task_groups).iter().map(|group| {
                                html! {
                                    <article class={classes!("rounded-[var(--radius)]", "border", "border-[var(--border)]", "p-3")}>
                                        <header class={classes!("mb-3", "flex", "items-center", "justify-between", "gap-2", "flex-wrap")}>
                                            <h3 class={classes!("m-0", "text-sm", "font-semibold")}>{ format!("article_id: {}", group.article_id) }</h3>
                                            <span class={classes!("text-xs", "text-[var(--muted)]")}>{ format!("{} tasks", group.total) }</span>
                                        </header>
                                        <div class={classes!("mb-3", "flex", "gap-2", "flex-wrap")}>
                                            { for group.status_counts.iter().map(|(status, count)| html! {
                                                <span class={status_badge_class(status)}>{ format!("{}: {}", status, count) }</span>
                                            }) }
                                        </div>
                                        <div class={classes!("overflow-x-auto")}>
                                            <table class={classes!("w-full", "text-sm")}>
                                                <thead>
                                                    <tr class={classes!("text-left", "text-[var(--muted)]")}>
                                                        <th class={classes!("py-2", "pr-3")}>{ "Task" }</th>
                                                        <th class={classes!("py-2", "pr-3")}>{ "Status" }</th>
                                                        <th class={classes!("py-2", "pr-3")}>{ "Attempts" }</th>
                                                        <th class={classes!("py-2", "pr-3")}>{ "Created" }</th>
                                                        <th class={classes!("py-2", "pr-3")}>{ "Actions" }</th>
                                                    </tr>
                                                </thead>
                                                <tbody>
                                                    { for group.tasks.iter().map(|task| {
                                                        let task_id = task.task_id.clone();
                                                        let status = task.status.clone();
                                                        let is_busy = task_action_inflight.contains(&task_id);
                                                        let can_approve = !is_busy && (status == "pending" || status == "failed");
                                                        let can_approve_run = !is_busy && (status == "pending" || status == "approved" || status == "failed");
                                                        let can_retry = !is_busy && status == "failed";
                                                        let can_reject = !is_busy && (status == "pending" || status == "approved" || status == "failed");
                                                        let can_delete = !is_busy && status != "running";

                                                        let select_click = {
                                                            let on_select_task = on_select_task.clone();
                                                            let task_id = task_id.clone();
                                                            Callback::from(move |_| on_select_task.emit(task_id.clone()))
                                                        };
                                                        let approve_click = {
                                                            let run_task_action = run_task_action.clone();
                                                            let task_id = task_id.clone();
                                                            Callback::from(move |_| run_task_action.emit((task_id.clone(), "approve".to_string())))
                                                        };
                                                        let approve_run_click = {
                                                            let run_task_action = run_task_action.clone();
                                                            let task_id = task_id.clone();
                                                            Callback::from(move |_| run_task_action.emit((task_id.clone(), "approve_run".to_string())))
                                                        };
                                                        let retry_click = {
                                                            let run_task_action = run_task_action.clone();
                                                            let task_id = task_id.clone();
                                                            Callback::from(move |_| run_task_action.emit((task_id.clone(), "retry".to_string())))
                                                        };
                                                        let reject_click = {
                                                            let run_task_action = run_task_action.clone();
                                                            let task_id = task_id.clone();
                                                            Callback::from(move |_| run_task_action.emit((task_id.clone(), "reject".to_string())))
                                                        };
                                                        let delete_click = {
                                                            let run_task_action = run_task_action.clone();
                                                            let task_id = task_id.clone();
                                                            Callback::from(move |_| run_task_action.emit((task_id.clone(), "delete".to_string())))
                                                        };

                                                        html! {
                                                            <tr class={classes!("border-t", "border-[var(--border)]")}>
                                                                <td class={classes!("py-2", "pr-3")}>
                                                                    <button class={classes!("text-[var(--primary)]", "underline")} onclick={select_click}>
                                                                        { task.task_id.clone() }
                                                                    </button>
                                                                </td>
                                                                <td class={classes!("py-2", "pr-3")}>
                                                                    <span class={status_badge_class(&status)}>{ status }</span>
                                                                </td>
                                                                <td class={classes!("py-2", "pr-3")}>{ task.attempt_count }</td>
                                                                <td class={classes!("py-2", "pr-3")}>{ format_ms(task.created_at) }</td>
                                                                <td class={classes!("py-2", "pr-3")}>
                                                                    <div class={classes!("flex", "gap-2", "flex-wrap")}>
                                                                        <button class={classes!("btn-fluent-secondary", "!px-2", "!py-1", "!text-xs")} onclick={approve_click} disabled={!can_approve}>{ "Approve" }</button>
                                                                        <button class={classes!("btn-fluent-primary", "!px-2", "!py-1", "!text-xs")} onclick={approve_run_click} disabled={!can_approve_run}>{ "Approve+Codex" }</button>
                                                                        <button class={classes!("btn-fluent-secondary", "!px-2", "!py-1", "!text-xs")} onclick={retry_click} disabled={!can_retry}>{ "Retry" }</button>
                                                                        <button class={classes!("btn-fluent-secondary", "!px-2", "!py-1", "!text-xs")} onclick={reject_click} disabled={!can_reject}>{ "Reject" }</button>
                                                                        <button class={classes!("btn-fluent-secondary", "!px-2", "!py-1", "!text-xs")} onclick={delete_click} disabled={!can_delete}>{ "Delete" }</button>
                                                                    </div>
                                                                </td>
                                                            </tr>
                                                        }
                                                    }) }
                                                </tbody>
                                            </table>
                                        </div>
                                    </article>
                                }
                            }) }
                        </div>

                        if let Some(task) = (*selected_task).clone() {
                            <div class={classes!("mt-4", "rounded-[var(--radius)]", "border", "border-[var(--border)]", "p-4")}>
                                <h3 class={classes!("m-0", "mb-3", "text-sm", "uppercase", "tracking-[0.08em]", "text-[var(--muted)]")}>
                                    { format!("Task Detail: {}", task.task_id) }
                                </h3>
                                <p class={classes!("m-0", "mb-2", "text-sm", "text-[var(--muted)]")}>
                                    { format!("status={} created={} updated={}", task.status, format_ms(task.created_at), format_ms(task.updated_at)) }
                                </p>
                                <label class={classes!("block", "text-sm", "mb-2")}>
                                    { "comment_text" }
                                    <textarea
                                        class={classes!("mt-1", "w-full", "min-h-[120px]", "rounded-lg", "border", "border-[var(--border)]", "px-3", "py-2")}
                                        value={task.comment_text.clone()}
                                        oninput={on_selected_task_comment_change}
                                    />
                                </label>
                                <label class={classes!("block", "text-sm", "mb-2")}>
                                    { "admin_note" }
                                    <textarea
                                        class={classes!("mt-1", "w-full", "min-h-[90px]", "rounded-lg", "border", "border-[var(--border)]", "px-3", "py-2")}
                                        value={task.admin_note.clone().unwrap_or_default()}
                                        oninput={on_selected_task_note_change}
                                    />
                                </label>
                                <p class={classes!("m-0", "mb-2", "text-sm", "text-[var(--muted)]")}>{ format!("selected_text={}", task.selected_text.clone().unwrap_or_default()) }</p>
                                <p class={classes!("m-0", "mb-3", "text-sm", "text-[var(--muted)]")}>{ format!("failure_reason={}", task.failure_reason.clone().unwrap_or_default()) }</p>
                                <button class={classes!("btn-fluent-primary")} onclick={on_save_task}>{ "Save Task Update" }</button>

                                if let Some(ai_output) = (*selected_task_ai_output).clone() {
                                    <div class={classes!("mt-4", "rounded-[var(--radius)]", "border", "border-[var(--border)]", "p-3")}>
                                        <div class={classes!("mb-2", "flex", "items-center", "justify-between", "gap-2", "flex-wrap")}>
                                            <h4 class={classes!("m-0", "text-sm", "uppercase", "tracking-[0.08em]", "text-[var(--muted)]")}>
                                                { format!("AI Runs ({})", ai_output.runs.len()) }
                                            </h4>
                                            <Link<Route>
                                                to={Route::AdminCommentRuns { task_id: task.task_id.clone() }}
                                                classes={classes!("btn-fluent-secondary", "!px-2", "!py-1", "!text-xs")}
                                            >
                                                { "Open Stream Page" }
                                            </Link<Route>>
                                        </div>

                                        if ai_output.runs.is_empty() {
                                            <p class={classes!("m-0", "text-sm", "text-[var(--muted)]")}>
                                                { "No AI run records for this task yet." }
                                            </p>
                                        } else {
                                            <div class={classes!("mb-3", "flex", "gap-2", "flex-wrap")}>
                                                { for ai_output.runs.iter().map(|run| {
                                                    let run_id = run.run_id.clone();
                                                    let selected = ai_output.selected_run_id.as_deref() == Some(run_id.as_str());
                                                    let click = {
                                                        let on_select_task_ai_run = on_select_task_ai_run.clone();
                                                        let run_id = run_id.clone();
                                                        Callback::from(move |_| on_select_task_ai_run.emit(run_id.clone()))
                                                    };
                                                    html! {
                                                        <button
                                                            class={if selected { classes!("btn-fluent-primary", "!px-2", "!py-1", "!text-xs") } else { classes!("btn-fluent-secondary", "!px-2", "!py-1", "!text-xs") }}
                                                            onclick={click}
                                                        >
                                                            { format!("{} · {}", run.status, run.run_id) }
                                                        </button>
                                                    }
                                                }) }
                                            </div>
                                        }

                                        <p class={classes!("m-0", "mb-2", "text-xs", "text-[var(--muted)]")}>
                                            { format!("stream chunks captured: {}", ai_output.chunks.len()) }
                                        </p>
                                        <ul class={classes!("m-0", "p-0", "list-none", "flex", "flex-col", "gap-2")}>
                                            { for ai_output.chunks.iter().rev().take(10).rev().map(|chunk| {
                                                let stream_badge = if chunk.stream == "stderr" {
                                                    classes!("inline-flex", "rounded-full", "px-2", "py-0.5", "text-xs", "font-semibold", "bg-red-500/15", "text-red-700", "dark:text-red-200")
                                                } else {
                                                    classes!("inline-flex", "rounded-full", "px-2", "py-0.5", "text-xs", "font-semibold", "bg-sky-500/15", "text-sky-700", "dark:text-sky-200")
                                                };
                                                html! {
                                                    <li class={classes!("rounded-[var(--radius)]", "border", "border-[var(--border)]", "p-2")}>
                                                        <div class={classes!("mb-1", "flex", "items-center", "gap-2", "flex-wrap")}>
                                                            <span class={stream_badge}>{ chunk.stream.clone() }</span>
                                                            <span class={classes!("text-xs", "text-[var(--muted)]")}>{ format!("batch={}", chunk.batch_index) }</span>
                                                        </div>
                                                        <pre class={classes!("m-0", "text-xs", "font-mono", "whitespace-pre-wrap", "break-words")}>{ chunk.content.clone() }</pre>
                                                    </li>
                                                }
                                            }) }
                                        </ul>
                                    </div>
                                }
                            </div>
                        }
                    </>
                    }
                    <div class={classes!("mt-4")}>
                        <Pagination current_page={*tasks_page} total_pages={tasks_total_pages} on_page_change={on_tasks_page_change} />
                    </div>
                    </div>
                } else if *active_tab == Some(AdminTab::Published) {
                    <div class="animate-[fadeIn_0.3s_ease]">
                    if tab_loading.contains(&AdminTab::Published) && published_comments.is_empty() {
                        <div class={classes!("flex", "justify-center", "py-8")}>
                            <LoadingSpinner size={SpinnerSize::Small} />
                        </div>
                    } else {
                    <>
                        <h2 class={classes!("m-0", "mb-3", "text-lg", "font-semibold")}>
                            { format!("Published Comments ({})", published_comments.len()) }
                        </h2>
                        if tab_loading.contains(&AdminTab::Published) {
                            <div class={classes!("mb-3", "inline-flex", "items-center", "gap-2", "text-xs", "text-[var(--muted)]")}>
                                <LoadingSpinner size={SpinnerSize::Small} />
                                <span>{ "Loading published comments..." }</span>
                            </div>
                        }
                        <div class={classes!("overflow-x-auto")}>
                            <table class={classes!("w-full", "text-sm")}>
                                <thead>
                                    <tr class={classes!("text-left", "text-[var(--muted)]")}>
                                        <th class={classes!("py-2", "pr-3")}>{ "Comment" }</th>
                                        <th class={classes!("py-2", "pr-3")}>{ "Article" }</th>
                                        <th class={classes!("py-2", "pr-3")}>{ "Task" }</th>
                                        <th class={classes!("py-2", "pr-3")}>{ "Published At" }</th>
                                        <th class={classes!("py-2", "pr-3")}>{ "Actions" }</th>
                                    </tr>
                                </thead>
                                <tbody>
                                    { for (*published_comments).iter().map(|comment| {
                                        let select_click = {
                                            let on_select_published = on_select_published.clone();
                                            let comment = comment.clone();
                                            Callback::from(move |_| on_select_published.emit(comment.clone()))
                                        };
                                        let delete_click = {
                                            let delete_published_action = delete_published_action.clone();
                                            let comment_id = comment.comment_id.clone();
                                            Callback::from(move |_| delete_published_action.emit(comment_id.clone()))
                                        };
                                        html! {
                                            <tr class={classes!("border-t", "border-[var(--border)]")}>
                                                <td class={classes!("py-2", "pr-3")}>{ comment.comment_id.clone() }</td>
                                                <td class={classes!("py-2", "pr-3")}>{ comment.article_id.clone() }</td>
                                                <td class={classes!("py-2", "pr-3")}>{ comment.task_id.clone() }</td>
                                                <td class={classes!("py-2", "pr-3")}>{ format_ms(comment.published_at) }</td>
                                                <td class={classes!("py-2", "pr-3") }>
                                                    <div class={classes!("flex", "gap-2", "flex-wrap")}>
                                                        <button class={classes!("btn-fluent-secondary", "!px-2", "!py-1", "!text-xs")} onclick={select_click}>{ "Update" }</button>
                                                        <button class={classes!("btn-fluent-secondary", "!px-2", "!py-1", "!text-xs")} onclick={delete_click}>{ "Delete" }</button>
                                                    </div>
                                                </td>
                                            </tr>
                                        }
                                    }) }
                                </tbody>
                            </table>
                        </div>

                        if let Some(comment) = (*selected_published).clone() {
                            <div class={classes!("mt-4", "rounded-[var(--radius)]", "border", "border-[var(--border)]", "p-4")}>
                                <h3 class={classes!("m-0", "mb-3", "text-sm", "uppercase", "tracking-[0.08em]", "text-[var(--muted)]")}>
                                    { format!("Published Detail: {}", comment.comment_id) }
                                </h3>
                                <label class={classes!("block", "text-sm", "mb-2")}>
                                    { "comment_text" }
                                    <textarea
                                        class={classes!("mt-1", "w-full", "min-h-[100px]", "rounded-lg", "border", "border-[var(--border)]", "px-3", "py-2")}
                                        value={comment.comment_text.clone()}
                                        oninput={on_selected_published_comment_change}
                                    />
                                </label>
                                <label class={classes!("block", "text-sm", "mb-2")}>
                                    { "ai_reply_markdown" }
                                    <textarea
                                        class={classes!("mt-1", "w-full", "min-h-[140px]", "rounded-lg", "border", "border-[var(--border)]", "px-3", "py-2")}
                                        value={comment.ai_reply_markdown.clone().unwrap_or_default()}
                                        oninput={on_selected_published_ai_change}
                                    />
                                </label>
                                <button class={classes!("btn-fluent-primary")} onclick={on_save_published}>{ "Save Published Update" }</button>
                            </div>
                        }
                    </>
                    }
                    <div class={classes!("mt-4")}>
                        <Pagination current_page={*published_page} total_pages={published_total_pages} on_page_change={on_published_page_change} />
                    </div>
                    </div>
                } else if *active_tab == Some(AdminTab::Audit) {
                    <div class="animate-[fadeIn_0.3s_ease]">
                    if tab_loading.contains(&AdminTab::Audit) && audit_logs.is_empty() {
                        <div class={classes!("flex", "justify-center", "py-8")}>
                            <LoadingSpinner size={SpinnerSize::Small} />
                        </div>
                    } else {
                    <>
                        <div class={classes!("flex", "items-center", "justify-between", "gap-2", "flex-wrap", "mb-3")}>
                            <h2 class={classes!("m-0", "text-lg", "font-semibold")}>{ format!("Audit Logs ({})", audit_logs.len()) }</h2>
                            <div class={classes!("flex", "items-center", "gap-2", "flex-wrap")}>
                                <input
                                    type="text"
                                    value={(*audit_task_filter).clone()}
                                    oninput={on_audit_task_filter_change}
                                    placeholder="task_id"
                                    class={classes!("rounded-lg", "border", "border-[var(--border)]", "px-3", "py-2", "text-sm", "w-[180px]")}
                                />
                                <input
                                    type="text"
                                    value={(*audit_action_filter).clone()}
                                    oninput={on_audit_action_filter_change}
                                    placeholder="action"
                                    class={classes!("rounded-lg", "border", "border-[var(--border)]", "px-3", "py-2", "text-sm", "w-[150px]")}
                                />
                                <button class={classes!("btn-fluent-secondary")} onclick={on_refresh_audit_click}>{ "Apply" }</button>
                            </div>
                        </div>
                        if tab_loading.contains(&AdminTab::Audit) {
                            <div class={classes!("mb-3", "inline-flex", "items-center", "gap-2", "text-xs", "text-[var(--muted)]")}>
                                <LoadingSpinner size={SpinnerSize::Small} />
                                <span>{ "Loading audit logs..." }</span>
                            </div>
                        }

                        <div class={classes!("overflow-x-auto")}>
                            <table class={classes!("w-full", "text-sm")}>
                                <thead>
                                    <tr class={classes!("text-left", "text-[var(--muted)]")}>
                                        <th class={classes!("py-2", "pr-3")}>{ "Log" }</th>
                                        <th class={classes!("py-2", "pr-3")}>{ "Task" }</th>
                                        <th class={classes!("py-2", "pr-3")}>{ "Action" }</th>
                                        <th class={classes!("py-2", "pr-3")}>{ "Operator" }</th>
                                        <th class={classes!("py-2", "pr-3")}>{ "Created" }</th>
                                    </tr>
                                </thead>
                                <tbody>
                                    { for (*audit_logs).iter().map(|log| html! {
                                        <tr class={classes!("border-t", "border-[var(--border)]")}>
                                            <td class={classes!("py-2", "pr-3")}>{ log.log_id.clone() }</td>
                                            <td class={classes!("py-2", "pr-3")}>{ log.task_id.clone() }</td>
                                            <td class={classes!("py-2", "pr-3")}>{ log.action.clone() }</td>
                                            <td class={classes!("py-2", "pr-3")}>{ log.operator.clone() }</td>
                                            <td class={classes!("py-2", "pr-3")}>{ format_ms(log.created_at) }</td>
                                        </tr>
                                    }) }
                                </tbody>
                            </table>
                        </div>
                    </>
                    }
                    <div class={classes!("mt-4")}>
                        <Pagination current_page={*audit_page} total_pages={audit_total_pages} on_page_change={on_audit_page_change} />
                    </div>
                    </div>
                } else if *active_tab == Some(AdminTab::Behavior) {
                    <div class="animate-[fadeIn_0.3s_ease]">
                        <div class={classes!("flex", "items-center", "justify-between", "gap-2", "flex-wrap", "mb-3")}>
                            <h2 class={classes!("m-0", "text-lg", "font-semibold")}>{ "API Behavior Analytics" }</h2>
                            <div class={classes!("flex", "items-center", "gap-2", "flex-wrap")}>
                                <input
                                    type="number"
                                    value={(*behavior_days).clone()}
                                    oninput={on_behavior_days_change}
                                    placeholder="days"
                                    disabled={!(*behavior_date).is_empty()}
                                    class={classes!("rounded-lg", "border", "border-[var(--border)]", "px-3", "py-2", "text-sm", "w-[110px]")}
                                />
                                <input
                                    type="date"
                                    value={(*behavior_date).clone()}
                                    oninput={on_behavior_date_change}
                                    class={classes!("rounded-lg", "border", "border-[var(--border)]", "px-3", "py-2", "text-sm", "w-[160px]")}
                                />
                                <input
                                    type="text"
                                    value={(*behavior_path_filter).clone()}
                                    oninput={on_behavior_path_filter_change}
                                    placeholder="path contains"
                                    class={classes!("rounded-lg", "border", "border-[var(--border)]", "px-3", "py-2", "text-sm", "w-[170px]")}
                                />
                                <input
                                    type="text"
                                    value={(*behavior_page_filter).clone()}
                                    oninput={on_behavior_page_filter_change}
                                    placeholder="page contains"
                                    class={classes!("rounded-lg", "border", "border-[var(--border)]", "px-3", "py-2", "text-sm", "w-[170px]")}
                                />
                                <input
                                    type="text"
                                    value={(*behavior_device_filter).clone()}
                                    oninput={on_behavior_device_filter_change}
                                    placeholder="device"
                                    class={classes!("rounded-lg", "border", "border-[var(--border)]", "px-3", "py-2", "text-sm", "w-[120px]")}
                                />
                                <input
                                    type="number"
                                    value={(*behavior_status_filter).clone()}
                                    oninput={on_behavior_status_filter_change}
                                    placeholder="status"
                                    class={classes!("rounded-lg", "border", "border-[var(--border)]", "px-3", "py-2", "text-sm", "w-[110px]")}
                                />
                                <button class={classes!("btn-fluent-secondary")} onclick={on_behavior_apply.clone()}>{ "Apply" }</button>
                                <button class={classes!("btn-fluent-secondary")} onclick={on_behavior_cleanup}>{ "Cleanup Old Logs" }</button>
                            </div>
                        </div>

                        if let Some(overview) = (*behavior_overview).clone() {
                            <div class={classes!("grid", "gap-3", "md:grid-cols-4", "mb-4")}>
                                <article class={classes!("rounded-[var(--radius)]", "border", "border-[var(--border)]", "p-3")}>
                                    <p class={classes!("m-0", "text-xs", "uppercase", "text-[var(--muted)]")}>{ "Events" }</p>
                                    <p class={classes!("m-0", "text-lg", "font-semibold")}>{ overview.total_events }</p>
                                </article>
                                <article class={classes!("rounded-[var(--radius)]", "border", "border-[var(--border)]", "p-3")}>
                                    <p class={classes!("m-0", "text-xs", "uppercase", "text-[var(--muted)]")}>{ "Unique IPs" }</p>
                                    <p class={classes!("m-0", "text-lg", "font-semibold")}>{ overview.unique_ips }</p>
                                </article>
                                <article class={classes!("rounded-[var(--radius)]", "border", "border-[var(--border)]", "p-3")}>
                                    <p class={classes!("m-0", "text-xs", "uppercase", "text-[var(--muted)]")}>{ "Unique Pages" }</p>
                                    <p class={classes!("m-0", "text-lg", "font-semibold")}>{ overview.unique_pages }</p>
                                </article>
                                <article class={classes!("rounded-[var(--radius)]", "border", "border-[var(--border)]", "p-3")}>
                                    <p class={classes!("m-0", "text-xs", "uppercase", "text-[var(--muted)]")}>{ "Avg Latency" }</p>
                                    <p class={classes!("m-0", "text-lg", "font-semibold")}>{ format!("{:.1} ms", overview.avg_latency_ms) }</p>
                                </article>
                            </div>

                            <ViewTrendChart points={to_view_points(&overview.timeseries)} empty_text={"No behavior trend data".to_string()} />

                            <div class={classes!("grid", "gap-3", "md:grid-cols-2", "xl:grid-cols-3", "mt-4", "mb-4")}>
                                <article class={classes!("rounded-[var(--radius)]", "border", "border-[var(--border)]", "p-3")}>
                                    <h3 class={classes!("m-0", "mb-2", "text-sm", "font-semibold")}>{ "Top Endpoints" }</h3>
                                    <ul class={classes!("m-0", "p-0", "list-none", "space-y-1", "text-sm")}>
                                        { for overview.top_endpoints.iter().map(|item| html! {
                                            <li class={classes!("flex", "items-center", "justify-between", "gap-2")}>
                                                <span class={classes!("truncate", "text-[var(--muted)]")}>{ item.key.clone() }</span>
                                                <span class={classes!("font-semibold")}>{ item.count }</span>
                                            </li>
                                        }) }
                                    </ul>
                                </article>
                                <article class={classes!("rounded-[var(--radius)]", "border", "border-[var(--border)]", "p-3")}>
                                    <h3 class={classes!("m-0", "mb-2", "text-sm", "font-semibold")}>{ "Top Pages" }</h3>
                                    <ul class={classes!("m-0", "p-0", "list-none", "space-y-1", "text-sm")}>
                                        { for overview.top_pages.iter().map(|item| html! {
                                            <li class={classes!("flex", "items-center", "justify-between", "gap-2")}>
                                                <span class={classes!("truncate", "text-[var(--muted)]")}>{ item.key.clone() }</span>
                                                <span class={classes!("font-semibold")}>{ item.count }</span>
                                            </li>
                                        }) }
                                    </ul>
                                </article>
                                <article class={classes!("rounded-[var(--radius)]", "border", "border-[var(--border)]", "p-3")}>
                                    <h3 class={classes!("m-0", "mb-2", "text-sm", "font-semibold")}>{ "Device Distribution" }</h3>
                                    <ul class={classes!("m-0", "p-0", "list-none", "space-y-1", "text-sm")}>
                                        { for overview.device_distribution.iter().map(|item| html! {
                                            <li class={classes!("flex", "items-center", "justify-between", "gap-2")}>
                                                <span class={classes!("truncate", "text-[var(--muted)]")}>{ item.key.clone() }</span>
                                                <span class={classes!("font-semibold")}>{ item.count }</span>
                                            </li>
                                        }) }
                                    </ul>
                                </article>
                                <article class={classes!("rounded-[var(--radius)]", "border", "border-[var(--border)]", "p-3")}>
                                    <h3 class={classes!("m-0", "mb-2", "text-sm", "font-semibold")}>{ "Browser Distribution" }</h3>
                                    <ul class={classes!("m-0", "p-0", "list-none", "space-y-1", "text-sm")}>
                                        { for overview.browser_distribution.iter().map(|item| html! {
                                            <li class={classes!("flex", "items-center", "justify-between", "gap-2")}>
                                                <span class={classes!("truncate", "text-[var(--muted)]")}>{ item.key.clone() }</span>
                                                <span class={classes!("font-semibold")}>{ item.count }</span>
                                            </li>
                                        }) }
                                    </ul>
                                </article>
                                <article class={classes!("rounded-[var(--radius)]", "border", "border-[var(--border)]", "p-3")}>
                                    <h3 class={classes!("m-0", "mb-2", "text-sm", "font-semibold")}>{ "OS Distribution" }</h3>
                                    <ul class={classes!("m-0", "p-0", "list-none", "space-y-1", "text-sm")}>
                                        { for overview.os_distribution.iter().map(|item| html! {
                                            <li class={classes!("flex", "items-center", "justify-between", "gap-2")}>
                                                <span class={classes!("truncate", "text-[var(--muted)]")}>{ item.key.clone() }</span>
                                                <span class={classes!("font-semibold")}>{ item.count }</span>
                                            </li>
                                        }) }
                                    </ul>
                                </article>
                                <article class={classes!("rounded-[var(--radius)]", "border", "border-[var(--border)]", "p-3")}>
                                    <h3 class={classes!("m-0", "mb-2", "text-sm", "font-semibold")}>{ "Region Distribution" }</h3>
                                    <ul class={classes!("m-0", "p-0", "list-none", "space-y-1", "text-sm")}>
                                        { for overview.region_distribution.iter().map(|item| html! {
                                            <li class={classes!("flex", "items-center", "justify-between", "gap-2")}>
                                                <span class={classes!("truncate", "text-[var(--muted)]")}>{ item.key.clone() }</span>
                                                <span class={classes!("font-semibold")}>{ item.count }</span>
                                            </li>
                                        }) }
                                    </ul>
                                </article>
                            </div>
                        } else if *behavior_overview_loading {
                            // Skeleton for overview cards
                            <div class={classes!("grid", "gap-3", "md:grid-cols-4", "mb-4")}>
                                { for (0..4).map(|_| html! {
                                    <article class={classes!("rounded-[var(--radius)]", "border", "border-[var(--border)]", "p-3")}>
                                        <div class="h-3 w-16 bg-[var(--border)] rounded animate-pulse mb-2"></div>
                                        <div class="h-5 w-24 bg-[var(--border)] rounded animate-pulse"></div>
                                    </article>
                                }) }
                            </div>
                            // Skeleton for distribution cards
                            <div class={classes!("grid", "gap-3", "md:grid-cols-2", "xl:grid-cols-3", "mt-4", "mb-4")}>
                                { for (0..6).map(|_| html! {
                                    <article class={classes!("rounded-[var(--radius)]", "border", "border-[var(--border)]", "p-3")}>
                                        <div class="h-4 w-28 bg-[var(--border)] rounded animate-pulse mb-3"></div>
                                        { for (0..4).map(|_| html! {
                                            <div class="h-3 w-full bg-[var(--border)] rounded animate-pulse mb-2"></div>
                                        }) }
                                    </article>
                                }) }
                            </div>
                        } else {
                            <p class={classes!("m-0", "text-sm", "text-[var(--muted)]", "mb-4")}>{ "Behavior overview unavailable." }</p>
                        }

                        <h3 class={classes!("m-0", "mb-2", "text-sm", "uppercase", "tracking-[0.08em]", "text-[var(--muted)]")}>
                            {
                                if (*behavior_date).is_empty() {
                                    format!("Recent Events ({}/{})", behavior_events.len(), *behavior_total)
                                } else {
                                    format!("Events for {} ({}/{})", *behavior_date, behavior_events.len(), *behavior_total)
                                }
                            }
                        </h3>
                        if *behavior_events_loading {
                            <div class={classes!("mb-2", "inline-flex", "items-center", "gap-2", "text-xs", "text-[var(--muted)]")}>
                                <LoadingSpinner size={SpinnerSize::Small} />
                                <span>{ "Loading events..." }</span>
                            </div>
                        }
                        if *behavior_events_loading && behavior_events.is_empty() {
                            <div class={classes!("flex", "justify-center", "py-8")}>
                                <LoadingSpinner size={SpinnerSize::Small} />
                            </div>
                        } else {
                        <div class={classes!("overflow-x-auto")}>
                            <table class={classes!("w-full", "text-sm")}>
                                <thead>
                                    <tr class={classes!("text-left", "text-[var(--muted)]")}>
                                        <th class={classes!("py-2", "pr-3")}>{ "Time" }</th>
                                        <th class={classes!("py-2", "pr-3")}>{ "Page" }</th>
                                        <th class={classes!("py-2", "pr-3")}>{ "API" }</th>
                                        <th class={classes!("py-2", "pr-3")}>{ "Status" }</th>
                                        <th class={classes!("py-2", "pr-3")}>{ "Device" }</th>
                                        <th class={classes!("py-2", "pr-3")}>{ "Browser/OS" }</th>
                                        <th class={classes!("py-2", "pr-3")}>{ "IP/Region" }</th>
                                        <th class={classes!("py-2", "pr-3")}>{ "Latency" }</th>
                                    </tr>
                                </thead>
                                <tbody>
                                    { for (*behavior_events).iter().map(|event| {
                                        html! {
                                            <tr class={classes!("border-t", "border-[var(--border)]")}>
                                                <td class={classes!("py-2", "pr-3", "whitespace-nowrap")}>{ format_ms(event.occurred_at) }</td>
                                                <td class={classes!("py-2", "pr-3", "max-w-[220px]", "truncate")} title={event.page_path.clone()}>{ event.page_path.clone() }</td>
                                                <td class={classes!("py-2", "pr-3", "max-w-[260px]", "truncate")} title={format!("{} {}?{}", event.method, event.path, event.query)}>{ format!("{} {}", event.method, event.path) }</td>
                                                <td class={classes!("py-2", "pr-3")}>{ event.status_code }</td>
                                                <td class={classes!("py-2", "pr-3")}>{ event.device_type.clone() }</td>
                                                <td class={classes!("py-2", "pr-3", "whitespace-nowrap")}>{ format!("{}/{}", event.browser_family, event.os_family) }</td>
                                                <td class={classes!("py-2", "pr-3", "whitespace-nowrap")}>{ format!("{}/{}", event.client_ip, event.ip_region) }</td>
                                                <td class={classes!("py-2", "pr-3", "whitespace-nowrap")}>{ format!("{} ms", event.latency_ms) }</td>
                                            </tr>
                                        }
                                    }) }
                                </tbody>
                            </table>
                        </div>
                        }
                        <div class={classes!("mt-4")}>
                            <Pagination current_page={*behavior_page} total_pages={behavior_total_pages} on_page_change={on_behavior_page_change} />
                        </div>
                    </div>
                } else if *active_tab == Some(AdminTab::MusicWishes) {
                    <div class="animate-[fadeIn_0.3s_ease]">
                    if tab_loading.contains(&AdminTab::MusicWishes) && music_wishes.is_empty() {
                        <div class={classes!("flex", "justify-center", "py-8")}>
                            <LoadingSpinner size={SpinnerSize::Small} />
                        </div>
                    } else {
                    <>
                        <div class={classes!("flex", "items-center", "justify-between", "gap-3", "flex-wrap", "mb-4")}>
                            <h2 class={classes!("m-0", "text-lg", "font-semibold")}>
                                { format!("Music Wishes ({})", music_wishes.len()) }
                            </h2>
                            <button class={classes!("btn-fluent-secondary")} onclick={
                                let r = refresh_music_wishes.clone();
                                Callback::from(move |_| r.emit(None))
                            }>{ "Refresh" }</button>
                        </div>
                        if tab_loading.contains(&AdminTab::MusicWishes) {
                            <div class={classes!("mb-3", "inline-flex", "items-center", "gap-2", "text-xs", "text-[var(--muted)]")}>
                                <LoadingSpinner size={SpinnerSize::Small} />
                                <span>{ "Loading music wishes..." }</span>
                            </div>
                        }
                        if music_wishes.is_empty() {
                            <p class={classes!("m-0", "text-sm", "text-[var(--muted)]")}>{ "No wishes yet." }</p>
                        } else {
                            <div class={classes!("overflow-x-auto")}>
                                <table class={classes!("w-full", "text-sm")}>
                                    <thead>
                                        <tr class={classes!("text-left", "text-[var(--muted)]")}>
                                            <th class={classes!("py-2", "pr-3")}>{ "Song" }</th>
                                            <th class={classes!("py-2", "pr-3")}>{ "Artist" }</th>
                                            <th class={classes!("py-2", "pr-3")}>{ "Nickname" }</th>
                                            <th class={classes!("py-2", "pr-3")}>{ "Status" }</th>
                                            <th class={classes!("py-2", "pr-3")}>{ "Region" }</th>
                                            <th class={classes!("py-2", "pr-3")}>{ "Created" }</th>
                                            <th class={classes!("py-2", "pr-3")}>{ "Actions" }</th>
                                        </tr>
                                    </thead>
                                    <tbody>
                                        { for (*music_wishes).iter().map(|wish| {
                                            let wid = wish.wish_id.clone();
                                            let inflight = music_wish_action_inflight.contains(&wid);
                                            let status = wish.status.clone();

                                            let on_approve = {
                                                let wid = wid.clone();
                                                let music_wishes = music_wishes.clone();
                                                let music_wish_action_inflight = music_wish_action_inflight.clone();
                                                let load_error = load_error.clone();
                                                Callback::from(move |_| {
                                                    let wid = wid.clone();
                                                    let music_wishes = music_wishes.clone();
                                                    let inflight = music_wish_action_inflight.clone();
                                                    let load_error = load_error.clone();
                                                    let mut s = (*inflight).clone();
                                                    s.insert(wid.clone());
                                                    inflight.set(s);
                                                    wasm_bindgen_futures::spawn_local(async move {
                                                        match admin_approve_and_run_music_wish(&wid, None).await {
                                                            Ok(updated) => {
                                                                let mut list = (*music_wishes).clone();
                                                                if let Some(item) = list.iter_mut().find(|w| w.wish_id == updated.wish_id) {
                                                                    *item = updated;
                                                                }
                                                                music_wishes.set(list);
                                                                load_error.set(None);
                                                            },
                                                            Err(err) => load_error.set(Some(format!("Approve failed: {}", err))),
                                                        }
                                                        let mut s = (*inflight).clone();
                                                        s.remove(&wid);
                                                        inflight.set(s);
                                                    });
                                                })
                                            };

                                            let on_reject = {
                                                let wid = wid.clone();
                                                let music_wishes = music_wishes.clone();
                                                let music_wish_action_inflight = music_wish_action_inflight.clone();
                                                let load_error = load_error.clone();
                                                Callback::from(move |_| {
                                                    let wid = wid.clone();
                                                    let music_wishes = music_wishes.clone();
                                                    let inflight = music_wish_action_inflight.clone();
                                                    let load_error = load_error.clone();
                                                    let mut s = (*inflight).clone();
                                                    s.insert(wid.clone());
                                                    inflight.set(s);
                                                    wasm_bindgen_futures::spawn_local(async move {
                                                        match admin_reject_music_wish(&wid, None).await {
                                                            Ok(updated) => {
                                                                let mut list = (*music_wishes).clone();
                                                                if let Some(item) = list.iter_mut().find(|w| w.wish_id == updated.wish_id) {
                                                                    *item = updated;
                                                                }
                                                                music_wishes.set(list);
                                                                load_error.set(None);
                                                            },
                                                            Err(err) => load_error.set(Some(format!("Reject failed: {}", err))),
                                                        }
                                                        let mut s = (*inflight).clone();
                                                        s.remove(&wid);
                                                        inflight.set(s);
                                                    });
                                                })
                                            };

                                            let on_retry = {
                                                let wid = wid.clone();
                                                let music_wishes = music_wishes.clone();
                                                let music_wish_action_inflight = music_wish_action_inflight.clone();
                                                let load_error = load_error.clone();
                                                Callback::from(move |_| {
                                                    let wid = wid.clone();
                                                    let music_wishes = music_wishes.clone();
                                                    let inflight = music_wish_action_inflight.clone();
                                                    let load_error = load_error.clone();
                                                    let mut s = (*inflight).clone();
                                                    s.insert(wid.clone());
                                                    inflight.set(s);
                                                    wasm_bindgen_futures::spawn_local(async move {
                                                        match admin_retry_music_wish(&wid).await {
                                                            Ok(updated) => {
                                                                let mut list = (*music_wishes).clone();
                                                                if let Some(item) = list.iter_mut().find(|w| w.wish_id == updated.wish_id) {
                                                                    *item = updated;
                                                                }
                                                                music_wishes.set(list);
                                                                load_error.set(None);
                                                            },
                                                            Err(err) => load_error.set(Some(format!("Retry failed: {}", err))),
                                                        }
                                                        let mut s = (*inflight).clone();
                                                        s.remove(&wid);
                                                        inflight.set(s);
                                                    });
                                                })
                                            };

                                            let on_delete = {
                                                let wid = wid.clone();
                                                let music_wishes = music_wishes.clone();
                                                let music_wish_action_inflight = music_wish_action_inflight.clone();
                                                let load_error = load_error.clone();
                                                Callback::from(move |_| {
                                                    let wid = wid.clone();
                                                    let music_wishes = music_wishes.clone();
                                                    let inflight = music_wish_action_inflight.clone();
                                                    let load_error = load_error.clone();
                                                    let mut s = (*inflight).clone();
                                                    s.insert(wid.clone());
                                                    inflight.set(s);
                                                    wasm_bindgen_futures::spawn_local(async move {
                                                        match admin_delete_music_wish(&wid).await {
                                                            Ok(()) => {
                                                                let list: Vec<_> = (*music_wishes).iter().filter(|w| w.wish_id != wid).cloned().collect();
                                                                music_wishes.set(list);
                                                                load_error.set(None);
                                                            },
                                                            Err(err) => load_error.set(Some(format!("Delete failed: {}", err))),
                                                        }
                                                        let mut s = (*inflight).clone();
                                                        s.remove(&wid);
                                                        inflight.set(s);
                                                    });
                                                })
                                            };

                                            html! {
                                                <tr class={classes!("border-t", "border-[var(--border)]")}>
                                                    <td class={classes!("py-2", "pr-3", "max-w-[180px]", "truncate")} title={wish.song_name.clone()}>{ wish.song_name.clone() }</td>
                                                    <td class={classes!("py-2", "pr-3")}>{ wish.artist_hint.clone().unwrap_or_default() }</td>
                                                    <td class={classes!("py-2", "pr-3")}>{ wish.nickname.clone() }</td>
                                                    <td class={classes!("py-2", "pr-3")}><span class={status_badge_class(&wish.status)}>{ wish.status.clone() }</span></td>
                                                    <td class={classes!("py-2", "pr-3")}>{ wish.ip_region.clone() }</td>
                                                    <td class={classes!("py-2", "pr-3", "whitespace-nowrap")}>{ format_ms(wish.created_at) }</td>
                                                    <td class={classes!("py-2", "pr-3")}>
                                                        <div class={classes!("flex", "gap-1", "flex-wrap")}>
                                                            if status == "pending" {
                                                                <button class={classes!("btn-fluent-primary", "!px-2", "!py-0.5", "!text-xs")} disabled={inflight} onclick={on_approve}>{ "Approve & Run" }</button>
                                                                <button class={classes!("btn-fluent-secondary", "!px-2", "!py-0.5", "!text-xs")} disabled={inflight} onclick={on_reject}>{ "Reject" }</button>
                                                            }
                                                            if status == "failed" {
                                                                <button class={classes!("btn-fluent-primary", "!px-2", "!py-0.5", "!text-xs")} disabled={inflight} onclick={on_retry}>{ "Retry" }</button>
                                                            }
                                                            if status == "done" || status == "running" || status == "failed" {
                                                                <Link<Route> to={Route::AdminMusicWishRuns { wish_id: wid.clone() }} classes={classes!("btn-fluent-secondary", "!px-2", "!py-0.5", "!text-xs")}>
                                                                    { "AI Output" }
                                                                </Link<Route>>
                                                            }
                                                            <button class={classes!("btn-fluent-secondary", "!px-2", "!py-0.5", "!text-xs", "text-red-600", "dark:text-red-400")} disabled={inflight} onclick={on_delete}>{ "Delete" }</button>
                                                        </div>
                                                    </td>
                                                </tr>
                                            }
                                        }) }
                                    </tbody>
                                </table>
                            </div>
                        }
                    </>
                    }
                    <div class={classes!("mt-4")}>
                        <Pagination current_page={*music_wish_page} total_pages={music_wish_total_pages} on_page_change={on_music_wish_page_change} />
                    </div>
                    </div>
                }
            </section>

            <section class={classes!(
                "bg-[var(--surface)]",
                "border",
                "border-[var(--border)]",
                "rounded-[var(--radius)]",
                "shadow-[var(--shadow)]",
                "p-5"
            )}>
                <h2 class={classes!("m-0", "mb-3", "text-lg", "font-semibold")}>{ "Cleanup" }</h2>
                <div class={classes!("flex", "items-center", "gap-2", "flex-wrap")}>
                    <input
                        type="number"
                        value={(*cleanup_days).clone()}
                        oninput={on_cleanup_days_change}
                        class={classes!("rounded-lg", "border", "border-[var(--border)]", "px-3", "py-2", "w-[180px]")}
                    />
                    <button class={classes!("btn-fluent-secondary")} onclick={on_cleanup}>
                        { "Cleanup Failed Tasks" }
                    </button>
                </div>
            </section>
        </main>
    }
}
