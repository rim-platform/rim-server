/// Sanitize a filename: strip path components, reject traversal.
///
/// Returns `None` if the filename is empty, is a traversal pattern,
/// or contains `..` after stripping path separators.
pub fn sanitize_filename(name: &str) -> Option<String> {
    let name = name.replace('\0', "");

    // Reject any input containing traversal sequences before stripping paths.
    if name.contains("..") {
        return None;
    }

    // Take only the last path component.
    let name = name.rsplit('/').next().unwrap_or(&name);
    let name = name.rsplit('\\').next().unwrap_or(name);
    let name = name.trim();

    if name.is_empty() || name == "." {
        return None;
    }

    Some(name.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_traversal_patterns() {
        assert_eq!(sanitize_filename(".."), None);
        assert_eq!(sanitize_filename("../etc/passwd"), None);
        assert_eq!(sanitize_filename("foo/../bar"), None);
        assert_eq!(sanitize_filename("..\\windows\\system32"), None);
    }

    #[test]
    fn strips_path_components() {
        assert_eq!(sanitize_filename("/etc/passwd"), Some("passwd".to_string()));
        assert_eq!(
            sanitize_filename("C:\\Users\\file.txt"),
            Some("file.txt".to_string())
        );
    }

    #[test]
    fn rejects_empty_and_dot() {
        assert_eq!(sanitize_filename(""), None);
        assert_eq!(sanitize_filename("."), None);
        assert_eq!(sanitize_filename("  "), None);
        assert_eq!(sanitize_filename("\0"), None);
    }

    #[test]
    fn accepts_valid_filenames() {
        assert_eq!(
            sanitize_filename("photo.png"),
            Some("photo.png".to_string())
        );
        assert_eq!(
            sanitize_filename("  hello.txt  "),
            Some("hello.txt".to_string())
        );
    }
}
