use ksni::blocking::TrayMethods;
use ksni::menu::{CheckmarkItem, RadioGroup, RadioItem, StandardItem, SubMenu};
use ksni::{MenuItem, ToolTip, Tray};

use crate::kwin::SharedState;
use crate::prefs::Prefs;
use crate::simulation::Snowfall;

struct SnowTray {
    shared: SharedState,
}

impl SnowTray {
    fn prefs(&self) -> Prefs {
        self.shared.lock().unwrap().prefs
    }

    fn change(&mut self, edit: impl FnOnce(&mut Prefs)) {
        let mut shared = self.shared.lock().unwrap();
        edit(&mut shared.prefs);
        shared.prefs.save();
        shared.prefs_generation += 1;
    }
}

fn check(label: &str, checked: bool, edit: fn(&mut Prefs)) -> MenuItem<SnowTray> {
    CheckmarkItem { label: label.into(), checked, activate: Box::new(move |tray: &mut SnowTray| tray.change(edit)), ..Default::default() }.into()
}

impl Tray for SnowTray {
    fn id(&self) -> String {
        "snowhush-kde".into()
    }

    fn title(&self) -> String {
        "Snowhush".into()
    }

    fn icon_name(&self) -> String {
        "weather-snow".into()
    }

    fn tool_tip(&self) -> ToolTip {
        ToolTip { title: "Snowhush".into(), description: format!("Snowfall: {}", self.prefs().snowfall.title()), ..Default::default() }
    }

    fn menu(&self) -> Vec<MenuItem<Self>> {
        let prefs = self.prefs();
        vec![
            SubMenu {
                label: "Snowfall".into(),
                icon_name: "weather-snow".into(),
                submenu: vec![
                    RadioGroup {
                        selected: Snowfall::ALL.iter().position(|&s| s == prefs.snowfall).unwrap_or(2),
                        select: Box::new(|tray: &mut SnowTray, index| tray.change(|prefs| prefs.snowfall = Snowfall::ALL[index])),
                        options: Snowfall::ALL.iter().map(|s| RadioItem { label: s.title().into(), ..Default::default() }).collect(),
                    }
                    .into(),
                ],
                ..Default::default()
            }
            .into(),
            MenuItem::Separator,
            check("Accumulate", prefs.accumulates, |prefs| prefs.accumulates = !prefs.accumulates),
            check("Melt Snow", prefs.thaws, |prefs| prefs.thaws = !prefs.thaws),
            check("Snow on Windows", prefs.snow_on_windows, |prefs| prefs.snow_on_windows = !prefs.snow_on_windows),
            check("Behind Windows", prefs.behind_windows, |prefs| prefs.behind_windows = !prefs.behind_windows),
            MenuItem::Separator,
            StandardItem {
                label: "Quit".into(),
                icon_name: "application-exit".into(),
                activate: Box::new(|tray: &mut SnowTray| tray.shared.lock().unwrap().quit = true),
                ..Default::default()
            }
            .into(),
        ]
    }
}

pub fn spawn(shared: SharedState) -> Option<ksni::blocking::Handle<impl Tray>> {
    match (SnowTray { shared }).spawn() {
        Ok(handle) => Some(handle),
        Err(error) => {
            eprintln!("snowhush: no tray icon: {error}");
            None
        }
    }
}
