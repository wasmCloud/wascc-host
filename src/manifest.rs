use std::collections::HashMap;

#[derive(Debug, Clone)]
#[cfg_attr(feature = "manifest", derive(serde::Serialize, serde::Deserialize))]
pub struct HostManifest {
    pub actors: Vec<String>,
    pub capabilities: Vec<String>,
    pub config: Vec<ConfigEntry>,
}

#[derive(Debug, Clone)]
#[cfg_attr(feature = "manifest", derive(serde::Serialize, serde::Deserialize))]
pub struct ConfigEntry {
    pub actor: String,
    pub capability: String,
    pub values: HashMap<String, String>,
}

#[cfg(feature = "manifest")]
use std::{fs::File, io::Read, path::Path};
#[cfg(feature = "manifest")]
impl HostManifest {
    pub fn from_yaml(
        path: impl AsRef<Path>,
        expand_env: bool,
    ) -> ::std::result::Result<HostManifest, Box<dyn std::error::Error>> {
        let mut contents = String::new();
        let mut file = File::open(path)?;
        file.read_to_string(&mut contents)?;
        if expand_env {
            contents = Self::expand_env(&contents);
        }

        match serde_yaml::from_str::<HostManifest>(&contents) {
            Ok(m) => Ok(m),
            Err(e) => Err(format!("Failed to deserialize yaml: {} ", e).into()),
        }
    }

    pub fn from_json(
        path: impl AsRef<Path>,
        expand_env: bool,
    ) -> ::std::result::Result<HostManifest, Box<dyn std::error::Error>> {
        let mut contents = String::new();
        let mut file = File::open(path)?;
        file.read_to_string(&mut contents)?;
        if expand_env {
            contents = Self::expand_env(&contents);
        }

        match serde_json::from_str::<HostManifest>(&contents) {
            Ok(m) => Ok(m),
            Err(_) => Err("Failed to deserialize json".into()),
        }
    }

    fn expand_env(contents: &str) -> String {
        let mut options = envmnt::ExpandOptions::new();
        options.default_to_empty = false; // If environment variable not found, leave unexpanded.
        options.expansion_type = Some(envmnt::ExpansionType::UnixBrackets); // ${VAR}

        envmnt::expand(contents, Some(options))
    }
}

#[cfg(feature = "manifest")]
#[cfg(test)]
mod test {
    use super::ConfigEntry;
    use std::collections::HashMap;

    #[test]
    fn round_trip() {
        let manifest = super::HostManifest {
            actors: vec!["a".to_string(), "b".to_string(), "c".to_string()],
            capabilities: vec!["wascc:one".to_string(), "wascc:two".to_string()],
            config: vec![ConfigEntry {
                actor: "a".to_string(),
                capability: "wascc:one".to_string(),
                values: gen_values(),
            }],
        };
        let yaml = serde_yaml::to_string(&manifest).unwrap();
        assert_eq!(yaml, "---\nactors:\n  - a\n  - b\n  - c\ncapabilities:\n  - \"wascc:one\"\n  - \"wascc:two\"\nconfig:\n  - actor: a\n    capability: \"wascc:one\"\n    values:\n      ROOT: /tmp");
    }

    #[test]
    fn env_expansion() {
        let values = vec!["echo Test", "echo $TEST_EXPAND_ENV_TEMP", "echo ${TEST_EXPAND_ENV_TEMP}", "echo ${TEST_EXPAND_ENV_TMP}"];
        let expected = vec!["echo Test", "echo $TEST_EXPAND_ENV_TEMP", "echo /tmp", "echo ${TEST_EXPAND_ENV_TMP}"];

        envmnt::set("TEST_EXPAND_ENV_TEMP", "/tmp");
        for (got, expected) in values.iter().map(|v| super::HostManifest::expand_env(v)).zip(expected.iter()) {
            assert_eq!(*expected, got);
        }
        envmnt::remove("TEST_EXPAND_ENV_TEMP");
    }

    fn gen_values() -> HashMap<String, String> {
        let mut hm = HashMap::new();
        hm.insert("ROOT".to_string(), "/tmp".to_string());

        hm
    }
}

/*
---
actors:
- bob
- al
- steve
capabilities:
- file://foo
config:
- actor: bob
  capability: wascc:messaging
  values:
    a: b
    */
