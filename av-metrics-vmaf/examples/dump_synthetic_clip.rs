//! Manual fixture export for reference-tool comparisons, not a scoring assertion.
#[path = "../tests/common/mod.rs"]
mod common;

fn main() {
    let destination = std::env::var("VMAF_DUMP_SYNTHETIC")
        .expect("manual export requires VMAF_DUMP_SYNTHETIC destination");
    let pair = common::SyntheticPair::generate(
        64,
        48,
        8,
        v_frame::chroma::ChromaSubsampling::Yuv420,
        12,
        common::Distortion::None,
    );
    std::fs::write(&destination, &pair.reference.data).expect("the clip should be written");
    eprintln!("wrote the synthetic clip to {destination}");
}
