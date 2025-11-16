/// Configuration for the frontend application

/// Base URL for static assets - always use relative paths
pub const BASE_URL: &str = "/";

/// Base path for routes - depends on mock feature
#[cfg(not(feature = "mock"))]
pub const ROUTE_BASE: &str = "";

#[cfg(feature = "mock")]
pub const ROUTE_BASE: &str = "/static_flow";

/// Helper function to construct asset paths
pub fn asset_path(path: &str) -> String {
    // Remove leading slash if present to make it relative
    let path = path.strip_prefix('/').unwrap_or(path);
    format!("{}{}", BASE_URL, path)
}

/// Helper function to construct route paths
pub fn route_path(path: &str) -> String {
    format!("{}{}", ROUTE_BASE, path)
}
