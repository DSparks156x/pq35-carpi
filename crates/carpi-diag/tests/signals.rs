//! Every name carpi-signals puts in the store has a catalog row, so the
//! picker can label and place it.

use carpi_diag::Catalog;
use carpi_signals::MESSAGES;
use std::collections::BTreeSet;

#[test]
fn every_signal_has_a_catalog_row() {
    let mut frames: Vec<[u8; 8]> = vec![[0; 8], [0xFF; 8]];
    // Multiplexed frames: each Haldex page, and both MFA 4 tank cycles.
    for page in 0..8u8 {
        frames.push([0xD0 | page, 0, 0, 0, 0, 0, 0, 0]);
    }
    frames.push([0, 0x80, 0, 0, 0, 0, 0, 0]);
    let mut names = BTreeSet::new();
    for m in MESSAGES {
        for f in &frames {
            (m.decode)(f, &mut |k, _| {
                names.insert(k);
            });
        }
    }
    assert!(names.len() > 180, "{}", names.len());
    let cat = Catalog::seed();
    let missing: Vec<_> = names.iter().filter(|n| cat.get(n).is_none()).collect();
    assert!(missing.is_empty(), "no catalog row: {missing:?}");
    for n in &names {
        assert_eq!(cat.get(n).unwrap().ican_name(), Some(*n), "{n}");
    }
}
