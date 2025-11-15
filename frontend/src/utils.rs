use pulldown_cmark::{html, Event, Options, Parser, Tag, CowStr};

const API_BASE: &str = "http://localhost:3000";

/// Convert image path to API endpoint if it's a relative path
pub fn image_url(path: &str) -> String {
    if path.starts_with("images/") {
        // Extract filename after "images/"
        let filename = path.strip_prefix("images/").unwrap_or(path);
        format!("{}/api/images/{}", API_BASE, filename)
    } else {
        path.to_string()
    }
}

/// Convert Markdown content into HTML with common extensions enabled.
/// Also transforms relative image paths to API endpoints.
pub fn markdown_to_html(content: &str) -> String {
    if content.trim().is_empty() {
        return String::new();
    }

    let mut options = Options::empty();
    options.insert(Options::ENABLE_TABLES);
    options.insert(Options::ENABLE_FOOTNOTES);
    options.insert(Options::ENABLE_STRIKETHROUGH);
    options.insert(Options::ENABLE_TASKLISTS);
    options.insert(Options::ENABLE_SMART_PUNCTUATION);

    let parser = Parser::new_ext(content, options);

    // Transform image paths
    let transformed_parser = parser.map(|event| match event {
        Event::Start(Tag::Image { link_type, dest_url, title, id }) => {
            // Check if image path is relative (starts with "images/")
            let new_url = if dest_url.starts_with("images/") {
                // Extract filename after "images/"
                let filename = dest_url.strip_prefix("images/").unwrap_or(&dest_url);
                // Convert to API endpoint
                CowStr::from(format!("{}/api/images/{}", API_BASE, filename))
            } else {
                dest_url
            };
            Event::Start(Tag::Image {
                link_type,
                dest_url: new_url,
                title,
                id
            })
        }
        _ => event,
    });

    let mut html_output = String::new();
    html::push_html(&mut html_output, transformed_parser);
    html_output
}
