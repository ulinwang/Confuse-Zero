use super::Agent;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    collections::BTreeSet,
    env, fs,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
};

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Installation {
    pub agent: Agent,
    pub executable: Option<PathBuf>,
    pub version: Option<String>,
    pub root: PathBuf,
    pub root_readable: bool,
}

#[derive(Clone)]
pub struct Discovery {
    pub home: PathBuf,
    pub bin_dirs: Vec<PathBuf>,
    pub applications: Vec<PathBuf>,
}

impl Discovery {
    pub fn from_environment(home: PathBuf) -> Self {
        let mut bin_dirs = env::var_os("PATH")
            .map(|path| env::split_paths(&path).collect::<Vec<_>>())
            .unwrap_or_default();
        let additions = [
            home.join(".local/bin"),
            home.join(".kimi-code/bin"),
            home.join(".pi/agent/bin"),
            home.join(".bun/bin"),
            home.join("Library/pnpm"),
            PathBuf::from("/opt/homebrew/bin"),
            PathBuf::from("/usr/local/bin"),
        ];
        for directory in additions {
            if !bin_dirs.contains(&directory) {
                bin_dirs.push(directory);
            }
        }
        for (manager, suffix) in [
            (
                env::var_os("FNM_DIR")
                    .map(PathBuf::from)
                    .unwrap_or(home.join(".local/share/fnm"))
                    .join("node-versions"),
                "installation/bin",
            ),
            (home.join(".nvm/versions/node"), "bin"),
        ] {
            let mut versions = fs::read_dir(manager)
                .into_iter()
                .flatten()
                .filter_map(Result::ok)
                .map(|entry| entry.path().join(suffix))
                .collect::<Vec<_>>();
            versions.sort_by(|a, b| b.cmp(a));
            bin_dirs.extend(versions);
        }
        let applications = vec![home.join("Applications"), PathBuf::from("/Applications")];
        Self {
            home,
            bin_dirs,
            applications,
        }
    }

    pub fn scan(&self) -> Vec<Installation> {
        Agent::ALL
            .into_iter()
            .map(|agent| self.find(agent))
            .collect()
    }

    pub fn find(&self, agent: Agent) -> Installation {
        let root = root_for(&self.home, agent);
        let binary_names = match agent {
            Agent::KimiCode => &["kimi", "kimi-code"][..],
            Agent::Codex => &["codex"][..],
            Agent::ClaudeCode => &["claude"][..],
            Agent::DeepSeekHarness => &["dsh", "deepseek-harness"][..],
            Agent::Pi => &["pi"][..],
        };
        let mut candidates: Vec<(PathBuf, bool)> = self
            .bin_dirs
            .iter()
            .flat_map(|dir| binary_names.iter().map(move |name| (dir.join(name), false)))
            .collect();
        if agent == Agent::KimiCode {
            candidates.push((root.join("bin/kimi"), false));
        }
        for applications in &self.applications {
            match agent {
                Agent::Codex => {
                    for name in ["ChatGPT.app", "Codex.app"] {
                        let app = applications.join(name);
                        candidates.push((
                            app.join(
                                "Contents/Resources/codex-cli/CodexCLI.app/Contents/MacOS/codex",
                            ),
                            true,
                        ));
                        candidates.push((app.join("Contents/Resources/codex"), true));
                    }
                }
                Agent::KimiCode => {
                    let app = applications.join("Kimi Code.app");
                    if bundle_identifier(&app).as_deref() == Some("com.kimi.code.desktop") {
                        candidates.push((app.join("Contents/MacOS/Kimi Code"), true));
                    }
                }
                Agent::ClaudeCode => {
                    let app = applications.join("Claude.app");
                    candidates.push((app.join("Contents/Resources/claude"), true));
                    candidates.push((app.join("Contents/Resources/claude-code/claude"), true));
                }
                Agent::DeepSeekHarness => {
                    let app = applications.join("DeepSeek Harness.app");
                    if bundle_identifier(&app).as_deref() == Some("com.deepseek.dsh") {
                        candidates.push((app.join("Contents/MacOS/DeepSeek Harness"), true));
                    }
                }
                Agent::Pi => {}
            }
        }

        let mut seen = BTreeSet::new();
        let mut executable = None;
        let mut version = None;
        for (path, bundled) in candidates {
            if !seen.insert(path.clone()) || !is_executable(&path) {
                continue;
            }
            let package = package_metadata(&path, agent);
            if matches!(agent, Agent::Pi | Agent::DeepSeekHarness)
                && !bundled
                && package.is_none()
                && !official_pi_install(agent, &path)
            {
                continue;
            }
            version = package.and_then(|package| {
                package
                    .get("version")
                    .and_then(Value::as_str)
                    .map(str::to_owned)
            });
            executable = Some(path);
            break;
        }
        let root_readable = fs::read_dir(&root).is_ok();
        Installation {
            agent,
            executable,
            version,
            root,
            root_readable,
        }
    }
}

