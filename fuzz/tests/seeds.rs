//! Runs the fuzz harnesses on stable over the seed corpus, deterministic
//! pseudo-random inputs and corrupted snapshots. This checks the harness
//! logic itself; real coverage-guided fuzzing uses `cargo fuzz`.

use std::fs;
use std::path::Path;

use lob::{MatchingEngine, journal};

fn seed_journals() -> Vec<(String, String)> {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("seeds/journal");
    let mut out: Vec<_> = fs::read_dir(dir)
        .expect("seed directory")
        .map(|e| {
            let p = e.unwrap().path();
            let name = p.file_name().unwrap().to_string_lossy().into_owned();
            (name, fs::read_to_string(&p).unwrap())
        })
        .collect();
    out.sort();
    assert!(!out.is_empty());
    out
}

struct XorShift(u64);

impl XorShift {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    fn bytes(&mut self, len: usize) -> Vec<u8> {
        (0..len).map(|_| self.next().to_le_bytes()[0]).collect()
    }
}

#[test]
fn journal_harness_accepts_seed_journals() {
    for (name, text) in seed_journals() {
        journal::parse(&text).unwrap_or_else(|e| panic!("{name}: {e}"));
        lob_fuzz::journal_text(text.as_bytes());
    }
}

#[test]
fn journal_harness_survives_mangled_text() {
    let mut rng = XorShift(0x9e37_79b9_7f4a_7c15);
    for (_, text) in seed_journals() {
        let bytes = text.as_bytes();
        for _ in 0..200 {
            let mut m = bytes.to_vec();
            let flips = 1 + (rng.next() % 4) as usize;
            for _ in 0..flips {
                let i = (rng.next() % m.len() as u64) as usize;
                m[i] = rng.next().to_le_bytes()[0];
            }
            lob_fuzz::journal_text(&m);
        }
    }
}

#[test]
fn command_harness_survives_random_bytes() {
    let mut rng = XorShift(0x2545_f491_4f6c_dd1d);
    for len in [0, 1, 7, 64, 512, 4096] {
        for _ in 0..100 {
            lob_fuzz::commands(&rng.bytes(len));
        }
    }
}

/// Inputs that once crashed a fuzz target; each file is named after the bug.
#[test]
fn snapshot_regressions() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("seeds/snapshot");
    let mut n = 0;
    for entry in fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        lob_fuzz::snapshot_bytes(&fs::read(&path).unwrap());
        n += 1;
    }
    assert!(n > 0);
}

#[test]
fn snapshot_harness_on_real_and_corrupted_snapshots() {
    let mut rng = XorShift(0xdead_beef_cafe_f00d);
    for (_, text) in seed_journals() {
        let mut engine = MatchingEngine::new();
        engine.replay(journal::parse(&text).unwrap());
        let bytes = engine.snapshot().encode();
        lob_fuzz::snapshot_bytes(&bytes);
        for _ in 0..500 {
            let mut m = bytes.clone();
            let i = (rng.next() % m.len() as u64) as usize;
            m[i] ^= 1 << (rng.next() % 8);
            lob_fuzz::snapshot_bytes(&m);
            m.truncate(i);
            lob_fuzz::snapshot_bytes(&m);
        }
    }
    for len in [0, 8, 24, 40, 72, 200] {
        lob_fuzz::snapshot_bytes(&rng.bytes(len));
    }
}
