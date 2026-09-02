// SPDX-FileCopyrightText: 2026 Curtis Galloway
// SPDX-License-Identifier: Apache-2.0
//! Constants and the per-run environment: the config root and every
//! harness path the tool touches, derived once from HOME and
//! CLAUDE_CONFIG_DIR the way the reference script derives its globals.

use crate::paths;
use serde_json::{json, Value as Json};
use std::env;
use std::path::PathBuf;

pub const VERSION: &str = "0.2.0";
/// Manifests carry a `schema` (1 when absent). Older schemas are upgraded in
/// memory on every load and rewritten by --upgrade-manifests; a manifest newer
/// than this is refused, so the upgrade path is forward-only.
pub const MANIFEST_SCHEMA: i64 = 2;
pub const STATE_SCHEMA: i64 = 2;
pub const STATE_FILE_NAME: &str = ".qbranch-state.json";
pub const LEGACY_STATE_FILE_NAME: &str = ".agent-skills-state.json";
/// Never copied into a copy of a skills directory, and ignored when checking
/// whether a copy is up to date.
pub const COPY_IGNORE: [&str; 2] = [STATE_FILE_NAME, LEGACY_STATE_FILE_NAME];
/// Claude Code registers its own marketplace on first interactive run, but the
/// `claude plugin` CLI does not: on a fresh config dir it must be added like
/// any other before an official plugin can be installed.
pub const OFFICIAL_MARKETPLACE: &str = "claude-plugins-official";

pub fn official_marketplace_source() -> Json {
    json!({"source": {"source": "github", "repo": "anthropics/claude-plugins-official"}})
}

pub struct Ctx {
    /// The config root: manifests/, skills/ and claude-code/ live here. Set
    /// once per run by `state::resolve_root` before anything reads it.
    pub repo: PathBuf,
    pub home: PathBuf,
    pub default_skills_target: PathBuf,
    pub skill_repos_cache: PathBuf,
    /// Claude Code relocates its whole config dir via CLAUDE_CONFIG_DIR.
    pub claude_dir: PathBuf,
    pub claude_skills_link: PathBuf,
    pub claude_settings_file: PathBuf,
    /// User-scope MCP servers live in the app-owned ~/.claude.json, which
    /// moves into the config dir when CLAUDE_CONFIG_DIR is set. Read-only.
    pub claude_json_file: PathBuf,
    /// Antigravity keeps its harness state under ~/.gemini: instruction
    /// files at the top level, CLI-specific state one level down.
    pub agy_dir: PathBuf,
    pub agy_cli_dir: PathBuf,
    pub agy_skills_link: PathBuf,
    pub agy_settings_file: PathBuf,
}

fn env_nonempty(name: &str) -> Option<String> {
    env::var(name).ok().filter(|s| !s.is_empty())
}

impl Ctx {
    pub fn from_env() -> Ctx {
        let home = paths::home_dir();
        let config_dir = env_nonempty("CLAUDE_CONFIG_DIR");
        let claude_dir = match &config_dir {
            Some(s) => paths::clean(s),
            None => home.join(".claude"),
        };
        let claude_json_file = if config_dir.is_some() {
            claude_dir.join(".claude.json")
        } else {
            home.join(".claude.json")
        };
        let agy_dir = home.join(".gemini");
        let agy_cli_dir = agy_dir.join("antigravity-cli");
        Ctx {
            repo: PathBuf::from("."),
            default_skills_target: home.join(".agents").join("skills"),
            skill_repos_cache: home.join(".agents").join("skill-repos"),
            claude_skills_link: claude_dir.join("skills"),
            claude_settings_file: claude_dir.join("settings.json"),
            claude_dir,
            claude_json_file,
            agy_skills_link: agy_cli_dir.join("skills"),
            agy_settings_file: agy_cli_dir.join("settings.json"),
            agy_dir,
            agy_cli_dir,
            home,
        }
    }

    /// Expand a manifest path: `${QBRANCH_ROOT}`, any `${VAR}`, a leading `~`.
    pub fn expand(&self, raw: &str) -> PathBuf {
        let s = raw.replace("${QBRANCH_ROOT}", &self.repo.to_string_lossy());
        let s = paths::expandvars(&s);
        paths::expanduser(&paths::clean(&s), &self.home)
    }

    pub fn claude_installed(&self) -> bool {
        paths::which("claude").is_some() || self.claude_dir.is_dir()
    }

    pub fn agy_installed(&self) -> bool {
        paths::which("agy").is_some() || self.agy_cli_dir.is_dir()
    }

    pub fn manifests_dir(&self) -> PathBuf {
        self.repo.join("manifests")
    }

    pub fn manifest_path(&self, name: &str) -> PathBuf {
        self.manifests_dir().join(format!("{name}.json"))
    }
}
