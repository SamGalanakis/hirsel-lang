//! Portable lexical path confinement, shared by live and simulated engines.
/// Normalize `path` below an already pinned logical `root`. Parent components
/// may cancel ordinary components but may never cross the root. Symlink target
/// traversal is a separate, physical check performed after this normalization.
pub fn confined_relative(root: &str, path: &str) -> Option<String> {
    if path.starts_with('/') || path.contains('\0') || path.contains('\\') {
        return None;
    }
    let mut parts: Vec<_> = root.split('/').filter(|p| !p.is_empty()).collect();
    let base = parts.len();
    for p in path.split('/') {
        match p {
            "" | "." => {}
            ".." => {
                if parts.len() == base {
                    return None;
                }
                parts.pop();
            }
            _ => parts.push(p),
        }
    }
    Some(parts.join("/"))
}
