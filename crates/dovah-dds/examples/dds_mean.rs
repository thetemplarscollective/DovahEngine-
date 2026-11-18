//! Decode one DDS file from a BSA and print its mean color +
//! dimensions, to check whether a texture is itself dark.
//! Usage: `cargo run -p dovah-dds --example dds_mean -- <bsa> <dds-path>`

use dovah_bsa::BsaArchive;
use dovah_dds::DdsImage;
use std::env;
use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = env::args().skip(1).collect();
    if args.len() < 2 {
        eprintln!("usage: dds_mean <bsa> <dds-path> [out.ppm]");
        return ExitCode::from(2);
    }
    let mut archive = match BsaArchive::open(&args[0]) {
        Ok(a) => a,
        Err(e) => {
            eprintln!("cannot open {}: {e}", args[0]);
            return ExitCode::from(1);
        }
    };
    let bytes = match archive.extract(&args[1]) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("cannot extract {}: {e}", args[1]);
            return ExitCode::from(1);
        }
    };
    let image = match DdsImage::from_bytes(&bytes) {
        Ok(i) => i,
        Err(e) => {
            eprintln!("cannot decode {}: {e:?}", args[1]);
            return ExitCode::from(1);
        }
    };
    let decoded = match image.decode(0) {
        Ok(d) => d,
        Err(e) => {
            eprintln!("cannot decode level 0 of {}: {e:?}", args[1]);
            return ExitCode::from(1);
        }
    };
    let rgba = &decoded.data;
    let n = (rgba.len() / 4) as f64;
    let (mut r, mut g, mut b, mut a) = (0u64, 0u64, 0u64, 0u64);
    for px in rgba.as_chunks::<4>().0 {
        r += u64::from(px[0]);
        g += u64::from(px[1]);
        b += u64::from(px[2]);
        a += u64::from(px[3]);
    }
    println!(
        "{}: {}x{} format={:?} mean RGBA=({:.0}, {:.0}, {:.0}, {:.0})",
        args[1],
        image.width,
        image.height,
        image.format,
        r as f64 / n,
        g as f64 / n,
        b as f64 / n,
        a as f64 / n
    );
    // Optional third arg: write level 0 as a PPM (viewable via PIL).
    if let Some(ppm_path) = args.get(2) {
        let mut ppm = format!("P6\n{} {}\n255\n", image.width, image.height).into_bytes();
        for px in rgba.as_chunks::<4>().0 {
            ppm.extend_from_slice(&px[0..3]);
        }
        std::fs::write(ppm_path, ppm).unwrap();
        println!("wrote {ppm_path}");
    }
    ExitCode::SUCCESS
}
