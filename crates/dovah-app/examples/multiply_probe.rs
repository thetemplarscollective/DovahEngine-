//! One-off probe: dump the gradient-multiply output for the
//! real fog textures (FXFogHeavy x GradCobWeb) and print
//! level-0 stats + a PPM, to decide whether the multiply
//! itself is broken for real BC3 data or the problem is
//! downstream (material/mesh/UVs).
//!
//! Usage: cargo run -q -p dovah-app --example multiply_probe
//! Output: /tmp/multiply_probe.ppm (level 0 of the product)

use dovah_app::convert;
use dovah_bsa::BsaArchive;

fn main() {
    let mut src_bytes = None;
    let mut grad_bytes = None;
    for name in [
        "test-data/Skyrim_-_Textures4.bsa",
        "test-data/Skyrim_-_Textures0.bsa",
        "test-data/Skyrim_-_Textures1.bsa",
        "test-data/Skyrim_-_Textures2.bsa",
        "test-data/Skyrim_-_Textures3.bsa",
        "test-data/Skyrim_-_Textures5.bsa",
        "test-data/Skyrim_-_Textures6.bsa",
        "test-data/Skyrim_-_Textures7.bsa",
        "test-data/Skyrim_-_Textures8.bsa",
    ] {
        let Ok(mut archive) = BsaArchive::open(name) else {
            continue;
        };
        // Collect owned names first: list() borrows immutably,
        // extract() needs &mut.
        let names: Vec<String> = archive.list().into_iter().map(str::to_string).collect();
        for stored in &names {
            let lower = stored.to_lowercase();
            if lower == "textures/effects/fxfogheavy.dds" {
                src_bytes = archive.extract(stored).ok();
            } else if lower == "textures/effects/gradients/gradcobweb.dds" {
                grad_bytes = archive.extract(stored).ok();
            }
        }
    }
    let src = src_bytes.expect("FXFogHeavy in Textures BSAs");
    let grad = grad_bytes.expect("GradCobWeb in Textures BSAs");
    println!("source {} bytes, gradient {} bytes", src.len(), grad.len());

    let plain = convert::dds_to_bevy_image(&src).expect("plain source converts");
    let product = convert::dds_multiply_to_bevy_image(&src, &grad).expect("multiply converts");
    for (name, image) in [("plain source", plain), ("product", product)] {
        let data = image.data.as_ref().unwrap();
        let w = image.width();
        let h = image.height();
        let px = &data[..(w * h * 4) as usize];
        let mut sum = [0f64; 4];
        let mut sum2 = [0f64; 4];
        let n = (w * h) as f64;
        for p in px.as_chunks::<4>().0 {
            for c in 0..4 {
                let v = p[c] as f64;
                sum[c] += v;
                sum2[c] += v * v;
            }
        }
        let mean: Vec<f64> = sum.iter().map(|s| s / n).collect();
        let std: Vec<f64> = (0..4)
            .map(|c| (sum2[c] / n - mean[c] * mean[c]).max(0.0).sqrt())
            .collect();
        println!(
            "{name}: {w}x{h} mips={} mean RGBA ({:.1},{:.1},{:.1},{:.1}) std ({:.1},{:.1},{:.1},{:.1})",
            image.texture_descriptor.mip_level_count,
            mean[0],
            mean[1],
            mean[2],
            mean[3],
            std[0],
            std[1],
            std[2],
            std[3]
        );
        if name == "product" {
            let mut ppm = format!("P6\n{w} {h}\n255\n").into_bytes();
            for p in px.as_chunks::<4>().0 {
                ppm.extend_from_slice(&p[..3]);
            }
            std::fs::write("/tmp/multiply_probe.ppm", ppm).unwrap();
            println!("wrote /tmp/multiply_probe.ppm");
        }
    }
}
