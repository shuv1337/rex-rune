//! Emit a QR symbol as a PBM with a four-module quiet zone, for independent decoding.
//! Usage: cargo run --locked --no-default-features --example qr_dump -- input.bin > out.pbm
use std::io::{self, Write};
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = std::env::args_os().nth(1).ok_or("expected input file")?;
    let symbol = rex_rune::qr::encode(&std::fs::read(path)?)?;
    let size = symbol.size() + 8;
    let mut out = io::BufWriter::new(io::stdout().lock());
    writeln!(out, "P1\n{size} {size}")?;
    for y in 0..size {
        for x in 0..size {
            let dark =
                x >= 4 && y >= 4 && x < size - 4 && y < size - 4 && symbol.dark(x - 4, y - 4);
            write!(out, "{} ", u8::from(dark))?;
        }
        writeln!(out)?;
    }
    Ok(())
}
