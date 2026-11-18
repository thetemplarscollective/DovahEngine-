//! Build a `gfxfontlib.swf` from a `fonts_en.swf` file (M4 tool).
//!
//! Usage: `synthesize_fontlib <fonts_en.swf> <out.gfxfontlib.swf>`
//!
//! The engine synthesizes the fontlib at runtime from the BSA; this
//! example produces the same file on disk, so the harness binary can
//! be run against a Patch-synthesized fontlib (the de-risk check for
//! M4: the HUD must render identically to the M2 proof when the
//! fontlib comes from Patch's 16-font `fonts_en.swf`).

use std::env;
use std::process::ExitCode;

use ruffle_harness::synthesize_fontlib;

fn main() -> ExitCode {
    let args: Vec<String> = env::args().collect();
    if args.len() != 3 {
        eprintln!("usage: synthesize_fontlib <fonts_en.swf> <out.gfxfontlib.swf>");
        return ExitCode::from(2);
    }
    let input = match std::fs::read(&args[1]) {
        Ok(bytes) => bytes,
        Err(e) => {
            eprintln!("failed to read {}: {e}", args[1]);
            return ExitCode::from(1);
        }
    };
    match synthesize_fontlib(&input) {
        Ok(fontlib) => match std::fs::write(&args[2], &fontlib) {
            Ok(()) => {
                println!("wrote {} ({} bytes)", args[2], fontlib.len());
                ExitCode::SUCCESS
            }
            Err(e) => {
                eprintln!("failed to write {}: {e}", args[2]);
                ExitCode::from(1)
            }
        },
        Err(e) => {
            eprintln!("{e}");
            ExitCode::from(1)
        }
    }
}
