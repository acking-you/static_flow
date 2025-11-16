/// Configuration for the frontend application

/// Base path for routes - depends on mock feature
#[cfg(not(feature = "mock"))]
pub const ROUTE_BASE: &str = "";

#[cfg(feature = "mock")]
pub const ROUTE_BASE: &str = "/static_flow";

/// Helper function to construct asset paths
/// In mock feature, assets are served under /static_flow/ prefix
pub fn asset_path(path: &str) -> String {
    // Remove leading slash if present to make it relative
    let path = path.strip_prefix('/').unwrap_or(path);
    format!("{}/{}", ROUTE_BASE, path)
}

/// Helper function to construct route paths
pub fn route_path(path: &str) -> String {
    format!("{}{}", ROUTE_BASE, path)
}
