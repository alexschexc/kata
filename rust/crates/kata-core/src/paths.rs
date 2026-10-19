//! State and config roots, ported from `src/platform_paths.zig`.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    State,
    Config,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HomeUnavailable;

impl std::fmt::Display for HomeUnavailable {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "HomeUnavailable")
    }
}

impl std::error::Error for HomeUnavailable {}

/// Directory holding Kata's state or config tree. `env` looks up an environment variable.
/// XDG variables win everywhere; on Windows LOCALAPPDATA/APPDATA and then USERPROFILE follow;
/// otherwise `$HOME/.local/state` or `$HOME/.config`.
pub fn root(
    env: impl Fn(&str) -> Option<String>,
    windows: bool,
    kind: Kind,
) -> Result<String, HomeUnavailable> {
    let xdg = if kind == Kind::State {
        "XDG_STATE_HOME"
    } else {
        "XDG_CONFIG_HOME"
    };
    if let Some(value) = env(xdg) {
        return Ok(value);
    }
    if windows {
        let appdata = if kind == Kind::State {
            "LOCALAPPDATA"
        } else {
            "APPDATA"
        };
        if let Some(value) = env(appdata) {
            return Ok(value);
        }
        if let Some(profile) = env("USERPROFILE") {
            return Ok(format!(
                "{profile}/AppData/{}",
                if kind == Kind::State {
                    "Local"
                } else {
                    "Roaming"
                }
            ));
        }
    }
    let home = env("HOME").ok_or(HomeUnavailable)?;
    Ok(format!(
        "{home}/{}",
        if kind == Kind::State {
            ".local/state"
        } else {
            ".config"
        }
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn env(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
        let map: HashMap<String, String> = pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        move |name| map.get(name).cloned()
    }

    #[test]
    fn windows_uses_localappdata_for_state_and_roaming_appdata_for_plans_without_home() {
        let e = env(&[
            ("LOCALAPPDATA", "C:/Users/Reader/AppData/Local"),
            ("APPDATA", "C:/Users/Reader/AppData/Roaming"),
        ]);
        assert_eq!(
            root(&e, true, Kind::State).unwrap(),
            "C:/Users/Reader/AppData/Local"
        );
        assert_eq!(
            root(&e, true, Kind::Config).unwrap(),
            "C:/Users/Reader/AppData/Roaming"
        );
    }

    #[test]
    fn windows_userprofile_fallback_follows_native_appdata_layout() {
        let e = env(&[
            ("USERPROFILE", "C:/Users/Reader"),
            ("HOME", "/different-home"),
        ]);
        assert_eq!(
            root(&e, true, Kind::State).unwrap(),
            "C:/Users/Reader/AppData/Local"
        );
        assert_eq!(
            root(&e, true, Kind::Config).unwrap(),
            "C:/Users/Reader/AppData/Roaming"
        );
    }

    #[test]
    fn xdg_precedence_and_unix_home_layout_are_preserved() {
        let e = env(&[("HOME", "/reader"), ("LOCALAPPDATA", "C:/Local")]);
        assert_eq!(
            root(&e, false, Kind::State).unwrap(),
            "/reader/.local/state"
        );
        assert_eq!(root(&e, false, Kind::Config).unwrap(), "/reader/.config");
        let e = env(&[
            ("HOME", "/reader"),
            ("XDG_STATE_HOME", "/xdg-state"),
            ("XDG_CONFIG_HOME", "/xdg-config"),
        ]);
        assert_eq!(root(&e, true, Kind::State).unwrap(), "/xdg-state");
        assert_eq!(root(&e, true, Kind::Config).unwrap(), "/xdg-config");
    }

    #[test]
    fn missing_environment_reports_home_unavailable_rather_than_guessing() {
        let e = env(&[]);
        assert_eq!(root(&e, true, Kind::State), Err(HomeUnavailable));
        assert_eq!(root(&e, false, Kind::Config), Err(HomeUnavailable));
    }
}
