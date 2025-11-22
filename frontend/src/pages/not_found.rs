use yew::prelude::*;
use yew_router::prelude::*;
use crate::router::Route;

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
                        <span class="terminal-title">{ "error.sh" }</span>
                    </div>

                    // Command showing 404 error
                    <div class="terminal-line">
                        <span class="terminal-prompt">{ "$ " }</span>
                        <span class="terminal-content">{ "curl http://localhost:8080$(location.pathname)" }</span>
                    </div>

                    // Error output
                    <div class="terminal-line" style="margin-top: 1rem;">
                        <span class="terminal-prompt" style="color: var(--error, #ef4444);">{ "ERROR: " }</span>
                        <span class="terminal-content" style="color: var(--error, #ef4444);">{ "404 Not Found" }</span>
                    </div>

                    <div class="terminal-line">
                        <span class="terminal-prompt">{ "> " }</span>
                        <span class="terminal-content">{ "The requested resource could not be found on this server." }</span>
                    </div>

                    // Helpful message
                    <div class="terminal-line" style="margin-top: 1.5rem;">
                        <span class="terminal-prompt">{ "$ " }</span>
                        <span class="terminal-content">{ "cat /var/log/suggestions.log" }</span>
                    </div>

                    <div class="terminal-line">
                        <span class="terminal-prompt">{ "> " }</span>
                        <span class="terminal-content">{ "抱歉，你要找的页面走丢了... 可能是被外星人劫持了 👽" }</span>
                    </div>

                    <div class="terminal-line">
                        <span class="terminal-prompt">{ "> " }</span>
                        <span class="terminal-content">{ "建议：检查 URL 拼写，或者返回首页重新探索。" }</span>
                    </div>

                    // Navigation options
                    <div class="terminal-line" style="margin-top: 1.5rem;">
                        <span class="terminal-prompt">{ "$ " }</span>
                        <span class="terminal-content">{ "ls -l ./available_routes/" }</span>
                    </div>

                    <div class={classes!("flex", "flex-wrap", "gap-3", "mt-4", "ml-8")}>
                        <Link<Route>
                            to={Route::Home}
                            classes={classes!("btn-fluent-primary", "!px-6", "!py-2.5", "!text-sm")}
                        >
                            <i class="fas fa-home mr-2"></i>
                            { "返回首页" }
                        </Link<Route>>
                        <Link<Route>
                            to={Route::LatestArticles}
                            classes={classes!("btn-fluent-secondary", "!px-6", "!py-2.5", "!text-sm")}
                        >
                            <i class="fas fa-newspaper mr-2"></i>
                            { "最新文章" }
                        </Link<Route>>
                        <Link<Route>
                            to={Route::Posts}
                            classes={classes!("btn-fluent-secondary", "!px-6", "!py-2.5", "!text-sm")}
                        >
                            <i class="fas fa-archive mr-2"></i>
                            { "文章归档" }
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
                        <span class="terminal-prompt">{ "$ " }</span>
                        <span class="terminal-cursor"></span>
                    </div>
                </div>
            </div>
        </main>
    }
}
