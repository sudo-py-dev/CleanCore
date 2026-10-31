//! Portable parsers for installed package-manager inventory. No commands or network I/O.

use super::{Program, ProgramKind};
#[cfg(any(not(windows), test))]
use std::path::Path;

#[cfg(any(not(windows), test))]
pub(crate) fn nix_programs(text: &str) -> std::result::Result<Vec<Program>, String> {
    let value: serde_json::Value = serde_json::from_str(text).map_err(|e| e.to_string())?;
    let elements = value.get("elements").ok_or("unsupported Nix profile JSON")?;
    let entries: Vec<_> = if let Some(map) = elements.as_object() {
        map.values().collect()
    } else if let Some(array) = elements.as_array() {
        array.iter().collect()
    } else {
        return Err("unsupported Nix profile elements".into());
    };
    let mut programs = Vec::new();
    for element in entries {
        if element.get("active").and_then(|v| v.as_bool()) == Some(false) {
            continue;
        }
        for path in element.get("storePaths").and_then(|v| v.as_array()).into_iter().flatten().filter_map(|v| v.as_str()) {
            let Some(file) = Path::new(path).file_name().and_then(|s| s.to_str()) else {
                continue;
            };
            let Some((hash, name)) = file.split_once('-') else {
                continue;
            };
            if Path::new(path).parent() != Some(Path::new("/nix/store"))
                || hash.len() != 32
                || !hash.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
                || name.is_empty()
            {
                continue;
            }
            let mut p = Program::new(format!("nix:{path}"), name, ProgramKind::Nix);
            // A store path is stable across profile index changes and is an exact remove selector.
            p.package = Some(path.into());
            p.install_location = Some(path.into());
            p.user_scope = true;
            p.uninstall_string = Some(format!("nix profile remove {path}"));
            programs.push(p);
            // Multiple outputs (out/dev/doc) still belong to one profile entry.
            break;
        }
    }
    Ok(programs)
}

#[cfg(any(not(windows), test))]
pub(crate) fn homebrew_programs(text: &str) -> std::result::Result<Vec<Program>, String> {
    let value: serde_json::Value = serde_json::from_str(text).map_err(|e| e.to_string())?;
    let formulae = value.get("formulae").and_then(|v| v.as_array()).ok_or("unsupported Homebrew inventory JSON")?;
    let mut programs = Vec::new();
    for formula in formulae {
        let Some(name) = formula.get("full_name").or_else(|| formula.get("name")).and_then(|v| v.as_str()) else {
            continue;
        };
        let Some(installed) = formula.get("installed").and_then(|v| v.as_array()).filter(|v| !v.is_empty()) else {
            continue;
        };
        let mut p = Program::new(format!("brew:{name}"), name, ProgramKind::Homebrew);
        p.version = installed.iter().filter_map(|v| v.get("version").and_then(|v| v.as_str())).collect::<Vec<_>>().join(", ");
        p.package = Some(name.into());
        p.website = formula.get("homepage").and_then(|v| v.as_str()).map(str::to_string);
        p.comment = formula.get("desc").and_then(|v| v.as_str()).map(str::to_string);
        p.system = installed.iter().all(|v| v.get("installed_on_request").and_then(|v| v.as_bool()) == Some(false));
        p.user_scope = true;
        p.uninstall_string = Some(format!("brew uninstall --formula -- {name}"));
        programs.push(p);
    }
    Ok(programs)
}

