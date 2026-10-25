//! Load, validate and merge the package files (design 2, 4.0 steps 3-5).

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use toml_edit::{Array, Document, DocumentMut, Value};

use crate::plan::Set;
use crate::{config, ui};

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

/// Append names to a package file (created if missing), keeping comments and layout.
pub fn append(file: &Path, names: &[String]) -> Result<(), String> {
    let text = if file.exists() {
        let old = fs::read_to_string(file).map_err(|e| format!("{}: {e}", file.display()))?;
        append_text(&old, names).map_err(|e| format!("{}: {e}", file.display()))?
    } else {
        let body: String = names.iter().map(|n| format!("  \"{n}\",\n")).collect();
        format!("# Added by novarch add\npackages = [\n{body}]\n")
    };
    config::write_atomic(file, &text)
}

/// Remove one name from a package file. Returns whether the file listed it.
pub fn remove_name(file: &Path, name: &str) -> Result<bool, String> {
    let old = fs::read_to_string(file).map_err(|e| format!("{}: {e}", file.display()))?;
    let (text, found) = remove_text(&old, name).map_err(|e| format!("{}: {e}", file.display()))?;
    if found {
        config::write_atomic(file, &text)?;
    }
    Ok(found)
}

/// New file with every explicitly installed package, repo and foreign (AUR) apart.
pub fn write_import(file: &Path, explicit: &Set, foreign: &Set) -> Result<(), String> {
    if file.exists() {
        return Err(format!(
            "{} already exists; not overwriting it",
            file.display()
        ));
    }
    let quoted =
        |names: Vec<&String>| -> String { names.iter().map(|n| format!("  \"{n}\",\n")).collect() };
    let repo = quoted(explicit.iter().filter(|n| !foreign.contains(*n)).collect());
    let aur = quoted(explicit.iter().filter(|n| foreign.contains(*n)).collect());
    let text = format!(
        "# Imported by novarch from the explicitly installed packages.\npackages = [\n  # repo\n{repo}  # AUR / foreign\n{aur}]\n"
    );
    config::write_atomic(file, &text)
}

fn packages_array(doc: &mut DocumentMut) -> Result<&mut Array, String> {
    doc.get_mut("packages")
        .and_then(|i| i.as_array_mut())
        .ok_or_else(|| "no `packages = [...]` list".to_string())
}

fn prefix(v: &Value) -> String {
    v.decor()
        .prefix()
        .and_then(|p| p.as_str())
        .unwrap_or("")
        .to_string()
}

/// Everything up to and including the last newline (comments belonging to earlier lines).
fn head(s: &str) -> &str {
    s.rfind('\n').map_or("", |i| &s[..=i])
}

/// Everything after the first newline (drops the comment of the line being removed).
fn after_first_newline(s: &str) -> &str {
    s.find('\n').map_or("", |i| &s[i + 1..])
}

fn append_text(text: &str, names: &[String]) -> Result<String, String> {
    let mut doc: DocumentMut = text.parse().map_err(|e| format!("{e}"))?;
    let arr = packages_array(&mut doc)?;
    let trailing = |a: &Array| a.trailing().as_str().unwrap_or("").to_string();
    let multiline = arr.iter().any(|v| prefix(v).contains('\n')) || trailing(arr).contains('\n');
    let indent = arr
        .iter()
        .last()
        .map(|v| prefix(v).rsplit('\n').next().unwrap_or("").to_string())
        .filter(|i| !i.is_empty())
        .unwrap_or_else(|| "  ".to_string());
    for name in names {
        let first = arr.is_empty();
        // A comment after the old last element lives in the array's trailing text; keep it
        // on that line by moving it in front of the new element.
        let old = trailing(arr);
        arr.push(name.as_str());
        let new_prefix = if multiline {
            let h = head(&old);
            format!("{}{indent}", if h.is_empty() { "\n" } else { h })
        } else if first {
            String::new()
        } else {
            " ".to_string()
        };
        let last = arr.len() - 1;
        if let Some(v) = arr.get_mut(last) {
            v.decor_mut().set_prefix(new_prefix);
        }
        arr.set_trailing(if multiline { "\n" } else { "" });
    }
    if multiline {
        arr.set_trailing_comma(true);
    }
    Ok(doc.to_string())
}

