//! Load, validate and merge the package files (design 2, 4.0 steps 3-5).

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use toml_edit::Document;

use crate::ui;

/// Declared package name -> files (relative to the packages folder) that list it.
pub type Declared = BTreeMap<String, Vec<PathBuf>>;

/// Shared files (top level) plus `hosts/<host>/*.toml`. Every error in every file is
/// reported, and any error stops the command: never run with a partial set.
pub fn load(dir: &Path, host: &str) -> Result<Declared, String> {
    let mut files = toml_files(dir)?;
    let host_dir = dir.join("hosts").join(host);
    if host_dir.is_dir() {
        files.extend(toml_files(&host_dir)?);
    } else if dir.join("hosts").is_dir() {
        ui::warn(&format!(
            "no folder hosts/{host}/ for this host; using shared files only"
        ));
    }

    let mut declared = Declared::new();
    let mut errors = Vec::new();
    for path in &files {
        let rel = path.strip_prefix(dir).unwrap_or(path).to_path_buf();
        let parsed = fs::read_to_string(path)
            .map_err(|e| vec![format!("{}: {e}", rel.display())])
            .and_then(|text| parse(&text, &rel.display().to_string()));
        match parsed {
            Ok(names) => {
                for name in names {
                    let sources = declared.entry(name).or_default();
                    if !sources.contains(&rel) {
                        sources.push(rel.clone());
                    }
                }
            }
            Err(mut errs) => errors.append(&mut errs),
        }
    }
    if !errors.is_empty() {
        return Err(errors.join("\n"));
    }
    if declared.is_empty() {
        return Err(format!(
            "no packages declared in {}; refusing to continue (an empty list would remove every managed package)",
            dir.display()
        ));
    }
    Ok(declared)
}

/// Regular `*.toml` files directly in `dir`, sorted.
fn toml_files(dir: &Path) -> Result<Vec<PathBuf>, String> {
    let entries = fs::read_dir(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let mut files: Vec<PathBuf> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_file() && p.extension().is_some_and(|x| x == "toml"))
        .collect();
    files.sort();
    Ok(files)
}

/// Parse one package file: exactly one key, `packages`, an array of valid names.
/// Errors are `file:line: message`.
fn parse(text: &str, file: &str) -> Result<Vec<String>, Vec<String>> {
    let doc = Document::parse(text).map_err(|e| vec![format!("{file}: {e}")])?;
    let table = doc.as_table();
    let at = |span: Option<std::ops::Range<usize>>| match span {
        Some(r) => format!("{file}:{}", text[..r.start].matches('\n').count() + 1),
        None => file.to_string(),
    };
    let mut errors = Vec::new();
    for (key, _) in table.iter() {
        if key != "packages" {
            let span = table.key(key).and_then(|k| k.span());
            errors.push(format!(
                "{}: unknown key `{key}` (only `packages` is allowed)",
                at(span)
            ));
        }
    }
    let Some(item) = table.get("packages") else {
        errors.push(format!("{file}: missing `packages = [...]`"));
        return Err(errors);
    };
    let Some(array) = item.as_array() else {
        errors.push(format!("{}: `packages` must be a list", at(item.span())));
        return Err(errors);
    };
    let mut names = Vec::new();
    for v in array.iter() {
        match v.as_str() {
            Some(name) if valid_name(name) => names.push(name.to_string()),
            Some(name) => errors.push(format!("{}: invalid package name \"{name}\"", at(v.span()))),
            None => errors.push(format!(
                "{}: package names must be strings, found {}",
                at(v.span()),
                v.to_string().trim()
            )),
        }
    }
    if errors.is_empty() {
        Ok(names)
    } else {
        Err(errors)
    }
}

/// makepkg's rule: alphanumerics and `@._+-`, not starting with `-` or `.`.
/// Also keeps a name like `--overwrite=*` from reaching pacman as an option.
fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && !name.starts_with(['-', '.'])
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "@._+-".contains(c))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_names_and_comments() {
        let text =
            "# dev tools\npackages = [\n  \"git\",\n  \"visual-studio-code-bin\", # AUR\n]\n";
        assert_eq!(
            parse(text, "dev.toml").unwrap(),
            ["git", "visual-studio-code-bin"]
        );
    }

    #[test]
    fn empty_list_is_fine_per_file() {
        assert!(parse("packages = []\n", "a.toml").unwrap().is_empty());
    }

    #[test]
    fn reports_line_of_bad_name_and_unknown_key() {
        let text = "packages = [\n  \"git\",\n  \"--overwrite=*\",\n]\npakages = []\n";
        let errs = parse(text, "a.toml").unwrap_err();
        assert!(
            errs.contains(&"a.toml:5: unknown key `pakages` (only `packages` is allowed)".into())
        );
        assert!(errs.contains(&"a.toml:3: invalid package name \"--overwrite=*\"".into()));
    }

    #[test]
    fn rejects_missing_key_wrong_type_and_bad_toml() {
        assert!(parse("# nothing\n", "a.toml").unwrap_err()[0].contains("missing"));
        assert!(parse("packages = \"git\"\n", "a.toml").unwrap_err()[0].contains("must be a list"));
        assert!(parse("packages = [1]\n", "a.toml").unwrap_err()[0].contains("must be strings"));
        assert!(parse("packages = [\n", "a.toml").is_err());
    }

    #[test]
    fn name_rule() {
        for ok in [
            "git",
            "lib32-glibc",
            "gtk+",
            "python3.14",
            "foo@bar",
            "Some_Pkg",
        ] {
            assert!(valid_name(ok), "{ok}");
        }
        for bad in ["", "-Rdd", ".hidden", "a b", "pkg;rm", "a/b", "ü"] {
            assert!(!valid_name(bad), "{bad}");
        }
    }

    #[test]
    fn load_merges_shared_and_host_and_refuses_empty() {
        let dir = std::env::temp_dir().join(format!("novarch-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("hosts/laptop")).unwrap();
        fs::write(dir.join("base.toml"), "packages = [\"git\", \"vim\"]\n").unwrap();
        fs::write(dir.join("notes.txt"), "ignored").unwrap();
        fs::write(
            dir.join("hosts/laptop/gui.toml"),
            "packages = [\"git\", \"firefox\"]\n",
        )
        .unwrap();

        let d = load(&dir, "laptop").unwrap();
        assert_eq!(d.keys().collect::<Vec<_>>(), ["firefox", "git", "vim"]);
        assert_eq!(d["git"].len(), 2);
        assert_eq!(load(&dir, "server").unwrap().len(), 2);

        fs::write(dir.join("base.toml"), "packages = []\n").unwrap();
        assert!(
            load(&dir, "server")
                .unwrap_err()
                .contains("no packages declared")
        );
        fs::remove_dir_all(&dir).unwrap();
    }
}
