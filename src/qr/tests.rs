use super::*;

#[test]
fn published_bch_values() {
    // ISO format strings, M masks 0–7; version strings, versions 7–10.
    assert_eq!(
        (0..8).map(format_bits).collect::<Vec<_>>(),
        [
            0x5412, 0x5125, 0x5e7c, 0x5b4b, 0x45f9, 0x40ce, 0x4f97, 0x4aa0
        ]
    );
    assert_eq!(
        (7..=10).map(version_bits).collect::<Vec<_>>(),
        [0x07c94, 0x085bc, 0x09a99, 0x0a4d3]
    );
}

#[test]
fn published_rs_generator_and_remainder() {
    // Degree 10 exponents: 0,251,67,46,61,118,70,64,94,32,45.
    assert_eq!(
        generator(10),
        [1, 216, 194, 159, 111, 199, 94, 95, 113, 157, 193]
    );
    // Standard version 1-M HELLO WORLD example's 16 data / 10 ECC words.
    assert_eq!(
        remainder(
            &[
                32, 91, 11, 120, 209, 114, 220, 77, 67, 64, 236, 17, 236, 17, 236, 17
            ],
            &generator(10)
        ),
        [196, 35, 39, 119, 235, 215, 231, 226, 93, 23]
    );
}

#[test]
fn published_capacities_and_boundaries() {
    let data = [16, 28, 44, 64, 86, 108, 124, 154, 182, 216];
    let capacity = [14, 26, 42, 62, 84, 106, 122, 152, 180, 213];
    for (i, &(total, ecc, blocks)) in BLOCKS.iter().enumerate() {
        assert_eq!(total - ecc * blocks, data[i]);
        assert_eq!(encode(&vec![b'x'; capacity[i]]).unwrap().size(), 21 + i * 4);
        if i < 9 {
            assert_eq!(
                encode(&vec![b'x'; capacity[i] + 1]).unwrap().size(),
                25 + i * 4
            );
        }
    }
    assert_eq!(encode(&[0; 214]), Err(TooLong));
    assert_eq!(encode(&[0; 1000]), Err(TooLong));
    assert_eq!(encode(b"").unwrap().size(), 21);
}

fn fixture(path: &std::path::Path) -> (Vec<u8>, Symbol) {
    let text = std::fs::read_to_string(path).unwrap();
    let mut lines = text.lines();
    let hex = lines.next().unwrap();
    let bytes = (0..hex.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).unwrap())
        .collect();
    let rows: Vec<_> = lines.collect();
    assert!(rows.iter().all(|r| r.len() == rows.len()));
    (
        bytes,
        Symbol {
            size: rows.len(),
            modules: rows
                .iter()
                .flat_map(|r| r.bytes().map(|b| b == b'1'))
                .collect(),
        },
    )
}

#[test]
fn independent_qrencode_matrices() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data/qr");
    let mut paths: Vec<_> = std::fs::read_dir(root)
        .unwrap()
        .map(|p| p.unwrap().path())
        .filter(|p| p.extension().is_some_and(|e| e == "qr"))
        .collect();
    paths.sort();
    let mut same = 0;
    for path in &paths {
        let (data, reference) = fixture(path);
        let mask = (0..8)
            .find(|&m| {
                let mut b = Builder::new((reference.size - 17) / 4);
                b.format(m);
                b.function
                    .iter()
                    .enumerate()
                    .all(|(i, &f)| !f || b.symbol.modules[i] == reference.modules[i])
            })
            .expect("reference format information is level M with valid BCH");
        let (forced, _) = encode_mask(&data, Some(mask)).unwrap();
        assert_eq!(forced, reference, "{} mask {mask}", path.display());
        let (own, chosen) = encode_mask(&data, None).unwrap();
        if chosen == mask {
            same += 1;
        }
        println!(
            "{}: v{} reference mask {mask}, own {chosen}, penalties reference={} own={}",
            path.file_name().unwrap().to_string_lossy(),
            (own.size - 17) / 4,
            penalty(&reference),
            penalty(&own)
        );
        // Explain every difference, rather than accepting unexplained mask drift.
        let scores: Vec<_> = (0..8)
            .map(|m| qrencode_penalty(&encode_mask(&data, Some(m)).unwrap().0))
            .collect();
        let best = scores
            .iter()
            .enumerate()
            .min_by_key(|(_, score)| *score)
            .unwrap()
            .0;
        assert_eq!(
            best,
            mask as usize,
            "qrencode scoring model: {} {scores:?}",
            path.display()
        );
        if chosen != mask {
            println!(
                "  qrencode N3 (one orientation) / rounded N4: {scores:?}; reference wins there, our mask wins with both N3 orientations"
            );
        }
    }
    println!(
        "{} exact forced-mask matches; {same}/{} own-mask agreements",
        paths.len(),
        paths.len()
    );
    assert_eq!(paths.len(), 22);
}

// Test-only model of libqrencode 4.1.1 mask.c, to investigate choice differences.
// It counts a finder-like core once even if BOTH sides qualify and rounds the
// dark percentage before N4. Production counts each orientation and uses the exact ratio.
fn qrencode_penalty(s: &Symbol) -> usize {
    let n = s.size;
    let mut score = 0;
    for vertical in [false, true] {
        for a in 0..n {
            let mut runs = vec![0usize];
            let mut dark = false;
            for b in 0..n {
                let value = if vertical { s.dark(a, b) } else { s.dark(b, a) };
                if value == dark {
                    *runs.last_mut().unwrap() += 1;
                } else {
                    runs.push(1);
                    dark = value;
                }
            }
            for (i, &run) in runs.iter().enumerate() {
                if run >= 5 {
                    score += run - 2;
                }
                if i % 2 == 1 && i >= 3 && i + 2 < runs.len() && run % 3 == 0 {
                    let unit = run / 3;
                    if [runs[i - 2], runs[i - 1], runs[i + 1], runs[i + 2]] == [unit; 4]
                        && (i == 3
                            || runs[i - 3] >= 4 * unit
                            || i + 4 >= runs.len()
                            || runs[i + 3] >= 4 * unit)
                    {
                        score += 40;
                    }
                }
            }
        }
    }
    for y in 0..n - 1 {
        for x in 0..n - 1 {
            let c = s.dark(x, y);
            if c == s.dark(x + 1, y) && c == s.dark(x, y + 1) && c == s.dark(x + 1, y + 1) {
                score += 3;
            }
        }
    }
    let dark = s.modules.iter().filter(|&&v| v).count();
    score + ((200 * dark + n * n) / (n * n) / 2).abs_diff(50) / 5 * 10
}

#[test]
fn finder_penalty_counts_both_orientations_scaled_and_at_edges() {
    let bits = |s: &str| s.bytes().map(|c| c == b'1').collect::<Vec<_>>();
    assert_eq!(finder_penalty(&bits("000010111010000")), 80);
    assert_eq!(finder_penalty(&bits("1011101")), 80);
    assert_eq!(finder_penalty(&bits("000000001100111111001100000000")), 80);
    assert_eq!(finder_penalty(&bits("00001011010000")), 0);
}
