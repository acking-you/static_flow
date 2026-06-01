use yew::prelude::*;
use yew_router::prelude::*;

use crate::{
    i18n::current::{common as common_text, not_found_page as t},
    router::Route,
};

#[function_component(NotFoundPage)]
pub fn not_found_page() -> Html {
    html! {
            <main class={classes!("container", "mx-auto", "px-4", "py-12", "flex", "justify-center", "items-center", "min-h-[60vh]")}>
                <div class={classes!("max-w-2xl", "w-full")}>
                    // Terminal-style 404 Error
                    <div class="terminal-hero">
                        // Terminal Header with macOS-style dots
                        <div class="terminal-header">
                            <span class="terminal-dot terminal-dot-red"></span>
                            <span class="terminal-dot terminal-dot-yellow"></span>
                            <span class="terminal-dot terminal-dot-green"></span>
                            <span class="terminal-title">{ t::TERMINAL_TITLE }</span>
                        </div>

                        // Command showing 404 error
                        <div class="terminal-line">
                            <span class="terminal-prompt">{ common_text::TERMINAL_PROMPT_CMD }</span>
                            <span class="terminal-content">{ t::CMD_LOOKUP }</span>
                        </div>

                        // Error output
                        <div class="terminal-line" style="margin-top: 1rem;">
                            <span class="terminal-prompt" style="color: var(--error, #ef4444);">{ t::ERROR_PREFIX }</span>
                            <span class="terminal-content" style="color: var(--error, #ef4444);">{ t::ERROR_CODE }</span>
                        </div>

                        <div class="terminal-line">
                            <span class="terminal-prompt">{ common_text::TERMINAL_PROMPT_OUTPUT }</span>
                            <span class="terminal-content">{ t::ERROR_DETAIL }</span>
                        </div>

                        // Helpful message
                        <div class="terminal-line" style="margin-top: 1.5rem;">
                            <span class="terminal-prompt">{ common_text::TERMINAL_PROMPT_CMD }</span>
                            <span class="terminal-content">{ t::CMD_SUGGESTIONS }</span>
                        </div>

                        <div class="terminal-line">
                            <span class="terminal-prompt">{ common_text::TERMINAL_PROMPT_OUTPUT }</span>
                            <span class="terminal-content">{ t::SUGGESTION_1 }</span>
                        </div>

                        <div class="terminal-line">
                            <span class="terminal-prompt">{ common_text::TERMINAL_PROMPT_OUTPUT }</span>
                            <span class="terminal-content">{ t::SUGGESTION_2 }</span>
                        </div>

                        // Navigation options
                        <div class="terminal-line" style="margin-top: 1.5rem;">
                            <span class="terminal-prompt">{ common_text::TERMINAL_PROMPT_CMD }</span>
                            <span class="terminal-content">{ t::CMD_AVAILABLE_ROUTES }</span>
                        </div>

                        <div class={classes!("flex", "flex-wrap", "gap-3", "mt-4", "ml-8")}>
                            <Link<Route>
                                to={Route::Home}
                                classes={classes!("btn-fluent-primary", "!px-6", "!py-2.5", "!text-sm")}
                            >
                                <i class="fas fa-home mr-2"></i>
                                { t::BTN_HOME }
                            </Link<Route>>
                            <Link<Route>
                                to={Route::LatestArticles}
                                classes={classes!("btn-fluent-secondary", "!px-6", "!py-2.5", "!text-sm")}
                            >
                                <i class="fas fa-newspaper mr-2"></i>
                                { t::BTN_LATEST }
                            </Link<Route>>
                            <Link<Route>
                                to={Route::Posts}
                                classes={classes!("btn-fluent-secondary", "!px-6", "!py-2.5", "!text-sm")}
                            >
                                <i class="fas fa-archive mr-2"></i>
                                { t::BTN_ARCHIVE }
                            </Link<Route>>
                        </div>

                        // ASCII Art (optional fun element)
                        <div class="terminal-line" style="margin-top: 1.5rem; font-family: monospace; line-height: 1.2;">
                            <pre style="color: var(--text-muted, #6b7280); font-size: 0.75rem;">
    {r#"  _  _    ___   _  _
 | || |  / _ \ | || |
 | || |_| | | || || |_
 |__   _| |_| ||__   _|
    |_|  \___/    |_|
"#}
                            </pre>
                        </div>

                        // Blinking cursor
                        <div class="terminal-line" style="margin-top: 1rem;">
                            <span class="terminal-prompt">{ common_text::TERMINAL_PROMPT_CMD }</span>
                            <span class="terminal-cursor"></span>
                        </div>
                    </div>
                </div>
            </main>
        }
}
