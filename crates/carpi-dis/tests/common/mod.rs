#![allow(dead_code)]

use carpi_dis::cmd::records;
use carpi_dis::fb::Fb;
use carpi_dis::pack::Packed;
use std::path::PathBuf;

/// Compare against `tests/golden/<name>.txt`. Writes the file when it's missing
/// or `UPDATE_GOLDENS` is set; review the diff before committing.
pub fn golden(name: &str, got: &str) {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/golden")
        .join(format!("{name}.txt"));
    if std::env::var_os("UPDATE_GOLDENS").is_some() || !path.exists() {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, got).unwrap();
        return;
    }
    let want = std::fs::read_to_string(&path)
        .unwrap()
        .replace("\r\n", "\n");
    assert!(
        want == got,
        "golden {name} differs; rerun with UPDATE_GOLDENS=1 to accept.\n--- want\n{want}\n--- got\n{got}"
    );
}

/// One line per message: index, size, gap, hex records, then group spans.
pub fn dump(p: &Packed) -> String {
    let mut s = format!("# {} messages, {} B\n", p.messages.len(), p.bytes());
    for (i, m) in p.messages.iter().enumerate() {
        let recs: Vec<String> = records(&m.bytes)
            .unwrap()
            .iter()
            .map(|r| {
                r.iter()
                    .map(|b| format!("{b:02X}"))
                    .collect::<Vec<_>>()
                    .join(" ")
            })
            .collect();
        s += &format!(
            "msg {i} ({} B, gap {} ms)\n",
            m.bytes.len(),
            m.gap.as_millis()
        );
        for r in recs {
            s += &format!("  {r}\n");
        }
        let spans: Vec<String> = p
            .spans
            .iter()
            .filter(|g| g.msg == i)
            .map(|g| format!("{}@{}..{}", g.tag, g.start, g.end))
            .collect();
        s += &format!("  groups: {}\n", spans.join(" "));
    }
    s
}

pub fn apply(fb: &mut Fb, p: &Packed) {
    for m in &p.messages {
        fb.apply(&m.bytes);
    }
}
