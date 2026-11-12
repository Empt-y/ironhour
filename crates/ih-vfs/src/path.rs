//! Game path normalisation.
//!
//! Ported from `GameEngine/Source/Common/System/ArchiveFileSystem.cpp`
//! (`loadIntoDirectoryTree`, `getArchiveFilenameForFile`) and
//! `GameEngine/Source/Common/System/AsciiString.cpp` (`nextToken`).
//!
//! Game paths are case-insensitive, accept `\` and `/` as separators, and ignore
//! empty components (`nextToken` skips runs of separators). Keys produced here are
//! ASCII-lowercased and joined with `\`.

/// Splits a game path into its non-empty components.
pub fn components(path: &str) -> impl Iterator<Item = &str> {
    path.split(['\\', '/']).filter(|c| !c.is_empty())
}

/// ASCII lowercase, as the C runtime's `tolower` does in the "C" locale.
pub fn lower(s: &str) -> String {
    s.to_ascii_lowercase()
}

/// Plain normalised key: lowercase components joined by `\`.
///
/// Used for loose files and for directory listings.
pub fn key(path: &str) -> String {
    let mut out = String::with_capacity(path.len());
    for (i, c) in components(path).enumerate() {
        if i > 0 {
            out.push('\\');
        }
        out.push_str(&lower(c));
    }
    out
}

/// The key under which the archive file system stores and looks up `path`.
///
/// The original walks the path one component at a time and treats a component as a
/// directory while it has no `.` *or* any later part of the path still contains a
/// `.`. The first component that fails that test is the file name and anything after
/// it is ignored. A path whose last component has no `.` therefore never names a
/// file, so extension-less files inside archives cannot be opened. That is mirrored
/// here by returning `None`.
pub fn archive_key(path: &str) -> Option<String> {
    let parts: Vec<String> = components(path).map(lower).collect();
    let mut out = String::new();
    for (i, token) in parts.iter().enumerate() {
        let rest_has_dot = parts[i + 1..].iter().any(|p| p.contains('.'));
        let is_directory = !token.contains('.') || rest_has_dot;
        if is_directory {
            out.push_str(token);
            out.push('\\');
        } else {
            out.push_str(token);
            return Some(out);
        }
    }
    None
}

/// Case-insensitive ordering used by the original's `FilenameList`
/// (`std::set` with `rts::less_than_nocase`, i.e. `_stricmp`, which compares
/// lowercased bytes).
pub fn cmp_nocase(a: &str, b: &str) -> std::cmp::Ordering {
    let a = a.bytes().map(|c| c.to_ascii_lowercase());
    let b = b.bytes().map(|c| c.to_ascii_lowercase());
    a.cmp(b)
}

/// Wildcard match used for archive directory listings.
///
/// Ported from `SearchStringMatches` in
/// `GameEngine/Source/Common/System/ArchiveFile.cpp`: `*` matches any run, `?` one
/// character, and the comparison is case-sensitive (archive names are already
/// lowercase, so lowercase masks behave as expected).
pub fn search_string_matches(s: &str, pattern: &str) -> bool {
    fn rec(s: &[u8], p: &[u8]) -> bool {
        if s.is_empty() {
            return p.is_empty();
        }
        if p.is_empty() {
            return false;
        }
        let (mut i, mut j) = (0usize, 0usize);
        loop {
            let c1 = s.get(i).copied().unwrap_or(0);
            let c2 = p.get(j).copied().unwrap_or(0);
            if !(c1 == c2 || c2 == b'?' || c2 == b'*') {
                return false;
            }
            if c1 == c2 || c2 == b'?' {
                i += 1;
                j += 1;
            } else {
                // c2 == '*'
                j += 1;
                if j >= p.len() {
                    return true;
                }
                while i < s.len() {
                    if rec(&s[i..], &p[j..]) {
                        return true;
                    }
                    i += 1;
                }
            }
            let at_end_s = i >= s.len();
            let at_end_p = j >= p.len();
            if at_end_s {
                return at_end_p;
            }
            if at_end_p {
                return false;
            }
        }
    }
    rec(s.as_bytes(), pattern.as_bytes())
}

/// Case-insensitive wildcard match, standing in for Win32 `FindFirstFile` on loose
/// files.
pub fn wildcard_matches_nocase(s: &str, pattern: &str) -> bool {
    search_string_matches(&lower(s), &lower(pattern))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_normalise_case_and_separators() {
        assert_eq!(key("Data\\INI//Object\\\\AmericaTank.ini"), "data\\ini\\object\\americatank.ini");
        assert_eq!(key("/Data/INI/"), "data\\ini");
    }

    #[test]
    fn archive_key_follows_the_dot_rule() {
        assert_eq!(archive_key("Data\\INI\\Weapon.ini").as_deref(), Some("data\\ini\\weapon.ini"));
        // Directories with dots still count as directories when a later part has one.
        assert_eq!(archive_key("art\\v1.2\\tank.w3d").as_deref(), Some("art\\v1.2\\tank.w3d"));
        // Extension-less files are unreachable.
        assert_eq!(archive_key("data\\readme"), None);
        // Anything after the file name is ignored.
        assert_eq!(archive_key("data\\x.ini\\extra").as_deref(), Some("data\\x.ini"));
    }

    #[test]
    fn nocase_order_puts_bang_first() {
        let mut v = vec!["W3DZH.big", "!Mod.big", "!!Patch.big", "launcher\\MapsZH.big", "MapsZH.big", "_x.big"];
        v.sort_by(|a, b| cmp_nocase(a, b));
        // '_' (0x5F) sorts before every lowercase letter because _stricmp lowercases.
        assert_eq!(v, ["!!Patch.big", "!Mod.big", "_x.big", "launcher\\MapsZH.big", "MapsZH.big", "W3DZH.big"]);
    }

    #[test]
    fn wildcard() {
        assert!(search_string_matches("weapon.ini", "*.ini"));
        assert!(search_string_matches("weapon.ini", "*"));
        assert!(search_string_matches("a.ini", "?.ini"));
        assert!(!search_string_matches("weapon.inx", "*.ini"));
        assert!(!search_string_matches("Weapon.INI", "*.ini"));
        assert!(wildcard_matches_nocase("Weapon.INI", "*.ini"));
        assert!(!search_string_matches("", "*"));
        assert!(search_string_matches("abcabc", "*bc"));
    }
}
