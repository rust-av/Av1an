use super::common::*;
use v_frame::chroma::ChromaSubsampling;

mod unit {
    use super::*;

    #[test]
    fn generation_is_deterministic() {
        let first = synthetic_y4m(
            64,
            48,
            8,
            ChromaSubsampling::Yuv420,
            4,
            42,
            Distortion::Mild,
        );
        let second = synthetic_y4m(
            64,
            48,
            8,
            ChromaSubsampling::Yuv420,
            4,
            42,
            Distortion::Mild,
        );
        assert_eq!(
            first, second,
            "identical parameters must produce identical bytes"
        );
    }

    #[test]
    fn different_seeds_produce_different_content() {
        // The seed only affects the noise term, so a non-zero amplitude is
        // required for the seed to be observable.
        let first = SyntheticClip::generate_with_seed(
            64,
            48,
            8,
            ChromaSubsampling::Yuv420,
            3,
            1,
            Distortion::Mild,
        );
        let second = SyntheticClip::generate_with_seed(
            64,
            48,
            8,
            ChromaSubsampling::Yuv420,
            3,
            2,
            Distortion::Mild,
        );
        assert_ne!(first.data, second.data);
    }

    /// With no amplitude there is no noise term, so content is fully determined
    /// by geometry. That is what lets a "distorted" clip built with
    /// [`Distortion::None`] be byte-identical to its reference.
    #[test]
    fn zero_distortion_ignores_the_seed() {
        let first = SyntheticClip::generate_with_seed(
            64,
            48,
            8,
            ChromaSubsampling::Yuv420,
            3,
            1,
            Distortion::None,
        );
        let second = SyntheticClip::generate_with_seed(
            64,
            48,
            8,
            ChromaSubsampling::Yuv420,
            3,
            999,
            Distortion::None,
        );
        assert_eq!(first.data, second.data);
    }
}

mod decoder_integration {
    use super::*;

    #[test]
    fn generated_stream_decodes_to_the_requested_geometry() {
        let clip =
            SyntheticClip::generate(64, 48, 8, ChromaSubsampling::Yuv420, 3, Distortion::None);
        let decoder = clip.decoder();
        let details = decoder.get_video_details();
        assert_eq!(details.width, clip.width);
        assert_eq!(details.height, clip.height);
        assert_eq!(details.bit_depth, usize::from(clip.effective_bit_depth()));
        assert_eq!(details.chroma_sampling, clip.chroma_sampling);
        // Y4M carries no frame count in its header.
        assert_eq!(details.total_frames, None);
    }

    #[test]
    fn the_requested_number_of_frames_is_written() {
        for frames in [1usize, 3, 7] {
            let clip = SyntheticClip::generate(
                64,
                48,
                8,
                ChromaSubsampling::Yuv420,
                frames,
                Distortion::None,
            );
            let mut decoder = clip.decoder();
            let mut decoded = 0;
            while decoder.read_video_frame::<u8>().is_ok() {
                decoded += 1;
            }
            assert_eq!(decoded, frames);
        }
    }
}
