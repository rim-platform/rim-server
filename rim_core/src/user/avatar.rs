use crate::user::model::UserId;

/// The URL prefix under which stored files are served.
const FILES_URL_PREFIX: &str = "/api/files/";

/// An image type an avatar may be stored as.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AvatarFormat {
    Png,
    Jpeg,
    Gif,
    Webp,
}

impl AvatarFormat {
    /// Every accepted format.
    pub const ALL: [AvatarFormat; 4] = [Self::Png, Self::Jpeg, Self::Gif, Self::Webp];

    /// The format's MIME type.
    pub fn mime(self) -> &'static str {
        match self {
            Self::Png => "image/png",
            Self::Jpeg => "image/jpeg",
            Self::Gif => "image/gif",
            Self::Webp => "image/webp",
        }
    }

    /// The extension of the stored file.
    pub fn extension(self) -> &'static str {
        match self {
            Self::Png => "png",
            Self::Jpeg => "jpeg",
            Self::Gif => "gif",
            Self::Webp => "webp",
        }
    }

    /// The format a declared content type names (matched case-insensitively by prefix, so
    /// parameters after the type are allowed); `None` for any other type.
    pub fn from_content_type(content_type: &str) -> Option<Self> {
        let content_type = content_type.to_ascii_lowercase();
        Self::ALL
            .into_iter()
            .find(|format| content_type.starts_with(format.mime()))
    }

    /// Whether `data` starts with the format's file signature: PNG's eight-byte signature,
    /// JPEG's `FF D8 FF`, `GIF87a` or `GIF89a`, or a RIFF container of type `WEBP`. The image
    /// itself is not decoded.
    pub fn matches_signature(self, data: &[u8]) -> bool {
        match self {
            Self::Png => data.starts_with(b"\x89PNG\r\n\x1a\n"),
            Self::Jpeg => data.starts_with(&[0xFF, 0xD8, 0xFF]),
            Self::Gif => data.starts_with(b"GIF87a") || data.starts_with(b"GIF89a"),
            Self::Webp => data.starts_with(b"RIFF") && data.get(8..12) == Some(b"WEBP"),
        }
    }
}

/// The storage directory that holds `user_id`'s avatar files, without a trailing slash.
pub fn avatar_dir(user_id: UserId) -> String {
    format!("avatars/{user_id}")
}

/// The storage key of `user_id`'s avatar stored as `format`.
pub fn avatar_key(user_id: UserId, format: AvatarFormat) -> String {
    format!("{}/avatar.{}", avatar_dir(user_id), format.extension())
}

/// The content-addressed avatar key, using the first eight bytes of its SHA-256 digest.
pub fn hashed_avatar_key(user_id: UserId, format: AvatarFormat, digest: &[u8; 32]) -> String {
    let hash: String = digest[..8]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    format!(
        "{}/avatar-{hash}.{}",
        avatar_dir(user_id),
        format.extension()
    )
}

fn hashed_filename(name: &str) -> bool {
    let Some((stem, extension)) = name.rsplit_once('.') else {
        return false;
    };
    let Some(hash) = stem.strip_prefix("avatar-") else {
        return false;
    };
    hash.len() == 16
        && hash
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        && AvatarFormat::ALL
            .iter()
            .any(|format| format.extension() == extension)
}

/// Whether `key` names a content-addressed avatar with no nested path segments.
pub fn is_hashed_avatar_key(key: &str) -> bool {
    let Some(rest) = key.strip_prefix("avatars/") else {
        return false;
    };
    let Some((user, filename)) = rest.split_once('/') else {
        return false;
    };
    !user.is_empty() && hashed_filename(filename)
}

/// The URL that serves the file stored under `key`.
pub fn file_url(key: &str) -> String {
    format!("{FILES_URL_PREFIX}{key}")
}

