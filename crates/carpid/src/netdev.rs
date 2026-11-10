//! CAN netdev discovery for the legacy single-interface fallback.
use std::path::Path;

pub fn can_interfaces(sys_class_net: &Path) -> std::io::Result<Vec<String>> {
    let mut names = Vec::new();
    for entry in std::fs::read_dir(sys_class_net)? {
        let entry = entry?;
        // Linux ARPHRD_CAN is 280. Ethernet/loopback must not affect the count.
        let kind = std::fs::read_to_string(entry.path().join("type"))?;
        if kind.trim() == "280" {
            names.push(entry.file_name().to_string_lossy().into_owned());
        }
    }
    names.sort();
    Ok(names)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_only_can_netdevs() {
        let dir = tempfile::tempdir().unwrap();
        for (name, kind) in [
            ("eth0", "1"),
            ("lo", "772"),
            ("can0", "280"),
            ("candiag", "280"),
        ] {
            let path = dir.path().join(name);
            std::fs::create_dir(&path).unwrap();
            std::fs::write(path.join("type"), format!("{kind}\n")).unwrap();
        }
        assert_eq!(can_interfaces(dir.path()).unwrap(), ["can0", "candiag"]);
    }

    #[test]
    fn incomplete_inventory_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("can0")).unwrap();
        assert!(can_interfaces(dir.path()).is_err());
    }
}
