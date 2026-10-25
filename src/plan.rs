//! Pure planning logic (design section 1, 4.6, 4.8). No external commands here.

use std::collections::{BTreeMap, BTreeSet};

pub type Set = BTreeSet<String>;

/// Never removed by novarch, only dropped from `managed` (design 4.6).
const PROTECTED: &[&str] = &[
    "base",
    "pacman",
    "sudo",
    "systemd",
    "grub",
    "refind",
    "limine",
    "efibootmgr",
    "paru",
    "paru-bin",
    "novarch",
    "novarch-bin",
];

pub fn is_protected(name: &str) -> bool {
    PROTECTED.contains(&name) || name == "linux" || name.starts_with("linux-")
}

#[derive(Debug, Default, PartialEq)]
pub struct Plan {
    /// Declared names pacman -T reports as not satisfied.
    pub install: Set,
    /// Real installed names of declared packages that novarch does not manage yet.
    pub adopt: Set,
    /// Managed, no longer declared, installed, not protected.
    pub remove: Set,
    /// Managed, no longer declared, and protected or no longer installed: forget only.
    pub unmanage: Set,
}

/// Declared names plus the real names they resolve to (`sh` -> `bash`).
pub fn wanted(declared: &Set, real: &BTreeMap<String, String>) -> Set {
    declared.iter().chain(real.values()).cloned().collect()
}

/// `real` maps each satisfied declared name to the installed package that satisfies it.
pub fn plan(
    declared: &Set,
    unsatisfied: &Set,
    real: &BTreeMap<String, String>,
    managed: &Set,
    installed: &Set,
) -> Plan {
    let wanted = wanted(declared, real);
    let mut p = Plan {
        install: declared.intersection(unsatisfied).cloned().collect(),
        adopt: real
            .values()
            .filter(|r| !managed.contains(*r))
            .cloned()
            .collect(),
        ..Plan::default()
    };
    for m in managed.difference(&wanted) {
        if installed.contains(m) && !is_protected(m) {
            p.remove.insert(m.clone());
        } else {
            p.unmanage.insert(m.clone());
        }
    }
    p
}

/// Split removals: (nothing hard-requires it: `-Rs`, still required: `-D --asdeps`).
/// `unrequired` is `pacman -Qqtt`.
pub fn split_removal(remove: &Set, unrequired: &Set) -> (Set, Set) {
    remove.iter().cloned().partition(|p| unrequired.contains(p))
}

/// System orphans (`pacman -Qdtq`) that are safe to offer: not wanted, not protected.
pub fn safe_orphans(orphans: &Set, wanted: &Set) -> Set {
    orphans
        .iter()
        .filter(|o| !wanted.contains(*o) && !is_protected(o))
        .cloned()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn set(items: &[&str]) -> Set {
        items.iter().map(|s| s.to_string()).collect()
    }

    fn map(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(a, b)| (a.to_string(), b.to_string()))
            .collect()
    }

    #[test]
    fn install_adopt_remove() {
        let p = plan(
            &set(&["git", "firefox", "vim"]),
            &set(&["firefox"]),
            &map(&[("git", "git"), ("vim", "vim")]),
            &set(&["vim", "tree"]),
            &set(&["git", "vim", "tree"]),
        );
        assert_eq!(p.install, set(&["firefox"]));
        assert_eq!(p.adopt, set(&["git"]));
        assert_eq!(p.remove, set(&["tree"]));
        assert!(p.unmanage.is_empty());
    }

    #[test]
    fn provides_keep_the_real_package() {
        // `sh` is declared, `bash` is what is installed and managed: never remove bash.
        let p = plan(
            &set(&["sh"]),
            &Set::new(),
            &map(&[("sh", "bash")]),
            &set(&["bash"]),
            &set(&["bash"]),
        );
        assert_eq!(p, Plan::default());
    }

    #[test]
    fn empty_state_removes_nothing() {
        let p = plan(
            &set(&["git"]),
            &Set::new(),
            &map(&[("git", "git")]),
            &Set::new(),
            &set(&["git", "tree", "htop"]),
        );
        assert!(p.remove.is_empty());
        assert_eq!(p.adopt, set(&["git"]));
    }

    #[test]
    fn protected_and_gone_are_only_unmanaged() {
        let p = plan(
            &set(&["git"]),
            &Set::new(),
            &map(&[("git", "git")]),
            &set(&["git", "linux", "linux-firmware", "sudo", "gone"]),
            &set(&["git", "linux", "linux-firmware", "sudo"]),
        );
        assert!(p.remove.is_empty());
        assert_eq!(
            p.unmanage,
            set(&["gone", "linux", "linux-firmware", "sudo"])
        );
    }

    #[test]
    fn declared_but_uninstalled_by_hand_is_reinstalled_not_removed() {
        let p = plan(
            &set(&["tree"]),
            &set(&["tree"]),
            &BTreeMap::new(),
            &set(&["tree"]),
            &Set::new(),
        );
        assert_eq!(p.install, set(&["tree"]));
        assert!(p.remove.is_empty() && p.unmanage.is_empty());
    }

    #[test]
    fn removal_split_and_orphans() {
        let (rs, asdeps) = split_removal(&set(&["a", "b"]), &set(&["a", "x"]));
        assert_eq!(rs, set(&["a"]));
        assert_eq!(asdeps, set(&["b"]));
        let o = safe_orphans(&set(&["lib1", "git", "linux-headers"]), &set(&["git"]));
        assert_eq!(o, set(&["lib1"]));
    }

    #[test]
    fn protection_rule() {
        for p in [
            "linux",
            "linux-lts",
            "linux-zen-headers",
            "base",
            "paru-bin",
        ] {
            assert!(is_protected(p), "{p}");
        }
        for p in ["linuxfoo", "base-devel", "git"] {
            assert!(!is_protected(p), "{p}");
        }
    }
}
