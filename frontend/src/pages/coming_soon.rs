use yew::prelude::*;
use yew_router::prelude::*;

use crate::{
    i18n::current::{coming_soon_page as t, common as common_text},
    router::Route,
};

#[derive(Properties, Clone, PartialEq)]
pub struct Props {
    pub feature: AttrValue,
}

#[function_component(ComingSoonPage)]
pub fn coming_soon_page(props: &Props) -> Html {
    let feature = &props.feature;

    let terminal_title = t::TERMINAL_TITLE_TEMPLATE.replace("{}", feature);
    let cmd_init = t::CMD_INIT_TEMPLATE.replace("{}", feature);

    let description = match feature.as_str() {
        "video" => t::DESC_VIDEO,
        "audio" => t::DESC_AUDIO,
        _ => t::DESC_DEFAULT,
    };

    html! {
        <main class={classes!(
            "container", "mx-auto", "px-4", "py-12",
            "flex", "justify-center", "items-center", "min-h-[60vh]"
        )}>
            <div class={classes!("max-w-2xl", "w-full")}>
                <div class="terminal-hero">
                    <div class="terminal-header">
                        <span class="terminal-dot terminal-dot-red"></span>
                        <span class="terminal-dot terminal-dot-yellow"></span>
                        <span class="terminal-dot terminal-dot-green"></span>
                        <span class="terminal-title">{ terminal_title }</span>
                    </div>

                    <div class="terminal-line">
                        <span class="terminal-prompt">
                            { common_text::TERMINAL_PROMPT_CMD }
                        </span>
                        <span class="terminal-content">{ cmd_init }</span>
                    </div>

                    <div class="terminal-line" style="margin-top: 1rem;">
                        <span class="terminal-prompt"
                            style="color: var(--warning, #f59e0b);">
                            { t::STATUS_LABEL }
                        </span>
                        <span class="terminal-content"
                            style="color: var(--warning, #f59e0b);">
                            { "🚧 " }{ t::STATUS_COMING_SOON }
                        </span>
                    </div>

                    <div class="terminal-line">
                        <span class="terminal-prompt">
                            { common_text::TERMINAL_PROMPT_OUTPUT }
                        </span>
                        <span class="terminal-content">{ description }</span>
                    </div>

                    <div class="terminal-line" style="margin-top: 1.5rem;">
                        <span class="terminal-prompt">
                            { common_text::TERMINAL_PROMPT_CMD }
                        </span>
                        <span class="terminal-content">
                            { t::CMD_AVAILABLE_ROUTES }
                        </span>
                    </div>

                    <div class={classes!(
                        "flex", "flex-wrap", "gap-3", "mt-4", "ml-8"
                    )}>
                        <Link<Route>
                            to={Route::Home}
                            classes={classes!(
                                "btn-fluent-primary",
                                "!px-6", "!py-2.5", "!text-sm"
                            )}
                        >
                            <i class="fas fa-home mr-2"></i>
                            { t::BTN_HOME }
                        </Link<Route>>
                    </div>

                    <div class="terminal-line" style="margin-top: 1.5rem;">
                        <span class="terminal-prompt">
                            { common_text::TERMINAL_PROMPT_CMD }
                        </span>
                        <span class="terminal-cursor"></span>
                    </div>
                </div>
            </div>
        </main>
    }
}