fn remove_text(text: &str, name: &str) -> Result<(String, bool), String> {
    let mut doc: DocumentMut = text.parse().map_err(|e| format!("{e}"))?;
    let arr = packages_array(&mut doc)?;
    let Some(i) = arr.iter().position(|v| v.as_str() == Some(name)) else {
        return Ok((text.to_string(), false));
    };
    let removed_prefix = arr.get(i).map(prefix).unwrap_or_default();
    let keep = head(&removed_prefix).to_string();
    arr.remove(i);
    if let Some(next) = arr.get_mut(i) {
        let p = prefix(next);
        let new = if keep.is_empty() {
            if i == 0 { String::new() } else { p }
        } else {
            format!("{keep}{}", after_first_newline(&p))
        };
        next.decor_mut().set_prefix(new);
    } else if !keep.is_empty() {
        let t = arr.trailing().as_str().unwrap_or("").to_string();
        arr.set_trailing(format!("{keep}{}", after_first_newline(&t)));
    }
    Ok((doc.to_string(), true))
}

/// makepkg's rule: alphanumerics and `@._+-`, not starting with `-` or `.`.
/// Also keeps a name like `--overwrite=*` from reaching pacman as an option.
pub fn valid_name(name: &str) -> bool {
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
    fn append_keeps_comments_and_layout() {
        let text = "# dev\npackages = [\n    \"git\",\n    \"neovim\", # editor\n]\n";
        let out = append_text(text, &["tree".into(), "jq".into()]).unwrap();
        assert_eq!(
            out,
            "# dev\npackages = [\n    \"git\",\n    \"neovim\", # editor\n    \"tree\",\n    \"jq\",\n]\n"
        );
        assert_eq!(
            append_text("packages = [\"a\"]\n", &["b".into()]).unwrap(),
            "packages = [\"a\", \"b\"]\n"
        );
        assert_eq!(
            append_text("packages = []\n", &["b".into()]).unwrap(),
            "packages = [\"b\"]\n"
        );
    }

    #[test]
    fn remove_keeps_other_comments() {
        let text = "packages = [\n  \"git\", # vcs\n  # AUR\n  \"yay\", # helper\n  \"paru\",\n]\n";
        let (out, found) = remove_text(text, "yay").unwrap();
        assert!(found);
        assert_eq!(
            out,
            "packages = [\n  \"git\", # vcs\n  # AUR\n  \"paru\",\n]\n"
        );
        let (out, _) = remove_text(text, "paru").unwrap();
        assert_eq!(
            out,
            "packages = [\n  \"git\", # vcs\n  # AUR\n  \"yay\", # helper\n]\n"
        );
        let (out, _) = remove_text(text, "git").unwrap();
        assert_eq!(
            out,
            "packages = [\n  # AUR\n  \"yay\", # helper\n  \"paru\",\n]\n"
        );
        assert_eq!(
            remove_text("packages = [\"a\", \"b\"]\n", "a").unwrap().0,
            "packages = [\"b\"]\n"
        );
        assert!(!remove_text(text, "nope").unwrap().1);
    }

    #[test]
    fn import_file_parses_back() {
        let dir = std::env::temp_dir().join(format!("novarch-import-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let f = dir.join("pre.toml");
        let set = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Set>();
        write_import(&f, &set(&["git", "yay"]), &set(&["yay"])).unwrap();
        let names = parse(&fs::read_to_string(&f).unwrap(), "pre.toml").unwrap();
        assert_eq!(names, ["git", "yay"]);
        assert!(write_import(&f, &Set::new(), &Set::new()).is_err());
        fs::remove_dir_all(&dir).unwrap();
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
