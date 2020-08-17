use std::collections::HashMap;

#[derive(Debug, Clone)]
#[cfg_attr(feature = "manifest", derive(serde::Serialize, serde::Deserialize))]
pub struct HostManifest {
    #[serde(default)]
    #[serde(skip_serializing_if = "HashMap::is_empty")]
    pub labels: HashMap<String, String>,
    pub actors: Vec<String>,
    pub capabilities: Vec<Capability>,
    pub bindings: Vec<BindingEntry>,
}

#[derive(Debug, Clone)]
#[cfg_attr(feature = "manifest", derive(serde::Serialize, serde::Deserialize))]
pub struct Capability {
    pub path: String,
    pub binding_name: Option<String>,
}

#[derive(Debug, Clone)]
#[cfg_attr(feature = "manifest", derive(serde::Serialize, serde::Deserialize))]
pub struct BindingEntry {
    pub actor: String,
    pub capability: String,
    pub binding: Option<String>,
    pub values: Option<HashMap<String, String>>,
}

#[cfg(feature = "manifest")]
use std::{fs::File, io::Read, path::Path};
#[cfg(feature = "manifest")]
impl HostManifest {
    /// Creates an instance of a host manifest from a file path. The de-serialization
    /// type will be chosen based on the file path extension, selecting YAML for .yaml
    /// or .yml files, and JSON for all other file extensions. If the path has no extension, the
    /// de-serialization type chosen will be YAML.
    pub fn from_path(
        path: impl AsRef<Path>,
        expand_env: bool,
    ) -> std::result::Result<HostManifest, Box<dyn std::error::Error + Send + Sync>> {
        let mut contents = String::new();
        let mut file = File::open(path.as_ref())?;
        file.read_to_string(&mut contents)?;
        if expand_env {
            contents = Self::expand_env(&contents);
        }
        match path.as_ref().extension() {
            Some(e) => {
                let e = e.to_str().unwrap().to_lowercase(); // convert away from the FFI str
                if e == "yaml" || e == "yml" {
                    serde_yaml::from_str::<HostManifest>(&contents).map_err(|e| e.into())
                } else {
                    serde_json::from_str::<HostManifest>(&contents).map_err(|e| e.into())
                }
            }
            None => serde_yaml::from_str::<HostManifest>(&contents).map_err(|e| e.into()),
        }
    }

    fn expand_env(contents: &str) -> String {
        let mut options = envmnt::ExpandOptions::new();
        options.default_to_empty = false; // If environment variable not found, leave unexpanded.
        options.expansion_type = Some(envmnt::ExpansionType::UnixBracketsWithDefaults); // ${VAR:DEFAULT}

        envmnt::expand(contents, Some(options))
    }
}

#[cfg(feature = "manifest")]
#[cfg(test)]
mod test {
    use super::{BindingEntry, Capability};
    use std::collections::HashMap;

    #[test]
    fn round_trip() {
        let manifest = super::HostManifest {
            labels: HashMap::new(),
            actors: vec!["a".to_string(), "b".to_string(), "c".to_string()],
            capabilities: vec![
                Capability {
                    path: "one".to_string(),
                    binding_name: Some("default".to_string()),
                },
                Capability {
                    path: "two".to_string(),
                    binding_name: Some("default".to_string()),
                },
            ],
            bindings: vec![BindingEntry {
                actor: "a".to_string(),
                binding: Some("default".to_string()),
                capability: "wascc:one".to_string(),
                values: Some(gen_values()),
            }],
        };
        let yaml = serde_yaml::to_string(&manifest).unwrap();
        assert_eq!(yaml, "---\nactors:\n  - a\n  - b\n  - c\ncapabilities:\n  - path: one\n    binding_name: default\n  - path: two\n    binding_name: default\nbindings:\n  - actor: a\n    capability: \"wascc:one\"\n    binding: default\n    values:\n      ROOT: /tmp");
    }

    #[test]
    fn round_trip_with_labels() {
        let manifest = super::HostManifest {
            labels: {
                let mut hm = HashMap::new();
                hm.insert("test".to_string(), "value".to_string());
                hm
            },
            actors: vec!["a".to_string(), "b".to_string(), "c".to_string()],
            capabilities: vec![
                Capability {
                    path: "one".to_string(),
                    binding_name: Some("default".to_string()),
                },
                Capability {
                    path: "two".to_string(),
                    binding_name: Some("default".to_string()),
                },
            ],
            bindings: vec![BindingEntry {
                actor: "a".to_string(),
                binding: Some("default".to_string()),
                capability: "wascc:one".to_string(),
                values: Some(gen_values()),
            }],
        };
        let yaml = serde_yaml::to_string(&manifest).unwrap();
        assert_eq!(yaml, "---\nlabels:\n  test: value\nactors:\n  - a\n  - b\n  - c\ncapabilities:\n  - path: one\n    binding_name: default\n  - path: two\n    binding_name: default\nbindings:\n  - actor: a\n    capability: \"wascc:one\"\n    binding: default\n    values:\n      ROOT: /tmp");
    }

    #[test]
    fn env_expansion() {
        let values = vec![
            "echo Test",
            "echo $TEST_EXPAND_ENV_TEMP",
            "echo ${TEST_EXPAND_ENV_TEMP}",
            "echo ${TEST_EXPAND_ENV_TMP}",
            "echo ${TEST_EXPAND_ENV_TEMP:/etc}",
            "echo ${TEST_EXPAND_ENV_TMP:/etc}",
        ];
        let expected = vec![
            "echo Test",
            "echo $TEST_EXPAND_ENV_TEMP",
            "echo /tmp",
            "echo ${TEST_EXPAND_ENV_TMP}",
            "echo /tmp",
            "echo /etc",
        ];

        envmnt::set("TEST_EXPAND_ENV_TEMP", "/tmp");
        for (got, expected) in values
            .iter()
            .map(|v| super::HostManifest::expand_env(v))
            .zip(expected.iter())
        {
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
