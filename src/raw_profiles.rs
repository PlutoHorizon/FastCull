//! Inventory of actual embedded camera profiles and matching installed DCPs.
use std::path::Path;
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Choice {
    pub name: String,
    pub label: String,
}
pub fn choices(path: &Path, model: &str) -> Vec<Choice> {
    let mut out = Vec::new();
    if let Ok(profiles) = crate::dcp::read_embedded(path) {
        for (index, p) in profiles.into_iter().enumerate() {
            out.push(Choice {
                name: if index == 0 {
                    "Embedded".into()
                } else {
                    p.name.clone()
                },
                label: format!("{}（文件内嵌）", p.name),
            });
        }
    }
    if crate::enhanced_profile::available() {
        out.push(Choice {
            name: "Adobe Color".into(),
            label: "Adobe Color".into(),
        });
    }
    for name in crate::dcp::installed_names(model) {
        if !out.iter().any(|p| p.name == name) {
            out.push(Choice {
                label: name.clone(),
                name,
            });
        }
    }
    out
}
