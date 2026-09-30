//! "A new version is available" (launch checklist B6, session 78).
//!
//! The account Worker names the newest app version beside the lease
//! (`latest_version`, outside the signed payload). Only a plain release
//! number is ever accepted, `x.y.z` with each part 0-9999 and no leading
//! zero, so nothing but a number can reach the file or the screen; anything
//! else is ignored and never breaks the lease.

/// The newest version as the Worker sent it, if it is a plain release number.
#[must_use]
pub fn parse_release(text: &str) -> Option<(u32, u32, u32)> {
    let mut parts = text.split('.');
    let mut next = || -> Option<u32> {
        let part = parts.next()?;
        let plain = !part.is_empty()
            && part.len() <= 4
            && part.bytes().all(|b| b.is_ascii_digit())
            && (part == "0" || !part.starts_with('0'));
        if plain {
            part.parse().ok()
        } else {
            None
        }
    };
    let version = (next()?, next()?, next()?);
    if parts.next().is_some() {
        return None;
    }
    Some(version)
}

/// Whether `latest` is a newer release than `current`. False whenever either
/// is not a plain release number: a bar that cannot be sure stays off.
#[must_use]
pub fn is_newer(latest: &str, current: &str) -> bool {
    match (parse_release(latest), parse_release(current)) {
        (Some(latest), Some(current)) => latest > current,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_plain_release_number_is_read() {
        assert_eq!(parse_release("0.3.0"), Some((0, 3, 0)));
        assert_eq!(parse_release("10.20.9999"), Some((10, 20, 9999)));
        for bad in [
            "",
            "0.3",
            "0.3.0.1",
            "v0.3.0",
            "0.3.0-beta",
            " 0.3.0",
            "01.2.3",
            "0.3.10000",
            "0..3",
            "a.b.c",
            "<b>1.0.0</b>",
        ] {
            assert_eq!(parse_release(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn newer_compares_each_part_as_a_number() {
        assert!(is_newer("0.3.1", "0.3.0"));
        assert!(is_newer("0.10.0", "0.9.9"));
        assert!(is_newer("1.0.0", "0.99.99"));
        assert!(!is_newer("0.3.0", "0.3.0"));
        assert!(!is_newer("0.2.9", "0.3.0"));
        // Unreadable on either side: no bar.
        assert!(!is_newer("junk", "0.3.0"));
        assert!(!is_newer("0.4.0", "0.3.0-dev"));
    }
}