fn root_for(home: &Path, agent: Agent) -> PathBuf {
    let override_name = match agent {
        Agent::KimiCode => "KIMI_CODE_HOME",
        Agent::Codex => "CODEX_HOME",
        Agent::ClaudeCode => "CLAUDE_CONFIG_DIR",
        Agent::DeepSeekHarness => "DSH_HOME",
        Agent::Pi => "PI_CODING_AGENT_DIR",
    };
    let root = env::var_os(override_name)
        .map(PathBuf::from)
        .map(|path| expand_home(home, &path))
        .unwrap_or_else(|| match agent {
            Agent::KimiCode if !home.join(".kimi-code").exists() => env::var_os("KIMI_SHARE_DIR")
                .map(PathBuf::from)
                .map(|path| expand_home(home, &path))
                .filter(|path| path.exists())
                .or_else(|| home.join(".kimi").exists().then(|| home.join(".kimi")))
                .unwrap_or_else(|| home.join(".kimi-code")),
            Agent::KimiCode => home.join(".kimi-code"),
            Agent::Codex => home.join(".codex"),
            Agent::ClaudeCode => home.join(".claude"),
            Agent::DeepSeekHarness => home.join(".dsh"),
            Agent::Pi => home.join(".pi/agent"),
        });
    if agent != Agent::Pi {
        return root;
    }
    env::var_os("PI_CODING_AGENT_SESSION_DIR")
        .map(PathBuf::from)
        .or_else(|| {
            read_json(&root.join("settings.json"))
                .and_then(|value| value.get("sessionDir")?.as_str().map(PathBuf::from))
        })
        .map(|path| expand_home(home, &path))
        .filter(|path| path.is_absolute())
        .unwrap_or_else(|| root.join("sessions"))
}

fn expand_home(home: &Path, path: &Path) -> PathBuf {
    path.strip_prefix("~")
        .map(|tail| home.join(tail))
        .unwrap_or_else(|_| path.to_path_buf())
}

fn is_executable(path: &Path) -> bool {
    path.metadata()
        .is_ok_and(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0)
}

fn package_metadata(binary: &Path, agent: Agent) -> Option<Value> {
    let names = match agent {
        Agent::Pi => &[
            "@earendil-works/pi-coding-agent",
            "@mariozechner/pi-coding-agent",
        ][..],
        Agent::DeepSeekHarness => &["@deepseek-ai/dsh"][..],
        Agent::Codex => &["@openai/codex"][..],
        Agent::ClaudeCode => &["@anthropic-ai/claude-code"][..],
        Agent::KimiCode => &["@moonshot-ai/kimi-code"][..],
    };
    let resolved = binary.canonicalize().ok()?;
    for ancestor in resolved.ancestors().skip(1).take(8) {
        if let Some(value) = read_json(&ancestor.join("package.json"))
            && value
                .get("name")
                .and_then(Value::as_str)
                .is_some_and(|name| names.contains(&name))
        {
            return Some(value);
        }
    }
    None
}

fn official_pi_install(agent: Agent, binary: &Path) -> bool {
    agent == Agent::Pi
        && binary.canonicalize().ok().is_some_and(|resolved| {
            resolved.ancestors().skip(1).take(6).any(|dir| {
                read_json(&dir.join("managed-install.json")).is_some_and(|marker| {
                    marker.as_object().is_some_and(|fields| {
                        fields
                            .values()
                            .any(|v| v.as_str() == Some("@earendil-works/pi-coding-agent"))
                    })
                })
            })
        })
}

fn read_json(path: &Path) -> Option<Value> {
    if path.metadata().ok()?.len() > 1024 * 1024 {
        return None;
    }
    serde_json::from_slice(&fs::read(path).ok()?).ok()
}

fn bundle_identifier(app: &Path) -> Option<String> {
    let info = plist::Value::from_file(app.join("Contents/Info.plist")).ok()?;
    Some(
        info.as_dictionary()?
            .get("CFBundleIdentifier")?
            .as_string()?
            .to_owned(),
    )
}
