use std::path::PathBuf;

use crate::simulation::Snowfall;

#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Prefs {
    pub snowfall: Snowfall,
    pub accumulates: bool,
    pub thaws: bool,
    pub snow_on_windows: bool,
    pub behind_windows: bool,
}

impl Default for Prefs {
    fn default() -> Prefs {
        Prefs { snowfall: Snowfall::Steady, accumulates: true, thaws: true, snow_on_windows: true, behind_windows: false }
    }
}

fn path() -> PathBuf {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".config"));
    base.join("snowhush-kde.conf")
}

impl Prefs {
    pub fn load() -> Prefs {
        let mut prefs = Prefs::default();
        let Ok(text) = std::fs::read_to_string(path()) else { return prefs };
        for line in text.lines() {
            let Some((key, value)) = line.split_once('=') else { continue };
            let (key, value) = (key.trim(), value.trim());
            let flag = value == "true";
            match key {
                "snowfall" => {
                    if let Some(&snowfall) = Snowfall::ALL.iter().find(|s| s.title().eq_ignore_ascii_case(value)) {
                        prefs.snowfall = snowfall;
                    }
                }
                "accumulate" => prefs.accumulates = flag,
                "melt" => prefs.thaws = flag,
                "snow_on_windows" => prefs.snow_on_windows = flag,
                "behind_windows" => prefs.behind_windows = flag,
                _ => {}
            }
        }
        prefs
    }

    pub fn save(&self) {
        let text = format!(
            "snowfall={}\naccumulate={}\nmelt={}\nsnow_on_windows={}\nbehind_windows={}\n",
            self.snowfall.title(),
            self.accumulates,
            self.thaws,
            self.snow_on_windows,
            self.behind_windows
        );
        let _ = std::fs::write(path(), text);
    }
}
