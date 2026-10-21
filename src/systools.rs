//! System Tools (Revo: Tools › Windows Tools): one place to open the operating system's own
//! utilities, plus tools the user adds.

use crate::config::{CustomTool, Settings};
use anyhow::{bail, Result};
use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
pub struct Tool {
    pub id: String,
    pub name: String,
    pub description: String,
    /// Group heading: "System", "Disks", "Network", "Security", …
    pub group: &'static str,
    /// Command line to run.
    pub command: String,
    /// Runs in a terminal window that stays open (console tools like `chkdsk`, `ip a`).
    pub terminal: bool,
    /// Needs administrator rights.
    pub admin: bool,
    /// Added by the user (removable).
    pub custom: bool,
}

impl Tool {
    pub fn builtin(id: &str, group: &'static str, name: &str, description: &str, command: &str) -> Self {
        Self { id: id.into(), name: name.into(), description: description.into(), group, command: command.into(), terminal: false, admin: false, custom: false }
    }
    pub fn in_terminal(mut self) -> Self {
        self.terminal = true;
        self
    }
    pub fn as_admin(mut self) -> Self {
        self.admin = true;
        self
    }
}

/// Built-in tools available on this system, then the user's own.
pub fn list(settings: &Settings) -> Vec<Tool> {
    let mut v = crate::platform::builtin_tools();
    const ORDER: [&str; 5] = ["System", "Disks", "Network", "Security", "Software"];
    v.sort_by_key(|t| ORDER.iter().position(|g| *g == t.group).unwrap_or(ORDER.len()));
    v.extend(settings.custom_tools.iter().enumerate().map(|(i, t)| Tool {
        id: format!("custom:{i}"),
        name: t.name.clone(),
        description: String::new(),
        group: "My tools",
        command: t.command.clone(),
        terminal: false,
        admin: false,
        custom: true,
    }));
    v
}

pub fn find<'a>(tools: &'a [Tool], key: &str) -> Result<&'a Tool> {
    tools.iter().find(|t| t.id.eq_ignore_ascii_case(key) || t.name.eq_ignore_ascii_case(key)).ok_or_else(|| anyhow::anyhow!("no tool \"{key}\" (see `cleancore tools list`)"))
}

pub fn run(t: &Tool) -> Result<()> {
    crate::platform::run_tool(&t.command, t.terminal, t.admin)
}

pub fn add(settings: &mut Settings, name: &str, command: &str) -> Result<()> {
    if name.trim().is_empty() || command.trim().is_empty() {
        bail!("a name and a command are needed");
    }
    if settings.custom_tools.iter().any(|t| t.name.eq_ignore_ascii_case(name.trim())) {
        bail!("a tool named \"{}\" already exists", name.trim());
    }
    settings.custom_tools.push(CustomTool { name: name.trim().into(), command: command.trim().into() });
    settings.save()
}

pub fn remove(settings: &mut Settings, name: &str) -> Result<()> {
    let before = settings.custom_tools.len();
    settings.custom_tools.retain(|t| !t.name.eq_ignore_ascii_case(name));
    if settings.custom_tools.len() == before {
        bail!("no tool of yours is named \"{name}\"");
    }
    settings.save()
}