/// Installed Scoop manifests (including portable apps absent from the Windows registry).
/// This reader never evaluates install/uninstall scripts embedded in a manifest.
#[cfg(any(windows, test))]
pub(crate) fn scoop_programs(roots: &[(std::path::PathBuf, bool)]) -> Vec<Program> {
    let mut programs = Vec::new();
    for (root, user_scope) in roots {
        let Ok(apps) = std::fs::read_dir(root.join("apps")) else { continue };
        for app in apps.flatten() {
            let name = app.file_name().to_string_lossy().to_string();
            if name == "scoop" || name.is_empty() || !name.chars().all(|c| c.is_ascii_alphanumeric() || "-_.".contains(c)) {
                continue;
            }
            let current = app.path().join("current");
            let manifest = ["scoop-manifest.json", "manifest.json"].into_iter().find_map(|file| std::fs::read_to_string(current.join(file)).ok());
            let Some(manifest) = manifest else { continue };
            let Ok(value) = serde_json::from_str::<serde_json::Value>(&manifest) else { continue };
            let mut p = Program::new(format!("scoop:{}:{name}", if *user_scope { "user" } else { "global" }), &name, ProgramKind::Scoop);
            p.version = value.get("version").and_then(|v| v.as_str()).unwrap_or_default().into();
            p.comment = value.get("description").and_then(|v| v.as_str()).map(str::to_string);
            p.website = value.get("homepage").and_then(|v| v.as_str()).map(str::to_string);
            p.install_location = Some(current.display().to_string());
            p.package = Some(name.clone());
            p.user_scope = *user_scope;
            p.install_date = std::fs::metadata(&current).and_then(|m| m.modified()).ok().map(|t| chrono::DateTime::<chrono::Local>::from(t).date_naive());
            p.uninstall_string = Some(format!("scoop uninstall {}{name}", if *user_scope { "" } else { "--global " }));
            programs.push(p);
        }
    }
    programs
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nix_named_and_legacy_manifests_preserve_exact_store_selectors() {
        for text in [
            r#"{"elements":{"editor":{"active":true,"storePaths":["/nix/store/01234567890123456789012345678901-editor-1.2"]},"off":{"active":false,"storePaths":["/nix/store/01234567890123456789012345678901-off-1.0"]}}}"#,
            r#"{"elements":[{"active":true,"storePaths":["/nix/store/01234567890123456789012345678901-editor-1.2"]}]}"#,
        ] {
            let programs = nix_programs(text).unwrap();
            assert_eq!(programs.len(), 1);
            assert_eq!(programs[0].name, "editor-1.2");
            assert_eq!(programs[0].package.as_deref(), Some("/nix/store/01234567890123456789012345678901-editor-1.2"));
            assert!(!programs[0].kind.is_store_app());
        }
        assert!(nix_programs(r#"{"elements":{}}"#).unwrap().is_empty());
        assert!(nix_programs("{}").is_err());
    }

    #[test]
    fn homebrew_uses_installed_versions_and_marks_only_dependencies_as_system() {
        let text = r#"{"formulae":[{"full_name":"editor","versions":{"stable":"9.0"},"installed":[{"version":"2.0","installed_on_request":true}]},{"full_name":"libfoo","installed":[{"version":"1.0","installed_on_request":false}]},{"full_name":"not-installed","installed":[]}]}"#;
        let programs = homebrew_programs(text).unwrap();
        assert_eq!(programs.len(), 2);
        assert_eq!(programs[0].version, "2.0");
        assert!(!programs[0].system);
        assert!(programs[1].system);
        assert!(!programs[0].kind.is_store_app());
    }

    #[test]
    fn scoop_discovers_user_and_global_portable_apps_from_both_manifest_layouts() {
        let root = std::env::temp_dir().join(format!("cleancore-scoop-fixture-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        for (scope, file) in [("user", "scoop-manifest.json"), ("global", "manifest.json")] {
            let dir = root.join(scope).join("apps/editor/current");
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(dir.join(file), r#"{"version":"2.0","description":"Portable editor","pre_uninstall":"this must never be evaluated"}"#).unwrap();
        }
        let programs = scoop_programs(&[(root.join("user"), true), (root.join("global"), false)]);
        assert_eq!(programs.len(), 2);
        assert_ne!(programs[0].id, programs[1].id);
        assert!(programs[0].user_scope);
        assert!(!programs[1].user_scope);
        assert!(programs.iter().all(|p| p.kind == ProgramKind::Scoop && !p.kind.is_store_app()));
        std::fs::remove_dir_all(root).unwrap();
    }
}
