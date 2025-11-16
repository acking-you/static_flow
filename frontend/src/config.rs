/// Configuration for the frontend application

/// Base URL for static assets
/// - For local development: "/"
/// - For GitHub Pages: "/static_flow/"
#[cfg(not(feature = "mock"))]
pub const BASE_URL: &str = "/";

#[cfg(feature = "mock")]
pub const BASE_URL: &str = "/static_flow/";

/// Helper function to construct asset paths
pub fn asset_path(path: &str) -> String {
    // Remove leading slash if present
    let path = path.strip_prefix('/').unwrap_or(path);
    format!("{}{}", BASE_URL, path)
}
