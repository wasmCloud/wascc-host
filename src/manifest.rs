use std::collections::HashMap;
use std::{path::Path, io::Read, fs::File};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HostManifest {
    pub actors: Vec<String>,
    pub capabilities: Vec<String>,
    pub config: Vec<ConfigEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConfigEntry {
    pub actor: String,
    pub capability: String,
    pub values: HashMap<String, String>,
}

impl HostManifest {
    pub fn from_yaml(path: impl AsRef<Path>) -> ::std::result::Result<HostManifest, Box<dyn std::error::Error>> {
        let mut buf = Vec::new();
        let mut file = File::open(path)?;
        file.read_to_end(&mut buf)?;
        match serde_yaml::from_slice::<HostManifest>(&buf) {
            Ok(m) => Ok(m),
            Err(e) => Err(format!("Failed to deserialize yaml: {} ", e).into())
        }
    }

    pub fn from_json(path: impl AsRef<Path>) -> ::std::result::Result<HostManifest, Box<dyn std::error::Error>> {
        let mut buf = Vec::new();
        let mut file = File::open(path)?;
        file.read_to_end(&mut buf)?;
        match serde_json::from_slice::<HostManifest>(&buf) {
            Ok(m) => Ok(m),
            Err(_) => Err("Failed to deserialize json".into())
        }
    }
}

#[cfg(test)]
mod test {
    use crate::manifest::ConfigEntry;
    use std::collections::HashMap;
    #[test]    
    fn round_trip() {
        let manifest = super::HostManifest{
            actors: vec!["a".to_string(), "b".to_string(), "c".to_string()],
            capabilities: vec!["wascc:one".to_string(), "wascc:two".to_string()],
            config: vec![
                ConfigEntry{
                    actor: "a".to_string(),
                    capability: "wascc:one".to_string(),
                    values: gen_values(),
                }
            ],
        };
        let yaml = serde_yaml::to_string(&manifest).unwrap();
        assert_eq!(yaml, "---\nactors:\n  - a\n  - b\n  - c\ncapabilities:\n  - \"wascc:one\"\n  - \"wascc:two\"\nconfig:\n  - actor: a\n    capability: \"wascc:one\"\n    values:\n      ROOT: /tmp");
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