/// The storage key `avatar_url` names when it is the URL of one of `user_id`'s own avatar files
/// (`avatar.<ext>` or `avatar-<16 lowercase hex>.<ext>`); `None` for any other URL.
pub fn own_avatar_key(user_id: UserId, avatar_url: &str) -> Option<String> {
    let key = avatar_url.strip_prefix(FILES_URL_PREFIX)?;
    let filename = key.strip_prefix(&format!("{}/", avatar_dir(user_id)))?;
    if hashed_filename(filename)
        || AvatarFormat::ALL
            .into_iter()
            .any(|format| filename == format!("avatar.{}", format.extension()))
    {
        Some(key.to_string())
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_content_type_names_its_format_and_others_name_none() {
        assert_eq!(
            AvatarFormat::from_content_type("image/png"),
            Some(AvatarFormat::Png)
        );
        assert_eq!(
            AvatarFormat::from_content_type("IMAGE/JPEG; q=1"),
            Some(AvatarFormat::Jpeg)
        );
        assert_eq!(
            AvatarFormat::from_content_type("image/webp"),
            Some(AvatarFormat::Webp)
        );
        assert_eq!(AvatarFormat::from_content_type("image/svg+xml"), None);
        assert_eq!(AvatarFormat::from_content_type("text/plain"), None);
    }

    #[test]
    fn each_format_accepts_only_its_own_signature() {
        let png = b"\x89PNG\r\n\x1a\nrest".as_slice();
        let jpeg = b"\xFF\xD8\xFF\xE0rest".as_slice();
        let gif87 = b"GIF87arest".as_slice();
        let gif89 = b"GIF89arest".as_slice();
        let webp = b"RIFF\x10\0\0\0WEBPVP8 ".as_slice();
        let cases = [
            (AvatarFormat::Png, vec![png]),
            (AvatarFormat::Jpeg, vec![jpeg]),
            (AvatarFormat::Gif, vec![gif87, gif89]),
            (AvatarFormat::Webp, vec![webp]),
        ];
        let all = [png, jpeg, gif87, gif89, webp];
        for (format, accepted) in cases {
            for data in all {
                assert_eq!(
                    format.matches_signature(data),
                    accepted.contains(&data),
                    "{format:?} {data:?}"
                );
            }
        }
        for format in AvatarFormat::ALL {
            assert!(!format.matches_signature(b""), "{format:?}");
        }
        assert!(!AvatarFormat::Png.matches_signature(b"\x89PNG avatar"));
        assert!(!AvatarFormat::Gif.matches_signature(b"GIF88a"));
        assert!(!AvatarFormat::Webp.matches_signature(b"RIFF\x10\0\0\0WAVE"));
        assert!(!AvatarFormat::Webp.matches_signature(b"RIFF\x10\0\0\0WEB"));
    }

    #[test]
    fn avatar_keys_live_in_the_users_directory() {
        let user_id = UserId::new();
        assert_eq!(avatar_dir(user_id), format!("avatars/{user_id}"));
        assert_eq!(
            avatar_key(user_id, AvatarFormat::Gif),
            format!("avatars/{user_id}/avatar.gif")
        );
        assert_eq!(
            file_url("avatars/u/avatar.png"),
            "/api/files/avatars/u/avatar.png"
        );
    }

    #[test]
    fn content_addressed_keys_validate_only_for_their_owner() {
        let user = UserId::new();
        let key = hashed_avatar_key(user, AvatarFormat::Png, &[0xab; 32]);
        assert_eq!(key, format!("avatars/{user}/avatar-abababababababab.png"));
        assert!(is_hashed_avatar_key(&key));
        assert_eq!(own_avatar_key(user, &file_url(&key)), Some(key.clone()));
        assert!(own_avatar_key(UserId::new(), &file_url(&key)).is_none());
        for key in [
            key.replace("abababababababab", "ABABABABABABABAB"),
            key.replace("abababababababab", "abcd"),
            key.replace(".png", ".svg"),
            format!("{key}/extra"),
        ] {
            assert!(!is_hashed_avatar_key(&key), "{key}");
            assert!(own_avatar_key(user, &file_url(&key)).is_none(), "{key}");
        }
    }

    #[test]
    fn only_the_users_own_avatar_urls_name_a_key() {
        let user_id = UserId::new();
        let other = UserId::new();
        for format in AvatarFormat::ALL {
            let key = avatar_key(user_id, format);
            assert_eq!(own_avatar_key(user_id, &file_url(&key)), Some(key));
        }
        for url in [
            format!("/api/files/avatars/{other}/avatar.png"),
            format!("/api/files/avatars/{user_id}/avatar.svg"),
            format!("/api/files/avatars/{user_id}/avatar.png/x"),
            format!("/api/files/avatars/{user_id}/../{other}/avatar.png"),
            format!("avatars/{user_id}/avatar.png"),
            format!("https://example.com/api/files/avatars/{user_id}/avatar.png"),
            "/api/files/".to_string(),
            String::new(),
        ] {
            assert_eq!(own_avatar_key(user_id, &url), None, "{url}");
        }
    }
}
